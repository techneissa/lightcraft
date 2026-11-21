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
