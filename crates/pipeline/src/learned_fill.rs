//! Fork feature (`fork/lama-fill.md`): the learned fill (LaMa) for Remove and Content-Aware spots.
//!
//! The pipeline cannot depend on the model (it is a layer above), so a spot whose [`lightcraft_develop::SpotFill`] is
//! `Lama` asks a [`LearnedFill`] that the engine installs once ([`install`]). The fill runs before white balance
//! ([`crate::spots_learned`]), so its result does not depend on the white balance sliders, and the provider caches it by
//! the pixels it was given: a slider drag asks again and gets the cached fill at once.
//!
//! No provider, a provider without its model, a cancelled or failed run: [`LearnedFill::fill`] says `None` and the
//! spot is drawn with the Classic fill instead. Nothing here panics.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// What one render holds across all its learned spots: one stop flag (the provider's activity row cancels it, so ✕
/// stops the whole render's fills, not the one spot being filled) and whatever the provider wants kept until the render
/// ends (its activity row).
#[derive(Default)]
pub struct FillRun {
    stop: Arc<AtomicBool>,
    held: Mutex<Vec<Box<dyn Any + Send>>>,
}

impl FillRun {
    /// A run that is not stopped.
    pub fn new() -> Self {
        Self::default()
    }

    /// The flag a row's ✕ sets; every fill of the render checks it.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Whether the render's fills were stopped.
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Keep `item` (an activity row) until the run is dropped, when the render's last spot is done.
    pub fn hold(&self, item: Box<dyn Any + Send>) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).push(item);
    }

    /// Whether the provider already holds something (so it starts one row for the render, not one per spot).
    pub fn holds(&self) -> bool {
        !self.held.lock().unwrap_or_else(PoisonError::into_inner).is_empty()
    }
}

/// A learned fill of a hole in a picture.
pub trait LearnedFill: Send + Sync {
    /// Rebuild the pixels of the `w × h` linear picture `rgb` (three `f32` per pixel) where `hole > 0`; the result is
    /// the whole picture, `w · h · 3` values. `run` is the render this spot belongs to (see [`FillRun`]). `draft` is a
    /// render that follows a slider drag: it takes a fill that is already known (cached) and never starts a run. `None`:
    /// no learned fill for this one (no model, stopped, failed, or a draft without a cached fill), so the spot uses the
    /// Classic fill. The provider says why in its own message.
    fn fill(&self, run: &FillRun, rgb: &[f32], hole: &[f32], w: usize, h: usize, draft: bool) -> Option<Arc<Vec<f32>>>;
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
