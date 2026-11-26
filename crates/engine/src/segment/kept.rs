//! Fork (issues #353 and #407, `fork/mask-models-kept.md`): while the Masking panel is open in the front window, an AI
//! mask model stays loaded while the open photo has an AI mask that uses it; brush, gradient and range masks hold none.
//! Each AI mask can let its models go at once (`mask.stopAi`). The panel says when a mask waits for a model to load.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Mask, MaskShape};

/// The AI mask models, each on or off: the ones a mask uses, the ones held, the ones loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaskModels {
    pub sam3: bool,
    pub birefnet: bool,
    /// BiRefNet Matting, the hair edges of Subject, Background and Person masks.
    pub matting: bool,
}

impl MaskModels {
    pub const NONE: MaskModels = MaskModels { sam3: false, birefnet: false, matting: false };

    pub fn any(self) -> bool {
        self.sam3 || self.birefnet || self.matting
    }

    /// The models in both.
    pub fn both(self, o: MaskModels) -> MaskModels {
        MaskModels { sam3: self.sam3 && o.sam3, birefnet: self.birefnet && o.birefnet, matting: self.matting && o.matting }
    }

    /// The models in either.
    pub fn either(self, o: MaskModels) -> MaskModels {
        MaskModels { sam3: self.sam3 || o.sam3, birefnet: self.birefnet || o.birefnet, matting: self.matting || o.matting }
    }

    /// The ids of the models that are on (`sam3`, `birefnet`, `matting`).
    pub fn ids(self) -> Vec<&'static str> {
        [(self.sam3, "sam3"), (self.birefnet, "birefnet"), (self.matting, "matting")].into_iter().filter_map(|(on, id)| on.then_some(id)).collect()
    }
}

impl super::Segmenter {
    /// Whether the request under way waits for a model to load, or to be planned on the GPU at its first run after a
    /// load (Core ML), rather than for the selection itself.
    pub fn loading_models(&self) -> bool {
        #[cfg(feature = "sam")]
        {
            if self.background && self.in_flight.is_empty() {
                return false;
            }
            self.worker.loading() && self.worker.pending() > 0
        }
        #[cfg(not(feature = "sam"))]
        false
    }

    /// Keep `models`' SAM 3 and BiRefNet loaded; the others go after the worker's idle time.
    pub fn keep_models(&self, models: MaskModels) {
        #[cfg(feature = "sam")]
        self.worker.hold(models);
        #[cfg(not(feature = "sam"))]
        let _ = models;
    }
}

impl crate::Session {
    /// Keep `models` loaded (the others go after the idle time): the app keeps the models the open photo's AI masks use
    /// while the Masking panel is open in the front window, so a mask made after a pause does not load them again.
    pub fn keep_mask_models(&self, models: MaskModels) {
        self.segmenter.keep_models(models);
        #[cfg(feature = "sam")]
        self.hair_edges.keep_loaded(models.matting);
    }

    /// The models an AI part `shape` is made with (brush, gradient and range parts: none). Whether a model is installed
    /// is not asked: one that is not never loads, so holding it costs nothing.
    pub fn mask_models_of(&self, shape: &MaskShape) -> MaskModels {
        use crate::hair_edges::EdgeKind;
        if !crate::segment::Segmenter::AVAILABLE {
            return MaskModels::NONE;
        }
        match shape {
            MaskShape::Subject { .. } | MaskShape::Background { .. } | MaskShape::Sky { .. } => {
                let Some(c) = crate::cmd::concepts::of(shape) else { return MaskModels::NONE };
                let model = self.segmenter.concept_model(c.kind);
                MaskModels {
                    sam3: model.uses_sam3(),
                    birefnet: model.uses_birefnet(),
                    matting: c.kind != "sky" && self.hair_edges.may_use_matting(EdgeKind::Subject),
                }
            }
            MaskShape::Object { .. } | MaskShape::Prompt { .. } => MaskModels { sam3: true, ..MaskModels::NONE },
            MaskShape::People { .. } => MaskModels { matting: self.hair_edges.may_use_matting(EdgeKind::People), ..MaskModels::NONE },
            _ => MaskModels::NONE,
        }
    }

