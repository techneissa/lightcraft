//! The backend where Core Image is absent (not a Mac, wasm): nothing is available.

use crate::{Error, Input, LinearImage, Probe, Request};
use std::sync::atomic::AtomicBool;

pub(crate) fn available() -> bool {
    false
}

pub(crate) fn supports(_exif_model: &str) -> bool {
    false
}

pub(crate) fn probe(_input: Input<'_>) -> Result<Probe, Error> {
    Err(Error::Unsupported)
}

pub(crate) fn render(_input: Input<'_>, _request: &Request, _cancel: &AtomicBool) -> Result<LinearImage, Error> {
    Err(Error::Unsupported)
}

pub(crate) fn cache_key() -> u64 {
    0
}

pub(crate) fn os_build() -> String {
    String::new()
}
