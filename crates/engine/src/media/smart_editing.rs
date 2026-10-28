//! Use Smart Previews instead of Originals for editing (Lightroom's Preferences ▸ Performance option).
//!
//! A smart preview is the photo's decoded source at the loupe's size (the [`SourceLevel::Preview`] pixels), kept in
//! the library ([`crate::smart`]). With the setting on, the thumbnail and preview levels are read from it when the
//! photo has a *current* one, so photos on a network or an external drive show and edit without touching the original.
//! The [`SourceLevel::Full`] level (zoom to 100 % and more), exports and the other jobs that need full pixels still read
//! the original.
//!
//! The render stays the same photo and the same edit: the proxy holds the pixels the original's decoder made (JPEG
//! quality 97, 8-bit sRGB) and the decoder facts the settings are read against, so only that rounding differs; the
//! test `a_render_from_the_smart_preview_matches_the_original` measures it. A proxy that could differ in colour is not
//! used: a stale one (an older camera look, encoding or local profile), one made for another colour than the photo
//! asks for (`No Camera Matching`, a local profile, macOS Colour), and a photo whose AI Denoise picture is on.
//!
//! The UI thread never touches the file system: [`MediaCache::choose_smart`] only wraps the original's source in
//! [`SourceRef::Preferred`]; the render worker checks the proxy and falls back to the original when it is missing or
//! out of date.

use std::path::PathBuf;

use super::{Arc, DecodedSource, LoadOptions, MediaCache, Photo, PhotoId, ReducedLoader, Rgb32f, Source, SourceLevel, SourceRef};
use crate::smart::LocalWant;

/// The setting's state in [`MediaCache`].
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    enabled: bool,
}

/// A photo's smart preview as an editing source: where it may be, and what it must say to be current.
#[derive(Clone, Debug)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct Proxy {
    /// The sharded location, then the pre-upgrade flat one (`smart::file_path`).
    paths: [PathBuf; 2],
    /// The photo takes the per-file camera look (a stale look makes the proxy out of date).
    look: bool,
    /// The local profile / macOS Colour the proxy must have been made with.
    want: LocalWant,
}

impl Proxy {
    /// The file to read, when one is there and current. Checks the file system: only for worker threads.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn current(&self) -> Option<&PathBuf> {
        self.paths.iter().find(|p| crate::smart::current_for(p, self.look, &self.want))
    }
}

/// Whether `p`'s proxy could show a different picture than its original would for `wanted`, so the original is used.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn proxy_would_differ(p: &Photo, wanted: LoadOptions) -> bool {
    if !p.fitted_look() {
        return false;
    }
    // the proxy has the colour of the photo's own Profile choice: No Camera Matching (and the profile browser's
    // variants of other choices) read the original
    wanted.colour == lightcraft_develop::CameraColour::Colorimetric || wanted != LoadOptions::of(&p.develop)
}

impl MediaCache {
    /// Whether the setting is on.
    pub fn smart_editing(&self) -> bool {
        self.smart_edit.enabled
    }

    /// Turn the setting on or off; `true` when it changed. Off forgets the sources decoded from proxies, so the next
    /// render reads the originals again.
    pub fn set_smart_editing(&mut self, on: bool) -> bool {
        if self.smart_edit.enabled == on {
            return false;
        }
        self.smart_edit.enabled = on;
        if !on {
            self.clear_sources();
        }
        true
    }

    /// The proxy to edit `p` from at `level`, when the setting and the photo allow it (the worker still checks the file).
    #[cfg(target_arch = "wasm32")]
    fn smart_proxy(&self, _: &Photo, _: SourceLevel, _: LoadOptions) -> Option<Proxy> {
        None
    }

    /// The proxy to edit `p` from at `level`, when the setting and the photo allow it (the worker still checks the file).
    #[cfg(not(target_arch = "wasm32"))]
    fn smart_proxy(&self, p: &Photo, level: SourceLevel, wanted: LoadOptions) -> Option<Proxy> {
        use lightcraft_develop::CameraColour;
        if !self.smart_edit.enabled || level == SourceLevel::Full || proxy_would_differ(p, wanted) {
            return None;
        }
        let (Source::File { .. }, Some(dir)) = (&p.source, &self.smart_dir) else { return None };
        // an AI Denoise picture is read from the raw
        if p.develop.denoise_amount() > 0.0 && self.denoise.spec(p.id).is_some() {
            return None;
        }
        let macos_here = p.develop.profile.camera_colour == CameraColour::Macos && crate::files::macos_lists(p);
        let (_, want) = crate::cmd::previews::smart_colour(p, macos_here);
        // a colour the proxy can't be checked against (a missing local profile; macOS Colour this computer can't make)
        if matches!(want, LocalWant::Keep | LocalWant::MacosElsewhere) {
            return None;
        }
        let sharded = crate::smart::sharded_file_path(dir, p);
        let legacy = dir.join(crate::smart::file_name(p));
        Some(Proxy { paths: [sharded, legacy], look: crate::cmd::previews::preview_fitted_look(p), want })
    }

