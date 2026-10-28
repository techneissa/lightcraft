//! macOS Colour: a raw's picture made by the Mac's own raw decoder (`lightcraft_macraw`, Core Image's `CIRAWFilter`)
//! instead of LightCraft's demosaic, white balance and camera matrix. LightCraft keeps tone, white balance and every
//! slider (`docs/camera-preview-colour.md`, "macOS Colour").
//!
//! The base is linear Rec.2020 at the camera's as-shot white, upright, at LightCraft's own size (the decoder's size
//! gate). Whenever it cannot be made (not a Mac, a camera macOS does not list, another size, an unreadable file) the
//! photo is decoded as Camera Matching and its source says why ([`MacosUnavailable`]); the setting is kept.
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_color::Xy;
use lightcraft_macraw::{Error, Input, LinearImage, Request};
use lightcraft_pipeline::MacosUnavailable;
use lightcraft_raw::{RawFormat, RawInfo};

use super::{Rgb32f, SourceInfo, xy_to_temp_tint};
use crate::camera_preview::colour_transfer::{self, Refused, Transfer};

/// The base's exposure scale, in stops, for LightCraft's default tone curve (a refused tone fit, [`toned`]): Core
/// Image's linear output with the pinned recipe is darker than the camera JPEG. Measured on the 16 A7 V photos of the
/// first calibration (2026-10-28, macOS 27.0.1, 1600 px, lens corrections off): at +0.44 stops the mean lightness
/// against the camera JPEG was +1.03 L* (σ 2.71); at the first calibration's 9.4 L* a stop that puts zero at +0.33,
/// which measured −0.06 L* (σ 2.86).
pub(crate) const GAIN_STOPS: f32 = 0.33;

/// Never set: the cancel flag of a load that has none.
static NEVER: AtomicBool = AtomicBool::new(false);

/// The Mac's raw decoder, or a stand-in in tests.
pub(crate) trait Backend: Sync {
    fn available(&self) -> bool;
    fn supports(&self, exif_model: &str) -> bool;
    fn render(&self, input: Input<'_>, request: &Request, cancel: &AtomicBool) -> Result<LinearImage, Error>;
    /// The file the load reads, when it has one: the decoder then reads it from disk and the load lets its bytes go
    /// first (no copy of the raw is held while Core Image works).
    fn path(&self) -> Option<&Path> {
        None
    }
    /// The flag that cancels the load's render, also while it waits for a render slot (the render job's, else never
    /// set).
    fn cancel(&self) -> &AtomicBool {
        &NEVER
    }
}

/// [`lightcraft_macraw`] itself.
pub(crate) struct System;

impl Backend for System {
    fn available(&self) -> bool {
        lightcraft_macraw::available()
    }
    fn supports(&self, exif_model: &str) -> bool {
        lightcraft_macraw::supports(exif_model)
    }
    fn render(&self, input: Input<'_>, request: &Request, cancel: &AtomicBool) -> Result<LinearImage, Error> {
        lightcraft_macraw::render(input, request, cancel)
    }
}

/// [`System`] for one file load: its path and the cancel flag of the job it runs in ([`with_job_cancel`]).
pub(crate) struct FileJob<'a> {
    path: &'a Path,
    cancel: Option<Arc<AtomicBool>>,
}

impl<'a> FileJob<'a> {
    /// A load of `path` on this thread: it takes the cancel flag of the job around it, if any.
    pub(crate) fn current(path: &'a Path) -> Self {
        FileJob { path, cancel: JOB_CANCEL.with(|c| c.borrow().clone()) }
    }
}

impl Backend for FileJob<'_> {
    fn available(&self) -> bool {
        System.available()
    }
    fn supports(&self, exif_model: &str) -> bool {
        System.supports(exif_model)
    }
    fn render(&self, input: Input<'_>, request: &Request, cancel: &AtomicBool) -> Result<LinearImage, Error> {
        System.render(input, request, cancel)
    }
    fn path(&self) -> Option<&Path> {
        Some(self.path)
    }
    fn cancel(&self) -> &AtomicBool {
        self.cancel.as_deref().unwrap_or(&NEVER)
    }
}

