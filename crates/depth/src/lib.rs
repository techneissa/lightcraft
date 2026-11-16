//! A photo's depth, from one picture: Depth Anything 3 Mono Large (ByteDance Seed, 2025; code and weights
//! Apache-2.0) in pure Rust on candle. The network is a DINOv2 ViT-L/14 backbone and a DPT head with a second,
//! one-channel sky head; it gives depth (up to a scale, larger is farther) at the size of its input, about 504
//! pixels on the long side.
//!
//! The weights are not part of LightCraft: they are the safetensors file of `depth-anything/DA3MONO-LARGE` on
//! Hugging Face (Apache-2.0), downloaded into the shared models folder ([`fetch`]) and pinned by size and
//! SHA-256. On macOS the network runs on the GPU through Metal, elsewhere on the CPU. No UI dependencies (L3).
//!
//! Ported from <https://github.com/ByteDance-Seed/Depth-Anything-3> (Apache-2.0, Copyright 2025 ByteDance Ltd.
//! and/or its affiliates) and the DINOv2 it builds on (Apache-2.0, Copyright Meta Platforms); see `NOTICE`.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
mod dpt;
#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
pub mod image;
#[cfg(not(target_arch = "wasm32"))]
mod params;
#[cfg(not(target_arch = "wasm32"))]
mod run;
#[cfg(not(target_arch = "wasm32"))]
mod vit;
#[cfg(not(target_arch = "wasm32"))]
mod weights;

#[cfg(not(target_arch = "wasm32"))]
pub use run::{DepthModel, best_device};

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("depth model: {0}")]
    Model(String),
    #[error("depth model file not found in {0} (expected model.safetensors)")]
    Missing(PathBuf),
    #[error("depth estimation cancelled")]
    Cancelled,
    #[cfg(not(target_arch = "wasm32"))]
    #[error(transparent)]
    Candle(#[from] candle_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The checkpoint's file name in a model directory.
pub const WEIGHTS_FILE: &str = "model.safetensors";

/// Whether `dir` holds the model's weights.
pub fn is_model_dir(dir: &std::path::Path) -> bool {
    dir.join(WEIGHTS_FILE).is_file()
}

/// What the model gives for a photo: depth and sky, `width × height`, row-major, over the whole photo (its
/// input was the photo resized, aspect kept up to rounding to whole patches).
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    pub width: usize,
    pub height: usize,
    /// Depth, up to a scale: larger is farther. Finite and positive. The sky is set to the far end of the rest
    /// (the 99th percentile of depth outside it), as the model's own post-processing does.
    pub depth: Vec<f32>,
    /// How sure the model is that a pixel is sky (0 = not), before any threshold.
    pub sky: Vec<f32>,
}