    /// The models mask `m`'s parts are made with.
    pub fn mask_models_of_mask(&self, m: &Mask) -> MaskModels {
        m.components.iter().fold(MaskModels::NONE, |a, c| a.either(self.mask_models_of(&c.shape)))
    }

    /// The models photo `id`'s masks are made with.
    pub fn mask_models_on(&self, id: PhotoId) -> MaskModels {
        let Some(d) = self.develop_of(id) else { return MaskModels::NONE };
        d.masks.iter().fold(MaskModels::NONE, |a, m| a.either(self.mask_models_of_mask(m)))
    }

    /// The models in memory now, or working (a request on the AI mask worker counts for SAM 3 and BiRefNet: it may be
    /// loading them).
    pub fn mask_models_in_use(&self) -> MaskModels {
        #[cfg(feature = "sam")]
        {
            let (loaded, busy) = (self.segmenter.worker.loaded_models(), self.segmenter.worker.pending() + self.segmenter.worker.detail() > 0);
            MaskModels { sam3: loaded.sam3 || busy, birefnet: loaded.birefnet || busy, matting: self.hair_edges.model_loaded() }
        }
        #[cfg(not(feature = "sam"))]
        MaskModels::NONE
    }

    /// Stop the AI of mask `mask` on the open photo (`mask.stopAi`): the requests on the AI mask worker are cancelled
    /// and the models the mask uses are let go now, even while held. The mask keeps its stored selection. The models
    /// let go.
    pub fn stop_mask_ai(&mut self, mask: u32) -> Result<MaskModels, String> {
        let id = self.active().ok_or("no photo is open")?;
        let d = self.develop_of(id).ok_or("no such photo")?;
        let m = d.masks.iter().find(|m| m.id == mask).ok_or_else(|| format!("no mask {mask}"))?;
        let models = self.mask_models_of_mask(m);
        if !models.any() {
            return Err(format!("“{}” is not made by an AI model", m.name));
        }
        #[cfg(feature = "sam")]
        {
            if models.sam3 || models.birefnet {
                for j in &self.segmenter.in_flight {
                    j.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                self.segmenter.worker.release(models);
            }
            if models.matting {
                if m.components.iter().any(|c| matches!(c.shape, MaskShape::People { .. })) {
                    // (a Person mask's matte is made in a people parse)
                    self.parsing.cancel_runs();
                }
                self.hair_edges.release();
            }
        }
        Ok(models)
    }
}

#[cfg(all(test, feature = "sam"))]
mod tests {
    use serde_json::json;

    use super::MaskModels;

