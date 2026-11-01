//! Fork feature (`fork/lama-fill.md`): the learned fill (LaMa) for Remove and Content-Aware spots.
//!
//! The pipeline cannot depend on the model (it is a layer above), so a spot whose [`lightcraft_develop::SpotFill`] is
//! `Lama` asks a [`LearnedFill`] that the engine installs once ([`install`]). The fill runs before white balance
//! ([`crate::spots_learned`]), so its result does not depend on the white balance sliders, and the provider caches it by
//! the pixels it was given: a slider drag asks again and gets the cached fill at once.
//!
//! No provider, a provider without its model, a cancelled or failed run: [`LearnedFill::fill`] says `None` and the
//! spot is drawn with the Classic fill instead. Nothing here panics.

use std::sync::{Arc, PoisonError, RwLock};

/// A learned fill of a hole in a picture.
pub trait LearnedFill: Send + Sync {
    /// Rebuild the pixels of the `w × h` linear picture `rgb` (three `f32` per pixel) where `hole > 0`; the result is
    /// the whole picture, `w · h · 3` values. `draft` is a render that follows a slider drag: it takes a fill that is
    /// already known (cached) and never starts a run. `None`: no learned fill for this one (no model, cancelled,
    /// failed, or a draft without a cached fill), so the spot uses the Classic fill. The provider says why in its own
    /// message.
    fn fill(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, draft: bool) -> Option<Vec<f32>>;
}

static PROVIDER: RwLock<Option<Arc<dyn LearnedFill>>> = RwLock::new(None);

/// Make `provider` the learned fill every render uses (the last one installed wins; `None` removes it).
pub fn install(provider: Option<Arc<dyn LearnedFill>>) {
    *PROVIDER.write().unwrap_or_else(PoisonError::into_inner) = provider;
}

/// The installed provider, if any.
pub fn current() -> Option<Arc<dyn LearnedFill>> {
    PROVIDER.read().unwrap_or_else(PoisonError::into_inner).clone()
}
