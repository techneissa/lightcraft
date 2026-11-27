//! Fork (issues #353, #407 and #452, `fork/mask-models-kept.md`): which AI mask models a mask uses (brush, gradient and
//! range masks use none) and which are in memory. The models go a minute after the last AI request (`idle`); each AI mask
//! can let its models go at once (`mask.stopAi`). The panel says when a mask waits for a model to load.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Mask, MaskShape};

/// The AI mask models, each on or off: the ones a mask uses, the ones loaded.
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

    /// The models any of `all` has.
    pub fn union(all: impl IntoIterator<Item = MaskModels>) -> MaskModels {
        all.into_iter().fold(MaskModels::NONE, MaskModels::either)
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
}

impl crate::Session {
    /// The models an AI part `shape` on photo `photo` is made with (brush, gradient and range parts: none). A Subject,
    /// Background or Sky part on a photo with its own matte for it, or with no model for it on this computer, uses none
    /// (as `concepts::wanted` decides when it is made). Reads a few files (the installed selection models).
    pub fn mask_models_of(&self, photo: PhotoId, shape: &MaskShape) -> MaskModels {
        use crate::hair_edges::EdgeKind;
        if !crate::segment::Segmenter::AVAILABLE {
            return MaskModels::NONE;
        }
        match shape {
            MaskShape::Subject { .. } | MaskShape::Background { .. } | MaskShape::Sky { .. } => {
                let Some(c) = crate::cmd::concepts::of(shape) else { return MaskModels::NONE };
                if !crate::cmd::concepts::wanted(self, photo, shape) {
                    return MaskModels::NONE;
                }
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

    /// The models mask `m` of photo `photo` is made with.
    pub fn mask_models_of_mask(&self, photo: PhotoId, m: &Mask) -> MaskModels {
        MaskModels::union(m.components.iter().map(|c| self.mask_models_of(photo, &c.shape)))
    }

    /// The models each mask of photo `photo` is made with, by mask id.
    pub fn mask_models_by_mask(&self, photo: PhotoId) -> Vec<(u32, MaskModels)> {
        let Some(d) = self.develop_of(photo) else { return Vec::new() };
        d.masks.iter().map(|m| (m.id, self.mask_models_of_mask(photo, m))).collect()
    }

    /// The models in memory now.
    pub fn mask_models_loaded(&self) -> MaskModels {
        #[cfg(feature = "sam")]
        {
            MaskModels { matting: self.hair_edges.model_loaded(), ..self.segmenter.worker.loaded_models() }
        }
        #[cfg(not(feature = "sam"))]
        MaskModels::NONE
    }

    /// Whether Stop AI does something for mask `mask` of photo `photo`, which uses `uses`: one of those models is in
    /// memory, or a request for this mask is under way (on the AI mask worker, or Update AI Masks finding its parts).
    pub fn mask_ai_stoppable(&self, photo: PhotoId, mask: u32, uses: MaskModels) -> bool {
        uses.both(self.mask_models_loaded()).any() || self.segmenter.mask_busy(photo, mask) || self.adaptive.asking_for(photo, mask)
    }

    /// Stop the AI of mask `mask` on the open photo (`mask.stopAi`): this mask's requests are cancelled (on the AI mask
    /// worker and Update AI Masks'), and the models it uses are let go once the step under way ends. The
    /// mask keeps its stored selection. Returns the models it uses.
    pub fn stop_mask_ai(&mut self, mask: u32) -> Result<MaskModels, String> {
        let id = self.active().ok_or("no photo is open")?;
        let d = self.develop_of(id).ok_or("no such photo")?;
        let m = d.masks.iter().find(|m| m.id == mask).ok_or_else(|| format!("no mask {mask}"))?;
        let models = self.mask_models_of_mask(id, m);
        if !models.any() {
            return Err(format!("“{}” uses none of the AI models that stay loaded (SAM 3, BiRefNet, BiRefNet Matting)", m.name));
        }
        self.segmenter.cancel_mask(id, mask);
        self.adaptive.stop_mask(id, mask);
        #[cfg(feature = "sam")]
        {
            if models.sam3 || models.birefnet {
                self.segmenter.worker.release(models);
            }
            if models.matting {
                // (a Person mask's people parse runs inside the command that makes the mask, so none is left to stop)
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

    /// A folder removed when the test ends, passed or not.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Issue #407: masks no AI model makes (brush, gradients, ranges) send no request to the AI mask worker and make no
    /// matte, so no model loads.
    #[test]
    fn brush_gradient_and_range_masks_ask_no_model() {
        let mut s = crate::Session::with_demo();
        s.segmenter.background = true;
        // (an installed-looking SAM 3 and matting model: a request would be sent if anything asked for one)
        let dir = TempDir(std::env::temp_dir().join(format!("lc-test-sam-no-ai-masks-{}", std::process::id())));
        let dir = &dir.0;
        std::fs::create_dir_all(dir).unwrap();
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
        assert!(s.mask_models_by_mask(id).iter().all(|(_, m)| *m == MaskModels::NONE), "brush, gradient and range masks use no model");
        assert_eq!(s.segmenter.next_job_id, jobs, "no request to the AI mask worker");
        assert_eq!((s.hair_edges.runs(), s.parsing.runs()), (mattes, parses), "no matte, no people parse");
    }

    /// Photo `id` with a Person mask (its stored selection `seg`) and a brush mask; the Person mask is selected.
    fn with_a_person_mask(s: &mut crate::Session) -> Option<(lightcraft_catalog::PhotoId, u32, lightcraft_develop::SegMask)> {
        use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, SegMask};
        let id = s.active()?;
        s.execute("mask.add", &json!({"kind": "brush"})).ok()?;
        let seg = SegMask::from_logits(4, &[3.0; 16]);
        let mut d = (*s.develop_of(id)?).clone();
        let person = d.masks.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        let shape = MaskShape::People { person: 0, parts: vec!["person".into()], face: None, seg: Some(seg.clone()), detail: Vec::new(), edge: 0.0 };
        let part = MaskComponent { name: None, op: MaskOp::Add, invert: false, shape };
        d.masks.push(Mask { id: person, name: "Person".into(), components: vec![part], ..Mask::default() });
        s.apply_silent(lightcraft_catalog::Op::SetDevelop { id, settings: std::sync::Arc::new(d), label: "Masks".into(), edited: None }).ok()?;
        s.execute("mask.select", &json!({"id": person})).ok()?;
        Some((id, person, seg))
    }

    /// Issue #407: a Person mask on the open photo uses BiRefNet Matting (its hair edge); the review found that a rule
    /// keyed on Subject mattes alone would miss it.
    #[test]
    fn a_person_mask_uses_birefnet_matting() {
        let mut s = crate::Session::with_demo();
        let (id, person, _) = with_a_person_mask(&mut s).unwrap();
        let uses = s.mask_models_by_mask(id);
        let models = uses.iter().find(|(m, _)| *m == person).map(|(_, m)| *m).unwrap();
        assert!(models.matting && !models.sam3, "{models:?}");
    }

    /// Issue #407: Stop AI lets the mask's model go now, and the mask keeps its stored
    /// selection. Runs BiRefNet Matting from the shared ArtCraft models folder.
    #[test]
    fn stop_ai_lets_the_model_go_and_keeps_the_selection() {
        let Some(models) = crate::people_parsing::default_models_dir().filter(|d| d.join("birefnet-matting/model.safetensors").is_file()) else {
            lightcraft_testkit::skip!(Model, "BiRefNet Matting is not in the shared ArtCraft models folder");
            return;
        };
        let mut s = crate::Session::with_demo();
        s.hair_edges.set_paths(Some(models), None);
        let (id, person, seg) = with_a_person_mask(&mut s).unwrap();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        s.hair_edges.matte(&[128; 64 * 64 * 3], 64, 64, &cancel).unwrap();
        assert!(s.hair_edges.model_loaded());
        assert_eq!(s.execute("mask.aiModels", &json!({})).unwrap()["canStop"], true);
        let r = s.execute("mask.stopAi", &json!({})).unwrap();
        assert_eq!(r["stopped"], json!(["matting"]));
        assert!(!s.hair_edges.model_loaded());
        assert_eq!(s.execute("mask.aiModels", &json!({})).unwrap()["canStop"], false);
        let d = s.develop_of(id).unwrap();
        let m = d.masks.iter().find(|m| m.id == person).unwrap();
        assert!(matches!(&m.components[0].shape, lightcraft_develop::MaskShape::People { seg: Some(kept), .. } if *kept == seg));
        // (a brush mask has nothing to stop)
        let brush = d.masks.iter().find(|m| m.id != person).unwrap().id;
        assert!(s.execute("mask.stopAi", &json!({"id": brush})).is_err());
    }

    /// Issue #407 (review): Stop AI on one mask cancels that mask's requests only; another AI mask's request goes on.
    #[test]
    fn stop_ai_on_one_mask_leaves_another_masks_request() {
        let mut s = crate::Session::with_demo();
        let (id, first, _) = with_a_person_mask(&mut s).unwrap();
        let (_, second, _) = with_a_person_mask(&mut s).unwrap();
        let mut flags = Vec::new();
        for mask in [first, second] {
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let tag = super::super::Tag::Clicks { photo: id, mask, comp: 0, hint: Vec::new(), exclude: Vec::new() };
            let task = s.activity.start_background("segment", "test", crate::activity::Cancel::Flag(cancel.clone()));
            s.segmenter.in_flight.push(super::super::InFlight { id: u64::from(mask), tag, cancel: cancel.clone(), detail: false, _task: task });
            flags.push(cancel);
        }
        let uses = MaskModels { matting: true, ..MaskModels::NONE };
        assert!(s.mask_ai_stoppable(id, second, uses), "a request for it is under way");
        s.stop_mask_ai(first).unwrap();
        let cancelled = |i: usize| flags[i].load(std::sync::atomic::Ordering::Relaxed);
        assert!(cancelled(0) && !cancelled(1), "only the stopped mask's request");
        assert!(s.segmenter.mask_busy(id, second) && !s.segmenter.mask_busy(id, first));
    }
}
