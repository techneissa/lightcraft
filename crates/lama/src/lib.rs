//! LaMa ("Resolution-robust Large Mask Inpainting with Fourier Convolutions", Suvorov et al., WACV
//! 2022; code and `big-lama` weights Apache-2.0) in pure Rust on Candle: a learned fill for large holes.
//!
//! - [`Lama::load`] reads the weights from the TorchScript file `big-lama.pt` (or a Lightning
//!   checkpoint) without Python.
//! - [`Lama::inpaint`] fills a hole in a picture of up to 2048 pixels a side.
//! - [`region`] fills a hole in a bigger picture: it works on a crop around the hole, at most 512
//!   pixels on a side, and puts only the hole's pixels back.
//!
//! The Fourier units' `rfft2` / `irfft2` ([`fft`]) are matrix products on the model's device. Metal
//! on macOS (when a GPU is available), else the CPU ([`best_device`]).
//!
//! Native only: on wasm32 this crate is empty.

#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod checkpoint;
mod cl;
mod error;
pub mod fft;
mod layers;
mod net;

pub use checkpoint::read_generator;
pub use error::{Error, Result};
pub use net::{Lama, MAX_SIDE, MODULO};

use candle_core::Device;

/// The best device here: Metal on macOS when a GPU is available, else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}