thread_local! {
    static JOB_CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Run `f` (a job's renders) with `cancel` as the flag that stops a macOS Colour render it starts on this thread,
/// while that render waits for a slot or before it starts (Core Image's own call cannot be interrupted). Building
/// previews passes its row's flag, so ✕ in the activity stack stops the photos still waiting.
pub(crate) fn with_job_cancel<R>(cancel: &Arc<AtomicBool>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let was = self.0.take();
            JOB_CANCEL.with(|c| *c.borrow_mut() = was);
        }
    }
    let _restore = Restore(JOB_CANCEL.with(|c| c.replace(Some(cancel.clone()))));
    f()
}

/// Files the decoder failed on this session (by [`fingerprint`]), so a failing photo is not rendered again for every
/// preview level, thumbnail and frame. Never cleared: a new OS build means a new session.
static FAILED: Mutex<Option<HashMap<u64, MacosUnavailable>>> = Mutex::new(None);

fn failed(key: u64) -> Option<MacosUnavailable> {
    FAILED.lock().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(|m| m.get(&key).copied())
}

fn remember(key: u64, why: MacosUnavailable) {
    FAILED.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(HashMap::new).insert(key, why);
}

/// FNV-1a over the file's length and its first 64 KiB (headers, maker notes, the model): different raws differ there.
fn fingerprint(bytes: &[u8]) -> u64 {
    let head = bytes.get(..65_536).unwrap_or(bytes);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in (bytes.len() as u64).to_le_bytes().iter().chain(head) {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The decoder's type for a raw format, for a bytes input (only formats verified to decode with it).
fn uti(format: RawFormat) -> Option<&'static str> {
    match format {
        RawFormat::Arw => lightcraft_macraw::uti_for_extension("arw"),
        _ => None,
    }
}

/// Whether macOS Colour can be offered for this raw on this computer (`SourceInfo::macos_offered`): its Profile option
/// is offered, the decoder is there, lists its camera, and has not failed on this file.
pub(super) fn offered(bytes: &[u8], info: &RawInfo, backend: &dyn Backend) -> bool {
    gate(bytes, info, backend).is_ok()
}

/// Whether this computer's decoder would make photo `p`'s picture, from the catalog alone: an ARW whose camera macOS
/// lists (the catalog joins make and model, `SONY ILCE-7M5`; macOS lists the EXIF model). The gates at load decide.
pub(super) fn lists(p: &lightcraft_catalog::Photo) -> bool {
    let camera = p.meta.camera.trim();
    let model = camera.split_once(' ').map_or(camera, |(_, model)| model.trim());
    p.format.eq_ignore_ascii_case("ARW")
        && lightcraft_macraw::available()
        && (lightcraft_macraw::supports(model) || lightcraft_macraw::supports(camera))
}

/// The name checks, before anything is rendered.
fn gate(bytes: &[u8], info: &RawInfo, backend: &dyn Backend) -> Result<&'static str, MacosUnavailable> {
    let offered = !lightcraft_raw::color::has_matrix(&info.color) && crate::camera_preview::file_local_look(info.format);
    if !offered {
        return Err(MacosUnavailable::Camera);
    }
    if !backend.available() {
        return Err(MacosUnavailable::System);
    }
    let uti = uti(info.format).ok_or(MacosUnavailable::Camera)?;
    let model = info.metadata.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).ok_or(MacosUnavailable::Camera)?;
    if !backend.supports(model) {
        return Err(MacosUnavailable::Camera);
    }
    match failed(fingerprint(bytes)) {
        Some(why) => Err(why),
        None => Ok(uti),
    }
}

