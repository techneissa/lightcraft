//! The original of the photo open for editing, decoded ahead of the first zoom to 100 % (#182).
//!
//! A window of the loupe zoomed past what one whole-frame render holds reads the photo's original at full size
//! ([`SourceLevel::Full`]). Decoded by the first window job, it made the first zoom wait for the whole decode (about
//! 2 s for a 45 MP raw on a test Mac, more on a slow drive). A frontend starts a [`FullDecodeJob`] once the photo is
//! on screen; its result goes into the same cache the window jobs read ([`crate::Session::accept_full_decode`]), so
//! the first zoom finds it there. The job only decodes: nothing is rendered.

use std::sync::atomic::{AtomicU8, Ordering};

use super::source::{DecodedSource, LoadOptions, SourceRef};
use super::{Hash128, PhotoId, SourceLevel};

/// Where a [`FullDecodeJob`] is: waiting at the memory gate, decoding, or stopped. Shared by the job and whoever
/// started it; each move happens once, so a window that stops a queued job and the job that starts decoding never
/// both win.
#[derive(Debug, Default)]
pub struct FullDecodeState(AtomicU8);

const QUEUED: u8 = 0;
const DECODING: u8 = 1;
const STOPPED: u8 = 2;

impl FullDecodeState {
    /// Stop the job: a queued one never decodes, a decoding one's result is dropped.
    pub fn stop(&self) {
        self.0.store(STOPPED, Ordering::SeqCst);
    }

    /// Stop the job if it is still waiting at the memory gate. Whether it was (and is now stopped).
    pub fn stop_if_queued(&self) -> bool {
        self.0.compare_exchange(QUEUED, STOPPED, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    /// Past the memory gate and decoding (not stopped).
    pub fn is_decoding(&self) -> bool {
        self.0.load(Ordering::SeqCst) == DECODING
    }

    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst) == STOPPED
    }

    /// Queued → decoding, unless it was stopped first.
    fn begin(&self) -> bool {
        self.0.compare_exchange(QUEUED, DECODING, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }
}

/// The memory gate weight of a full-size load, per byte of the file (as the file system's loader weighs it).
const GATE_WEIGHT: usize = 6;

/// Decodes photo `photo`'s original at full size ([`crate::Session::full_decode_job`]).
pub struct FullDecodeJob {
    pub photo: PhotoId,
    source_key: Hash128,
    source: SourceRef,
}

/// What a [`FullDecodeJob`] decoded.
pub struct FullDecoded {
    pub photo: PhotoId,
    source_key: Hash128,
    pub loaded: Result<DecodedSource, String>,
}

impl FullDecodeJob {
    /// Decode (on a worker: seconds for a large raw), as `state` allows; `None` when it was stopped.
    ///
    /// It first waits at the memory gate like other background work, weighing what a full-size load of the file
    /// does, so a render the user waits for (or AI Denoise holding the gate) goes first; stopped meanwhile, it gives
    /// up at once. Past the gate it is `decoding` (a window waits for it from then on) and loads without queueing
    /// again.
    pub fn run(self, state: &FullDecodeState) -> Option<FullDecoded> {
        let len = match &self.source {
            SourceRef::File { path, .. } => std::fs::metadata(path).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0),
            _ => 0,
        };
        let permit = crate::memory::work_gate().acquire_unless(len.saturating_mul(GATE_WEIGHT), || state.is_stopped())?;
        if !state.begin() {
            return None;
        }
        // (the load takes its own place at the gate, as interactive work: this one has waited its turn)
        drop(permit);
        let loaded = self.source.load_source();
        (!state.is_stopped()).then_some(FullDecoded { photo: self.photo, source_key: self.source_key, loaded })
    }
}

impl crate::Session {
    /// A job decoding photo `id`'s original at full size, when a window job would have to read its file for it:
    /// `None` when the original is decoded already, for a video, or when the photo's pixels don't come from a file
    /// (a demo scene, cheap to make).
    pub fn full_decode_job(&mut self, id: PhotoId) -> Option<FullDecodeJob> {
        if self.media.has_source(id, SourceLevel::Full) {
            return None;
        }
        let p = self.catalog.photo(id)?.clone();
        if p.is_video() {
            return None;
        }
        let source = self.media.source_ref(&p, SourceLevel::Full);
        if !matches!(source, SourceRef::File { .. }) {
            return None;
        }
        Some(FullDecodeJob { photo: id, source_key: super::quick::source_identity(&p), source })
    }

    /// Keep what a [`FullDecodeJob`] decoded as photo `photo`'s original, unless the photo changed meanwhile (its file,
    /// or the settings the decode depends on). Whether it was kept.
    pub fn accept_full_decode(&mut self, done: &FullDecoded) -> bool {
        let Ok(src) = &done.loaded else { return false };
        self.accept_loaded(done.photo, SourceLevel::Full, Some(done.source_key), src)
    }

    /// Whether `src` (decoded for photo `id` at `level`, from the file `source_key` names) still serves the photo.
    pub(crate) fn loaded_serves(&self, id: PhotoId, source_key: Option<Hash128>, src: &DecodedSource) -> bool {
        let photo = self.catalog.photo(id);
        source_key.is_none_or(|key| photo.is_some_and(|p| super::quick::source_identity(p) == key))
            // (not a source decoded with settings the photo no longer has: one decoded for them replaces it)
            && photo.is_none_or(|p| src.serves(LoadOptions::of(&p.develop)))
    }
}