    /// Whether a source kept in memory for `p` must not be used now: it was read from a smart preview, and the setting
    /// is off or the photo's colour has become one the proxy can't show.
    pub(super) fn smart_source_vetoed(&self, r: &SourceRef, p: &Photo, level: SourceLevel, wanted: LoadOptions) -> bool {
        r.is_edit_proxy() && self.smart_proxy(p, level, wanted).is_none()
    }

    /// `r` (the original's source) with the photo's smart preview preferred, when the setting is on and `cached` (a job
    /// that must have the original passes `false`: exports, [`crate::Session::original_job`]).
    pub(super) fn choose_smart(&self, p: &Photo, level: SourceLevel, cached: bool, wanted: LoadOptions, r: SourceRef) -> SourceRef {
        let SourceRef::File { max_edge, .. } = &r else { return r };
        if !cached {
            return r;
        }
        match self.smart_proxy(p, level, wanted) {
            Some(proxy) => SourceRef::Preferred { max_edge: *max_edge, options: wanted, proxy: Box::new(proxy), otherwise: Box::new(r) },
            None => r,
        }
    }

    /// Whether the decoded source of `id` the loupe or the grid holds was read from its smart preview, with the setting on
    /// and the original not known to be offline (the Info panel's "Editing from Smart Preview").
    pub fn editing_from_smart_preview(&self, id: PhotoId, p: &Photo) -> bool {
        if !self.smart_edit.enabled {
            return false;
        }
        let held = self.previews.iter().filter(|e| e.0 == id).map(|e| &e.1).chain(self.thumbs.peek(&id)).next();
        held.is_some_and(|s| s.edit_proxy) && !matches!(&p.source, Source::File { path } if self.availability.is_offline(path))
    }
}

impl SourceRef {
    /// The thumbnail loader slot of a file source, whether or not a smart preview is preferred over it.
    pub fn reduced_mut(&mut self) -> Option<&mut Option<ReducedLoader>> {
        match self {
            SourceRef::File { reduced, .. } => Some(reduced),
            SourceRef::Preferred { otherwise, .. } => otherwise.reduced_mut(),
            _ => None,
        }
    }

    /// The source that reads the original: `self` without a preferred smart preview.
    pub fn original(self) -> SourceRef {
        match self {
            SourceRef::Preferred { otherwise, .. } => otherwise.original(),
            r => r,
        }
    }

    /// Whether this source reads a smart preview by choice (not because the original is offline).
    fn is_edit_proxy(&self) -> bool {
        match self {
            SourceRef::Preferred { .. } => true,
            SourceRef::Loaded(d) => d.edit_proxy,
            SourceRef::Fit { from, .. } => from.edit_proxy,
            _ => false,
        }
    }
}

/// Load the preferred proxy (resized to `max_edge`), else the original.
pub(super) fn load_preferred(proxy: &Proxy, options: LoadOptions, max_edge: usize, otherwise: &SourceRef) -> Result<DecodedSource, String> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = proxy.current() {
        match crate::smart::load_for_editing(path) {
            Ok(d) => return Ok(fit(DecodedSource { options, ..d }, max_edge)),
            // (a proxy that can't be read is the original's business now; Build Smart Previews repairs it)
            Err(e) => log::warn!("smart preview not used for editing: {e}"),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (proxy, options, max_edge);
    otherwise.load_source()
}

/// `d` shrunk to at most `max_edge` pixels long (the thumbnail level is cut from the preview-size proxy).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn fit(d: DecodedSource, max_edge: usize) -> DecodedSource {
    if d.image.width.max(d.image.height) <= max_edge {
        return d;
    }
    // Lanczos, as `SourceRef::Fit` does
    let image: Rgb32f = lightcraft_raster::resample::fit(&d.image, max_edge, max_edge, lightcraft_raster::resample::Filter::Lanczos3);
    DecodedSource { image: Arc::new(image), ..d }
}

impl crate::Session {
    /// `job` made to read the original: the output jobs (export, Edit In, contact sheet, print, slideshow) and the ones
    /// that need the photo's pixels (Find Dust Spots) never take a smart preview's.
    pub fn original_job(&mut self, job: &mut super::RenderJob) {
        if !job.source.is_edit_proxy() {
            return;
        }
        if let Some(p) = self.catalog.photo(job.photo).cloned() {
            job.source = self.media.source_ref_uncached(&p, job.level);
        }
    }
}

#[cfg(test)]
mod tests;