/// LightCraft's size of the developed raw, upright: what the decoder's picture must match.
pub(super) fn expected_size(info: &RawInfo) -> Option<(u32, u32)> {
    let (w, h) = info.developed_size();
    let (w, h) = if info.orientation.swaps_axes() { (h, w) } else { (w, h) };
    Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?)).filter(|(w, h)| *w > 0 && *h > 0)
}

/// The request for a source no longer than `max_edge`: full size when that covers the picture, the high-quality
/// downsample for thumbnails.
fn request(expected: (u32, u32), max_edge: usize) -> Request {
    let long = expected.0.max(expected.1);
    let long_edge = u32::try_from(max_edge).ok().filter(|&e| e < long);
    let draft = max_edge <= crate::media::SourceLevel::Thumb.max_edge();
    Request { long_edge, draft, expected_size: Some(expected) }
}

/// The memory gate weight of a macOS Colour load of the raw in `bytes` no larger than `max_edge`: Core Image's peak
/// footprint for the picture it will draw (`lightcraft_macraw::weight_bytes`, 1 GB at any size up to the A7 V's full
/// 33 MP, more for larger sensors); the floor when the header cannot be read.
pub(crate) fn gate_weight(bytes: &[u8], max_edge: usize) -> usize {
    let size = lightcraft_raw::probe_info(bytes).ok().as_ref().and_then(expected_size).map(|expected| {
        let long = expected.0.max(expected.1);
        match u32::try_from(max_edge).ok().filter(|&e| e < long) {
            Some(edge) => {
                let s = f64::from(edge) / f64::from(long);
                let px = |v: u32| (f64::from(v) * s).round() as u32;
                (px(expected.0), px(expected.1))
            }
            None => expected,
        }
    });
    let weight = size.map_or(lightcraft_macraw::FLOOR_BYTES, |(w, h)| lightcraft_macraw::weight_bytes(w, h));
    usize::try_from(weight).unwrap_or(usize::MAX)
}

/// A decoded base: the picture (linear Rec.2020, upright, gain not applied) and the camera's as-shot white.
pub(super) struct Base {
    pub image: Rgb32f,
    pub white: Xy,
}

/// The macOS base of the raw in `bytes` (header `info`), no larger than `max_edge`; why not, when it cannot be made.
/// A failure of this file is remembered for the session.
pub(super) fn base(bytes: &[u8], info: &RawInfo, max_edge: usize, backend: &dyn Backend) -> Result<Base, MacosUnavailable> {
    let ready = prepare(bytes, info, backend)?;
    render_base(&ready, Input::Bytes { data: bytes, uti: ready.uti }, max_edge, backend)
}

/// A raw that passed the name checks ([`prepare`]): what [`render_base`] needs once its bytes may be gone.
pub(super) struct Prepared {
    uti: &'static str,
    expected: (u32, u32),
    fingerprint: u64,
}

impl Prepared {
    /// The decoder's type for the raw's bytes.
    pub(super) fn uti(&self) -> &'static str {
        self.uti
    }
}

/// The checks before the decoder runs: the name gates, LightCraft's size, and the file's fingerprint.
pub(super) fn prepare(bytes: &[u8], info: &RawInfo, backend: &dyn Backend) -> Result<Prepared, MacosUnavailable> {
    let uti = gate(bytes, info, backend)?;
    let expected = expected_size(info).ok_or(MacosUnavailable::Unreadable)?;
    Ok(Prepared { uti, expected, fingerprint: fingerprint(bytes) })
}

