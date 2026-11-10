//! The original of the photo open for editing, decoded ahead of the first zoom to 100 % (#182).
//!
//! A window of the loupe zoomed past what one whole-frame render holds reads the photo's original at full size
//! ([`SourceLevel::Full`]). Decoded by the first window job, it made the first zoom wait for the whole decode (about
//! 2 s for a 45 MP raw on a test Mac, more on a slow drive). A frontend starts a [`FullDecodeJob`] once the photo is
//! on screen; its result goes into the same cache the window jobs read ([`crate::Session::accept_full_decode`]), so
//! the first zoom finds it there. The job only decodes: nothing is rendered.

use super::source::{DecodedSource, LoadOptions, SourceRef};
use super::{Hash128, PhotoId, SourceLevel};

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
    /// Decode (on a worker: seconds for a large raw). Held back like other background work
    /// ([`crate::memory::in_background`]), so a render the user waits for goes first.
    pub fn run(self) -> FullDecoded {
        let loaded = crate::memory::in_background(|| self.source.load_source());
        FullDecoded { photo: self.photo, source_key: self.source_key, loaded }
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
