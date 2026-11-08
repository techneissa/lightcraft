//! People parsing for LightCraft: turns a photo into people, each with a face box, 478 face landmarks and part masks
//! (Entire Person, Facial and Body Skin, Hair, Eyebrows, Eye Whites, Iris, Lips, Teeth, Clothes Top / Bottom / Dress
//! / Shoes / Hat / Bag, Others), for the portrait features and the Select People masks.
//!
//! Three model sets parse ([`ModelSet`]): SegFormer-B0 faces with SegFormer-B3 bodies, BiSeNet faces with
//! SegFormer-B2 bodies, or the SegFormer set with Sapiens2 deciding where skin, hair and clothes are; all use
//! MediaPipe's face mesh v2. The weights are not part of LightCraft: they are downloaded
//! into the models folder ([`models`]) and read from there ([`Parser::load`]). On macOS the models run on the GPU
//! through Metal, elsewhere on the CPU. No UI dependencies (L3).
//!
//! Masks are resolution independent: each is a [`Plane`] over a rectangle given in fractions of the photo, at the
//! resolution the model saw it; [`render`] samples it at any size and refines its edges along the photo's own edges
//! (a guided filter whose radius is a fraction of the picture), so a preview and a 24 MP export agree. [`cache`]
//! stores a parse on disk by the pixels it was made from and the model set's version.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cache;
pub mod image;
pub mod models;
pub mod render;
mod types;

#[cfg(not(target_arch = "wasm32"))]
mod bisenet;
#[cfg(not(target_arch = "wasm32"))]
mod body;
#[cfg(not(target_arch = "wasm32"))]
mod crops;
#[cfg(not(target_arch = "wasm32"))]
mod eyes;
#[cfg(not(target_arch = "wasm32"))]
mod face;
#[cfg(not(target_arch = "wasm32"))]
mod facemesh;
#[cfg(not(target_arch = "wasm32"))]
mod fuse;
#[cfg(not(target_arch = "wasm32"))]
mod nn;
#[cfg(not(target_arch = "wasm32"))]
mod parser;
#[cfg(not(target_arch = "wasm32"))]
mod people;
#[cfg(not(target_arch = "wasm32"))]
mod sapiens;
#[cfg(not(target_arch = "wasm32"))]
mod segformer;
#[cfg(not(target_arch = "wasm32"))]
mod weights;

pub use image::RgbImage;
pub use models::{ModelFile, ModelSet};
#[cfg(not(target_arch = "wasm32"))]
pub use parser::{Options, Parser, best_device};
pub use types::{FaceHint, Landmark, NormRect, Parsing, Part, Person, Plane};

/// The version of how parts are made from the models' outputs; part of every cached parse's key.
pub const PARSE_VERSION: u32 = 1;
/// A photo whose long side is shorter than this has no people to find (the parse is empty).
pub const MIN_SIDE: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A model file is not in the models folder.
    #[error("people parsing needs the {0}: download the model set first")]
    Missing(String),
    /// A model file is damaged or not the model it should be.
    #[error("people parsing model: {0}")]
    Model(String),
    /// The picture handed in can't be parsed.
    #[error("people parsing input: {0}")]
    Input(String),
    #[error("people parsing cancelled")]
    Cancelled,
    /// The tensor library failed (out of memory, a missing GPU op).
    #[error("people parsing compute: {0}")]
    Compute(String),
    #[error("people parsing cache: {0}")]
    Cache(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Self::Compute(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