/// The decoder's base of a [`prepare`]d raw read from `input` (its bytes or its path), cancelled by the backend's
/// flag. A failure is remembered for the session; a cancelled render is not (the photo is tried again next time).
pub(super) fn render_base(ready: &Prepared, input: Input<'_>, max_edge: usize, backend: &dyn Backend) -> Result<Base, MacosUnavailable> {
    let rendered = backend.render(input, &request(ready.expected, max_edge), backend.cancel());
    // Core Image's buffers are gone (the render's autorelease pool ended): give their pages back
    crate::memory::release();
    let why = match rendered {
        Ok(img) => match into_base(img) {
            Some(base) => return Ok(base),
            None => MacosUnavailable::Unreadable,
        },
        Err(Error::Unsupported) => return Err(MacosUnavailable::System),
        // (a cancelled render, and a camera macOS must first download support for, are tried again next time)
        Err(Error::Cancelled | Error::NeedsDownload { .. }) => return Err(MacosUnavailable::Unreadable),
        Err(Error::SizeMismatch { .. }) => MacosUnavailable::Size,
        Err(Error::Undecodable | Error::TooLarge { .. } | Error::Os(_)) => MacosUnavailable::Unreadable,
    };
    remember(ready.fingerprint, why);
    Err(why)
}

/// Whether the load's render was cancelled (the job's flag is set).
pub(super) fn cancelled(backend: &dyn Backend) -> bool {
    backend.cancel().load(Ordering::Relaxed)
}

/// The bytes of `path` again, after a render from the path failed and the Camera Matching decode needs them.
pub(super) fn reread(path: &Path) -> Result<Cow<'static, [u8]>, String> {
    std::fs::read(path).map(Cow::Owned).map_err(|e| format!("{}: {e}", path.display()))
}

/// The decoder's picture as LightCraft's image; `None` when it is empty, the wrong length or its white is not a colour.
fn into_base(img: LinearImage) -> Option<Base> {
    let (width, height) = (usize::try_from(img.width).ok()?, usize::try_from(img.height).ok()?);
    let [x, y] = img.white_xy;
    let usable = width > 0 && height > 0 && img.rgb.len() == width.checked_mul(height)? && x.is_finite() && y.is_finite() && x > 0.0 && y > 0.0;
    usable.then(|| Base { image: Rgb32f { width, height, data: img.rgb }, white: Xy::new(x, y) })
}

/// The base and its tone: the curve fitted to the camera JPEG (`camera_preview::fit_base_tone`) on the base as the
/// decoder made it, which maps this photo's own scene values, so nothing is scaled; when the fit is refused (no camera
/// JPEG, another frame, the gates), LightCraft's default tone on the base scaled by [`GAIN_STOPS`]. One scale, never
/// two (plan §3.2: on the 68-photo sample the fit on the unscaled base was accepted for 64, on the scaled one for 63,
/// with the same colour against the camera JPEG and against macOS's own render).
pub(super) fn toned(mut img: Rgb32f, look: Option<crate::camera_preview::CameraLook>) -> (Rgb32f, Option<lightcraft_pipeline::tone::CameraTone>) {
    match look {
        Some(look) => (img, Some(look.tone)),
        None => {
            apply_gain(&mut img, GAIN_STOPS);
            (img, None)
        }
    }
}

/// Scale the base by `stops` (a refused tone fit's fixed gain), keeping it finite and non-negative.
fn apply_gain(img: &mut Rgb32f, stops: f32) {
    let gain = 2f32.powf(stops);
    for px in &mut img.data {
        for v in px {
            *v = if v.is_finite() { (*v * gain).max(0.0) } else { 0.0 };
        }
    }
}

/// The source facts of a macOS base: the as-shot white on the Kelvin scale from the decoder's white (the slider reads
/// Kelvin, never the relative scale), no camera colour model (white balance adapts the linear base, Bradford). A white
/// balance still written on the relative scale (set on Camera Matching) renders as the Kelvin it composes to from
/// macOS's as-shot white (`legacy_relative_wb`, `pipeline::local::legacy_to_kelvin`), so As Shot stays as shot and a
/// Custom white keeps its shift, with nothing rewritten.
pub(super) fn source_info(base: &Base, lens: Option<lightcraft_develop::EmbeddedLens>) -> SourceInfo {
    let (temp, tint) = xy_to_temp_tint(base.white);
    SourceInfo {
        raw: true,
        as_shot_temp: temp.round(),
        as_shot_tint: tint.round(),
        lens,
        relative_wb: false,
        legacy_relative_wb: true,
        camera_color: None,
        camera_tone: None,
        colorimetric: true,
        macos_offered: true,
        ..SourceInfo::default()
    }
}

