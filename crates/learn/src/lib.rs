//! Learn My Edits: a profile learns how one photographer edits one kind of photo from 10 to 200 of their own
//! edited photos, and applies that to new photos as ordinary slider values.
//!
//! - Each photo, developed at its defaults and made small, becomes features: [`features::photo_stats`] (brightness,
//!   colour casts, hues, contrast) and, when the model is installed, the 768 numbers of the frozen image encoder
//!   ([`encoder`], DINOv2-small on candle) that say what the photo shows.
//! - Its edit becomes targets: each learned slider's change from the photo's own defaults ([`sliders`]).
//! - A small regressor per slider group maps features to targets ([`ridge`], [`train`]): kernel ridge regression,
//!   robust with few examples, with its choices made on held-out photos and its accuracy measured on others.
//! - A [`Profile`] holds all of that in one file; [`apply`] writes its prediction into a photo's settings.
//!
//! It learns sliders, never pixels, so the result is an edit like any other and is fast to apply.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
pub mod encoder;
pub mod features;
#[cfg(not(target_arch = "wasm32"))]
pub mod image_encoder;
mod linalg;
pub mod profile;
pub mod ridge;
pub mod sliders;
#[cfg(test)]
mod tests;
pub mod train;

use lightcraft_develop::DevelopSettings;

pub use features::PHOTO_DIMS;
pub use profile::{Features, Profile};
pub use ridge::Layout;
pub use sliders::{Group, SLIDERS};
pub use train::{Example, Report};

/// The encoder's id in profile files and the shared models folder.
pub const ENCODER_ID: &str = "dinov2-small";
/// Its weights file.
pub const ENCODER_FILE: &str = "model.safetensors";
/// Where it is downloaded from (pinned to one revision).
pub const ENCODER_URL: &str = "https://huggingface.co/facebook/dinov2-small/resolve/ed25f3a31f01632728cabb09d1542f84ab7b0056/model.safetensors";
/// Its size and SHA-256.
pub const ENCODER_BYTES: u64 = 88_249_960;
pub const ENCODER_SHA256: &str = "ae1e99fcefd534ed978cdeb8326f08030c96e28b7a81ffcbc98a857c84d14be1";
/// Long side of the preview features are taken from.
pub const PREVIEW_EDGE: u32 = 384;

/// Why a Learn My Edits call failed. Nothing here panics.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The encoder's weights are missing, damaged or not DINOv2-small.
    #[error("image model: {0}")]
    Model(String),
    /// A photo or example that can't be used.
    #[error("{0}")]
    Input(String),
    /// Training can't make an honest profile from these examples.
    #[error("{0}")]
    Train(String),
    /// A profile file that can't be read or written.
    #[error("profile file: {0}")]
    Profile(String),
    #[error("cancelled")]
    Cancelled,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Self::Model(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Something that turns an sRGB photo (`width × height × 3` bytes) into the encoder's numbers: the candle
/// [`encoder::Encoder`], or [`image_encoder::ImageEncoder`], which runs Core ML or candle.
pub trait ImageFeatures {
    fn features(&self, width: usize, height: usize, rgb: &[u8]) -> Result<Vec<f32>>;
}

/// The layout of features made with the encoder (`true`) or with the photo statistics only.
pub fn layout(with_encoder: bool) -> Layout {
    Layout { photo: PHOTO_DIMS, encoder: if with_encoder { encoder_dims() } else { 0 } }
}

fn encoder_dims() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        encoder::DIMS
    }
    #[cfg(target_arch = "wasm32")]
    {
        768
    }
}

/// The features of a photo developed at its defaults (`rgb`: `width × height × 3` sRGB bytes), with the encoder's
/// when one is given.
#[cfg(not(target_arch = "wasm32"))]
pub fn photo_features<E: ImageFeatures + ?Sized>(encoder: Option<&E>, width: usize, height: usize, rgb: &[u8]) -> Result<Vec<f32>> {
    let mut f = features::photo_stats(width, height, rgb)?;
    if let Some(e) = encoder {
        f.extend(e.features(width, height, rgb)?);
    }
    Ok(f)
}

/// An example from a before/after pair of files, for the Match Look fitter (#84) to feed: `before` is the
/// unedited photo's features ([`photo_features`] of the before file), `fitted` the settings the fitter found to
/// turn it into the after, relative to `defaults`. The profile learns the fitted sliders as if they were an edit.
pub fn example_from_fit(photo: &str, before: Vec<f32>, fitted: &DevelopSettings, defaults: &DevelopSettings) -> Example {
    Example { photo: photo.to_string(), features: before, targets: sliders::targets(fitted, defaults) }
}

/// `settings` with `profile`'s prediction for a photo with features `f` written in; only the sliders of the groups
/// the profile learned change. `defaults` are the photo's own defaults (with its as-shot white balance).
pub fn apply(profile: &Profile, f: &[f32], settings: &DevelopSettings, defaults: &DevelopSettings) -> Result<DevelopSettings> {
    let predicted = profile.predict(f)?;
    let mut out = settings.clone();
    sliders::write(&mut out, defaults, &predicted);
    Ok(out)
}
