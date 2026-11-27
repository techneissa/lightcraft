//! What the renderer lets go when the window has been in the background a while (issue #304, `fork/idle-memory.md`),
//! and when the loupe has rested at Fit in front (issue #436).

use super::Renderer;

impl Renderer {
    /// Free what the photo in front can do without: the per-view stage caches (CPU images and GPU buffers that make a
    /// slider drag redo only what changed; the first drag back rebuilds them), the full-size picture (its disk copy
    /// stays; the same look is built again when the window is back) and the GPU's recycled buffers. The textures on screen and the renders in the preview cache stay, so the
    /// window looks as it did.
    pub fn release_for_background(&mut self) {
        for stage in self.stages.values() {
            stage.clear();
        }
        self.full.free_for_background();
        lightcraft_engine::gpu::trim_pool(0);
    }

    /// Whether the view is at rest with nothing on screen drawn from the full-size original: no render pending, and
    /// neither a zoom window nor the full-size picture drawn (what shows comes from the preview level).
    pub fn rests_without_original(&self) -> bool {
        self.pending.is_empty() && !self.window_on_screen && !self.full.on_screen
    }

    /// The loupe has rested at Fit (issue #436): let go of the full-size picture until the next zoom (its disk copy
    /// stays). The stage caches stay: they make the next slider tick redo only what changed.
    pub fn release_at_rest(&mut self) {
        self.full.free_at_rest();
    }
}
