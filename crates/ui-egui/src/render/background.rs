//! What the renderer lets go when the window has been in the background a while (issue #304, `fork/idle-memory.md`).

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
}