/// The long edge of macOS's picture the AI Denoise colour transfer is fitted to (plan §6.6: about 0.5 s and at most
/// 0.3 GB, rendered before LightCraft's develop so the two never hold memory together; never a full-size render).
pub(super) const TRANSFER_EDGE: usize = 1024;

/// What a macOS Colour photo with AI Denoise needs before LightCraft develops it: macOS's small picture (the
/// transfer's target, and its white) and the camera JPEG the tone is fitted to.
pub(super) struct TransferTarget {
    base: Base,
    reference: Option<crate::camera_preview::BaseReference>,
}

/// The [`TransferTarget`] of the raw in `bytes`; why not, when macOS cannot make its picture.
pub(super) fn transfer_target(bytes: &[u8], backend: &dyn Backend) -> Result<TransferTarget, MacosUnavailable> {
    let info = lightcraft_raw::probe_info(bytes).map_err(|_| MacosUnavailable::Unreadable)?;
    let reference = crate::camera_preview::base_reference(bytes);
    let base = base(bytes, &info, TRANSFER_EDGE, backend)?;
    Ok(TransferTarget { base, reference })
}

/// The transfer from LightCraft's developed camera RGB `camera` (sensor orientation `orientation`, before highlight
/// reconstruction and white balance; `mask` its binned clip mask) white-balanced by `wb`, to macOS's colours: both
/// block-averaged to one upright grid and fitted (`camera_preview::colour_transfer`).
pub(super) fn transfer_for(
    target: &TransferTarget,
    camera: &Rgb32f,
    mask: Option<&[u8]>,
    wb: [f32; 3],
    orientation: lightcraft_geom::Orientation,
) -> Result<Transfer, Refused> {
    let apple = &target.base.image;
    let swaps = orientation.swaps_axes();
    let (w, h) = if swaps { (camera.height, camera.width) } else { (camera.width, camera.height) };
    let (cols, rows) = colour_transfer::grid(w.min(apple.width), h.min(apple.height));
    let (sensor_cols, sensor_rows) = if swaps { (rows, cols) } else { (cols, rows) };
    let blocks = colour_transfer::camera_blocks(camera, mask, wb, sensor_cols, sensor_rows).into_oriented(orientation);
    colour_transfer::fit_transfer(&blocks, &colour_transfer::apple_blocks(apple, cols, rows))
}

/// The source of a macOS Colour photo with AI Denoise, from its `plain` and denoised (`twin`) pictures already through
/// the transfer (upright): the tone fitted to the camera JPEG on the plain picture, the same for both (the fixed gain
/// on both when the fit is refused), and the facts of a macOS base (Kelvin from macOS's white).
pub(super) fn transferred(
    target: TransferTarget,
    plain: Rgb32f,
    twin: Option<Rgb32f>,
    lens: Option<lightcraft_develop::EmbeddedLens>,
    orientation: lightcraft_geom::Orientation,
) -> (Rgb32f, Option<Rgb32f>, SourceInfo) {
    let look = target.reference.as_ref().and_then(|r| crate::camera_preview::fit_base_tone(r, orientation, &plain));
    let (plain, camera_tone) = toned(plain, look);
    let twin = twin.map(|t| if camera_tone.is_some() { t } else { toned(t, None).0 });
    let info = source_info(&target.base, lens);
    (plain, twin, SourceInfo { camera_tone, ..info })
}

#[cfg(test)]
mod file_tests;
#[cfg(test)]
mod preset_tests;
#[cfg(all(test, target_os = "macos"))]
mod real_tests;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod transfer_tests;
