//! macOS Colour: a raw's picture made by the Mac's own raw decoder (`lightcraft_macraw`, Core Image's `CIRAWFilter`)
//! instead of LightCraft's demosaic, white balance and camera matrix. LightCraft keeps tone, white balance and every
//! slider (`docs/camera-preview-colour.md`, "macOS Colour").
//!
//! The base is linear Rec.2020 at the camera's as-shot white, upright, at LightCraft's own size (the decoder's size
//! gate). Whenever it cannot be made (not a Mac, a camera macOS does not list, another size, an unreadable file) the
//! photo is decoded as Camera Matching and its source says why ([`MacosUnavailable`]); the setting is kept.
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, PoisonError};

use lightcraft_color::Xy;
use lightcraft_macraw::{Error, Input, LinearImage, Request};
use lightcraft_pipeline::MacosUnavailable;
use lightcraft_raw::{RawFormat, RawInfo};

use super::{Rgb32f, SourceInfo, xy_to_temp_tint};

/// The base's exposure scale, in stops, for LightCraft's default tone curve (a refused tone fit, [`toned`]): Core
/// Image's linear output with the pinned recipe is darker than the camera JPEG. Measured on the 16 A7 V photos of the
/// first calibration (2026-10-28, macOS 27.0.1, 1600 px, lens corrections off): at +0.44 stops the mean lightness
/// against the camera JPEG was +1.03 L* (σ 2.71); at the first calibration's 9.4 L* a stop that puts zero at +0.33,
/// which measured −0.06 L* (σ 2.86).
pub(crate) const GAIN_STOPS: f32 = 0.33;

/// The Mac's raw decoder, or a stand-in in tests.
pub(crate) trait Backend: Sync {
    fn available(&self) -> bool;
    fn supports(&self, exif_model: &str) -> bool;
    fn render(&self, input: Input<'_>, request: &Request, cancel: &AtomicBool) -> Result<LinearImage, Error>;
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

/// A decoded base: the picture (linear Rec.2020, upright, gain not applied) and the camera's as-shot white.
pub(super) struct Base {
    pub image: Rgb32f,
    pub white: Xy,
}

/// The macOS base of the raw in `bytes` (header `info`), no larger than `max_edge`; why not, when it cannot be made.
/// A failure of this file is remembered for the session.
pub(super) fn base(bytes: &[u8], info: &RawInfo, max_edge: usize, backend: &dyn Backend) -> Result<Base, MacosUnavailable> {
    let uti = gate(bytes, info, backend)?;
    let expected = expected_size(info).ok_or(MacosUnavailable::Unreadable)?;
    let cancel = AtomicBool::new(false);
    let rendered = backend.render(Input::Bytes { data: bytes, uti }, &request(expected, max_edge), &cancel);
    // Core Image's buffers are gone (the render's autorelease pool ended): give their pages back
    crate::memory::release();
    let why = match rendered {
        Ok(img) => match into_base(img) {
            Some(base) => return Ok(base),
            None => MacosUnavailable::Unreadable,
        },
        Err(Error::Unsupported) => return Err(MacosUnavailable::System),
        Err(Error::Cancelled) => return Err(MacosUnavailable::Unreadable),
        Err(Error::SizeMismatch { .. }) => MacosUnavailable::Size,
        Err(Error::Undecodable | Error::TooLarge { .. } | Error::NeedsDownload { .. } | Error::Os(_)) => MacosUnavailable::Unreadable,
    };
    remember(fingerprint(bytes), why);
    Err(why)
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
/// Kelvin, never the relative scale), no camera colour model (white balance adapts the linear base, Bradford).
pub(super) fn source_info(base: &Base, lens: Option<lightcraft_develop::EmbeddedLens>) -> SourceInfo {
    let (temp, tint) = xy_to_temp_tint(base.white);
    SourceInfo {
        raw: true,
        as_shot_temp: temp.round(),
        as_shot_tint: tint.round(),
        lens,
        relative_wb: false,
        legacy_relative_wb: false,
        camera_color: None,
        camera_tone: None,
        colorimetric: true,
        macos_offered: true,
        ..SourceInfo::default()
    }
}

#[cfg(test)]
pub(crate) mod tests;