    /// Issue #407: masks no AI model makes (brush, gradients, ranges), with the Masking panel's hold on, send no request to
    /// the AI mask worker and make no matte, so no model loads.
    #[test]
    fn brush_gradient_and_range_masks_ask_no_model() {
        let mut s = crate::Session::with_demo();
        s.segmenter.background = true;
        // (an installed-looking SAM 3 and matting model: a request would be sent if anything asked for one)
        let dir = std::env::temp_dir().join(format!("lc-test-sam-no-ai-masks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"\x10\x00\x00\x00\x00\x00\x00\x00{}").unwrap();
        std::fs::write(dir.join("vocab.json"), b"{}").unwrap();
        std::fs::write(dir.join("merges.txt"), b"#version: 0.2\n").unwrap();
        s.segmenter.dir = Some(dir.clone());
        assert!(s.segmenter.installed());
        s.hair_edges.set_runner(Some(std::sync::Arc::new(|_, _, _, _| Err("no model in this test".into()))));
        let id = s.active().unwrap();
        let (jobs, mattes, parses) = (s.segmenter.next_job_id, s.hair_edges.runs(), s.parsing.runs());
        s.execute("mask.add", &json!({"kind": "brush"})).unwrap();
        s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.2], [0.6, 0.4]]})).unwrap();
        s.execute("mask.brushStroke", &json!({"points": [[0.3, 0.3], [0.4, 0.3]], "erase": true})).unwrap();
        for kind in ["linear", "radial", "luminanceRange", "colorRange"] {
            s.execute("mask.add", &json!({"kind": kind})).unwrap();
            s.execute("mask.adjust", &json!({"values": {"exposure": 0.5}})).unwrap();
        }
        s.execute("crop.set", &json!({"rect": [0.1, 0.1, 0.9, 0.9]})).unwrap();
        // (what the Masking panel holds for this photo: nothing)
        assert_eq!(s.mask_models_on(id), MaskModels::NONE, "brush, gradient and range masks hold no model");
        s.keep_mask_models(s.mask_models_on(id));
        assert!(!s.hair_edges.is_held());
        assert_eq!(s.segmenter.next_job_id, jobs, "no request to the AI mask worker");
        assert_eq!((s.hair_edges.runs(), s.parsing.runs()), (mattes, parses), "no matte, no people parse");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Photo `id` with a Person mask (its stored selection `seg`) and a brush mask; the Person mask is selected.
    fn with_a_person_mask(s: &mut crate::Session) -> (lightcraft_catalog::PhotoId, u32, lightcraft_develop::SegMask) {
        use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, SegMask};
        let id = s.active().unwrap();
        s.execute("mask.add", &json!({"kind": "brush"})).unwrap();
        let seg = SegMask::from_logits(4, &[3.0; 16]);
        let mut d = (*s.develop_of(id).unwrap()).clone();
        let person = d.masks.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        let shape = MaskShape::People { person: 0, parts: vec!["person".into()], face: None, seg: Some(seg.clone()), detail: Vec::new(), edge: 0.0 };
        let part = MaskComponent { name: None, op: MaskOp::Add, invert: false, shape };
        d.masks.push(Mask { id: person, name: "Person".into(), components: vec![part], ..Mask::default() });
        s.apply_silent(lightcraft_catalog::Op::SetDevelop { id, settings: std::sync::Arc::new(d), label: "Masks".into(), edited: None }).unwrap();
        s.execute("mask.select", &json!({"id": person})).unwrap();
        (id, person, seg)
    }

    /// Issue #407: a Person mask on the open photo holds BiRefNet Matting (its hair edge) while masking; the review found
    /// that a rule keyed on Subject mattes alone would let it go.
    #[test]
    fn a_person_mask_holds_birefnet_matting() {
        let mut s = crate::Session::with_demo();
        let (id, ..) = with_a_person_mask(&mut s);
        let models = s.mask_models_on(id);
        assert!(models.matting && !models.sam3, "{models:?}");
        s.keep_mask_models(models);
        assert!(s.hair_edges.is_held());
        s.keep_mask_models(MaskModels::NONE);
        assert!(!s.hair_edges.is_held());
    }

    /// Issue #407: Stop AI lets the mask's model go now, though the Masking panel holds it, and the mask keeps its stored
    /// selection. Runs BiRefNet Matting from the shared ArtCraft models folder.
    #[test]
    fn stop_ai_lets_the_model_go_and_keeps_the_selection() {
        let Some(models) = crate::people_parsing::default_models_dir().filter(|d| d.join("birefnet-matting/model.safetensors").is_file()) else {
            lightcraft_testkit::skip!(Model, "BiRefNet Matting is not in the shared ArtCraft models folder");
            return;
        };
        let mut s = crate::Session::with_demo();
        s.hair_edges.set_paths(Some(models), None);
        let (id, person, seg) = with_a_person_mask(&mut s);
        s.keep_mask_models(s.mask_models_on(id));
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        s.hair_edges.matte(&[128; 64 * 64 * 3], 64, 64, &cancel).unwrap();
        assert!(s.hair_edges.model_loaded());
        assert_eq!(s.execute("mask.aiModels", &json!({})).unwrap()["canStop"], true);
        let r = s.execute("mask.stopAi", &json!({})).unwrap();
        assert_eq!(r["stopped"], json!(["matting"]));
        assert!(!s.hair_edges.model_loaded(), "let go while held");
        assert_eq!(s.execute("mask.aiModels", &json!({})).unwrap()["canStop"], false);
        let d = s.develop_of(id).unwrap();
        let m = d.masks.iter().find(|m| m.id == person).unwrap();
        assert!(matches!(&m.components[0].shape, lightcraft_develop::MaskShape::People { seg: Some(kept), .. } if *kept == seg));
        // (a brush mask has nothing to stop)
        let brush = d.masks.iter().find(|m| m.id != person).unwrap().id;
        assert!(s.execute("mask.stopAi", &json!({"id": brush})).is_err());
        s.keep_mask_models(MaskModels::NONE);
    }
}
