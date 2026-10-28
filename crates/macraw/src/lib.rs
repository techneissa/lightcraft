//! macOS's own raw decoder (Core Image `CIRAWFilter`, the one Photos and Quick Look use) as a
//! linear base for LightCraft's "macOS Colour" profile choice.
//!
//! [`render`] decodes a raw file with a pinned recipe: linear extended Rec.2020 floats (LightCraft's
//! working space), no tone curve, no noise reduction, sharpening, contrast, detail, moiré or local
//! tone map, no gamut mapping and no lens correction, the camera's as-shot white, upright. LightCraft
//! then does tone, white balance and every slider on that picture.
//!
//! All `unsafe` (the Objective-C calls) lives in the `macos` module; the API is safe. Elsewhere
//! (Windows, Linux, wasm) everything compiles, [`available`] is false and [`render`] returns
//! [`Error::Unsupported`]. With lightcraft-sysmem and lightcraft-filepanel this is one of the three
//! crates allowed `unsafe` (AGENTS.md). It depends on no workspace crate: it returns plain `Vec<f32>`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

// The pure helpers are used only by the Core Image backend; their tests run everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod copy;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod gate;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod size;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod slots;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod stub;

#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(not(target_os = "macos"))]
use stub as imp;

pub use gate::uti_for_extension;
pub use slots::weight_bytes;

use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Bumped whenever the recipe changes what [`render`] returns (part of [`cache_key`]).
pub const RECIPE_VERSION: u32 = 1;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
static SLOTS: slots::Slots = slots::Slots::new();

/// Where the raw comes from.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// A file on disk. Core Image reads its type from the file.
    Path(&'a Path),
    /// The file's bytes, with the exact Uniform Type Identifier of its format
    /// (`com.sony.arw-raw-image`). A wrong or generic hint makes Core Image return an empty picture.
    Bytes { data: &'a [u8], uti: &'a str },
}

/// What to render.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Request {
    /// The long edge of the result in pixels; `None` = full size.
    pub long_edge: Option<u32>,
    /// Core Image's draft decode (faster, for thumbnails).
    pub draft: bool,
    /// The raw's oriented size as LightCraft reads it (its default crop). Apple's size must agree
    /// within 2 % on each edge ([`Error::SizeMismatch`] otherwise) and is resampled to exactly this
    /// size (scaled to `long_edge`), so normalised edit coordinates map one to one. `None`: no gate.
    pub expected_size: Option<(u32, u32)>,
}

/// A rendered base: linear extended Rec.2020, upright.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    /// Three floats per pixel, row-major, ≥ 0 and finite (values above 1 are kept).
    pub rgb: Vec<f32>,
    /// The camera's as-shot white, CIE 1931 xy (`neutralChromaticity`).
    pub white_xy: [f64; 2],
    /// Core Image's own reading of that white, in Kelvin and tint.
    pub white_kelvin: f32,
    pub white_tint: f32,
    /// Apple's full-size upright size before any scaling.
    pub native_size: (u32, u32),
    /// Core Image's decoder version used (`"8"` on macOS 27.0.1).
    pub decoder_version: String,
}

/// What [`probe`] reads without decoding the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// Apple's full-size upright size.
    pub native_size: (u32, u32),
}

/// Why a render or probe failed. Every variant has a message a user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a Mac, macOS older than 12, or a Core Image setting the recipe needs is missing.
    Unsupported,
    /// macOS could not read this raw (unknown camera, corrupt data, or a wrong type hint).
    Undecodable,
    /// macOS reads this raw at a size more than 2 % away from LightCraft's.
    SizeMismatch {
        apple: (u32, u32),
        expected: (u32, u32),
    },
    /// The picture would exceed the pixel cap.
    TooLarge {
        width: u32,
        height: u32,
    },
    /// macOS needs to download support for this camera first.
    NeedsDownload {
        camera: String,
    },
    Cancelled,
    /// Core Image raised an error.
    Os(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported => f.write_str("macOS Colour needs macOS 12 or later on a Mac"),
            Error::Undecodable => f.write_str("macOS could not read this raw"),
            Error::SizeMismatch { apple, expected } => {
                write!(f, "macOS reads this raw at {}×{}, LightCraft at {}×{}", apple.0, apple.1, expected.0, expected.1)
            }
            Error::TooLarge { width, height } => write!(f, "the picture is too large ({width}×{height})"),
            Error::NeedsDownload { camera } => write!(f, "macOS needs to download support for the {camera}"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Os(msg) => write!(f, "macOS raw decoder: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// True when this machine can render the recipe: a Mac on macOS 12 or later whose Core Image has
/// every required `CIRAWFilter` setting.
pub fn available() -> bool {
    imp::available()
}

/// True when macOS lists a camera with this EXIF model (`ILCE-7M5`, not LightCraft's joined
/// `SONY ILCE-7M5`). Necessary, not sufficient: the size gate and the decode still decide.
pub fn supports(exif_model: &str) -> bool {
    imp::supports(exif_model)
}

/// Read Apple's upright size without decoding the picture.
pub fn probe(input: Input<'_>) -> Result<Probe, Error> {
    imp::probe(input)
}

/// Render the base. Checks `cancel` before and after the (uninterruptible) Core Image call.
pub fn render(input: Input<'_>, request: &Request, cancel: &AtomicBool) -> Result<LinearImage, Error> {
    imp::render(input, request, cancel)
}

/// Changes when the result of [`render`] may change: the OS build (which ships Core Image's raw
/// decoder) and [`RECIPE_VERSION`]. 0 where [`available`] is false. The decoder version a render used
/// is in [`LinearImage::decoder_version`].
pub fn cache_key() -> u64 {
    imp::cache_key()
}

/// FNV-1a over the OS build and the recipe version: stable across Rust releases, unlike std's hasher.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn key_of(os_build: &str, recipe: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in os_build.bytes().chain([0]).chain(recipe.to_le_bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h.max(1)
}

/// The OS version and build ("Version 27.0.1 (Build 27A…)"); empty off the Mac.
pub fn os_build() -> String {
    imp::os_build()
}

#[cfg(test)]
mod tests;
