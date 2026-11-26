//! Fork (issue #353, `fork/mask-models-kept.md`): the AI mask models stay loaded while the Masking panel is open in the
//! front window, and the panel says when a mask waits for a model to load.

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

    /// Keep SAM 3 and BiRefNet loaded while `on`; once off, they go after the worker's idle time.
    pub fn keep_models(&self, on: bool) {
        #[cfg(feature = "sam")]
        self.worker.hold(on);
        #[cfg(not(feature = "sam"))]
        let _ = on;
    }
}

impl crate::Session {
    /// Keep the AI mask models (SAM 3, BiRefNet, BiRefNet Matting) loaded while `on`: the app turns it on while the
    /// Masking panel is open in the front window, so a mask made after a pause does not load and plan them again.
    pub fn keep_mask_models(&self, on: bool) {
        self.segmenter.keep_models(on);
        #[cfg(feature = "sam")]
        self.hair_edges.keep_loaded(on);
    }
}

#[cfg(all(test, feature = "sam"))]
mod tests {
    use serde_json::json;

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
        s.keep_mask_models(true);
        let (jobs, mattes, parses) = (s.segmenter.next_job_id, s.hair_edges.runs(), s.parsing.runs());
        s.execute("mask.add", &json!({"kind": "brush"})).unwrap();
        s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.2], [0.6, 0.4]]})).unwrap();
        s.execute("mask.brushStroke", &json!({"points": [[0.3, 0.3], [0.4, 0.3]], "erase": true})).unwrap();
        for kind in ["linear", "radial", "luminanceRange", "colorRange"] {
            s.execute("mask.add", &json!({"kind": kind})).unwrap();
            s.execute("mask.adjust", &json!({"values": {"exposure": 0.5}})).unwrap();
        }
        s.execute("crop.set", &json!({"rect": [0.1, 0.1, 0.9, 0.9]})).unwrap();
        assert_eq!(s.segmenter.next_job_id, jobs, "no request to the AI mask worker");
        assert_eq!((s.hair_edges.runs(), s.parsing.runs()), (mattes, parses), "no matte, no people parse");
        s.keep_mask_models(false);
        let _ = std::fs::remove_dir_all(dir);
    }
}
