//! The backend where Core ML is absent (not a Mac, wasm): nothing is available.

use std::path::{Path, PathBuf};

use crate::{ComputeUnits, Error, Output, Result, Tensor};

/// Nothing: there is no model to hold.
pub(crate) struct Handle;

pub(crate) fn available() -> bool {
    false
}

pub(crate) fn functions_available() -> bool {
    false
}

pub(crate) fn compile(_package: &Path) -> Result<PathBuf> {
    Err(Error::Unavailable)
}

pub(crate) fn load(_compiled: &Path, _units: ComputeUnits, _function: Option<&str>) -> Result<(Handle, Vec<String>, Vec<String>)> {
    Err(Error::Unavailable)
}

pub(crate) fn predict(_handle: &Handle, _inputs: &[Tensor<'_>], _outputs: &[&str]) -> Result<Vec<Output>> {
    Err(Error::Unavailable)
}

/// Apple Vision where it is absent: no request is supported.
pub(crate) mod vision {
    use crate::vision::{Image, Matte, Request};
    use crate::{Error, Result};

    pub(crate) fn supported(_request: Request) -> bool {
        false
    }

    pub(crate) fn person_segmentation(_img: &Image) -> Result<Matte> {
        Err(Error::Unavailable)
    }

    pub(crate) fn instances(_img: &Image, _kind: Request, _each: &mut dyn FnMut(Matte) -> Result<()>) -> Result<()> {
        Err(Error::Unavailable)
    }
}
