//! Relight (issue #288): a light placed in the photo that lights it by depth. Near things catch it, far things
//! fall off, surfaces facing it brighten while the rest stays as it was.
//!
//! - [`Field`]: what no control changes, made once per photo from its depth map (the file's own, or a depth
//!   model's) and a guide picture of it, and kept: depth brought to the guide's size edge-aware, smoothed normals,
//!   the guide's luminance.
//! - [`Light`], [`Field::rig`], [`Field::lobe`] and [`Field::shading_of`]: up to [`MAX_LIGHTS`] lights, and what they do
//!   to each pixel of the field: a gain on scene-linear light in stops and the share of the pixel's light that is the
//!   new lights' (which takes their colours). A drag of a light makes one [`Lobe`] and one [`Shading`]; each render
//!   reads its planes ([`Shading::split`]) from the shading.
//!
//! Positions are normalized photo coordinates (before the crop and the lens corrections) plus a depth between the
//! photo's nearest and farthest, so a draft, the loupe and an export make the same picture.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod effects;
pub mod field;
pub mod filter;
pub mod kinds;
pub mod light;
mod occluder;
pub mod overlay;
pub mod probe;
mod rim;
pub mod shadow;
#[cfg(test)]
mod tests_edge;
#[cfg(test)]
mod tests_kinds;
#[cfg(test)]
mod tests_rim;
#[cfg(test)]
mod tests_shadow;

pub use effects::{Effects, HAZE_GAIN, Pattern};
pub use field::{DepthMap, Field, Kind};
pub use kinds::{Fill, LightKind, Rim, Softbox, Spot, Sun};
pub use light::{Light, Lobe, MAX_LIGHTS, Part, Planes, Rig, Shade, Shading, Split, ambient, warmth_tilt};
pub use shadow::Visibility;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The depth map can't be used (its shape, its size, its values).
    BadDepth(String),
    /// The guide picture can't be used.
    BadGuide(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadDepth(e) => write!(f, "the depth map can't be used: {e}"),
            Error::BadGuide(e) => write!(f, "the photo can't be read for its depth: {e}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
