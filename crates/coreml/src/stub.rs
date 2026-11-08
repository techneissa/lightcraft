//! The backend where Core ML is absent (not a Mac, wasm): nothing is available.

use std::path::{Path, PathBuf};

use crate::{ComputeUnits, Error, Output, Result};

/// Nothing: there is no model to hold.
pub(crate) struct Handle;

pub(crate) fn available() -> bool {
    false
}

pub(crate) fn compile(_package: &Path) -> Result<PathBuf> {
    Err(Error::Unavailable)
}

pub(crate) fn load(_compiled: &Path, _units: ComputeUnits) -> Result<(Handle, Vec<String>, Vec<String>)> {
    Err(Error::Unavailable)
}

pub(crate) fn predict(_handle: &Handle, _input: &str, _shape: &[usize], _data: &[f32], _output: &str) -> Result<Output> {
    Err(Error::Unavailable)
}
