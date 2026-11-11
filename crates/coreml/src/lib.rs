//! Runs converted AI models through Apple's Core ML, which spreads a model over the Neural Engine, the GPU and the
//! CPU. A model is converted once from its PyTorch original (a script in `tools/`, issue #105), compiled once on
//! this Mac ([`compile`]), loaded ([`Model::load`]) and run on `f32` tensors ([`Model::predict`]), or on several named
//! `f32` and `i32` tensors at once with several outputs read ([`Model::predict_many`]).
//!
//! The candle port of each model stays the reference and the fallback: callers try Core ML, and on any [`Error`] use
//! candle. All `unsafe` (the Objective-C calls) lives in the `macos` module; the API is safe. Elsewhere (Windows,
//! Linux, wasm) everything compiles, [`available`] is false and every call returns [`Error::Unavailable`]. With
//! lightcraft-sysmem, lightcraft-filepanel and lightcraft-macraw this is one of the crates allowed `unsafe`
//! (AGENTS.md). It has no dependency on a workspace crate.
//!
//! [`vision`] reaches Apple Vision's own person and subject masks the same way (issue #115).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![deny(unsafe_op_in_unsafe_fn)]

mod cache;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod layout;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod stub;
pub mod vision;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod vision_macos;

pub use cache::{load_package, load_package_function};
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(not(target_os = "macos"))]
use stub as imp;
#[cfg(not(target_os = "macos"))]
use stub::vision as imp_vision;
#[cfg(target_os = "macos")]
use vision_macos as imp_vision;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// Largest tensor passed in or read out (64 M numbers, 256 MB as `f32`).
pub const MAX_ELEMENTS: usize = 64 << 20;
/// Most dimensions of a tensor (the Neural Engine takes at most 5).
pub const MAX_RANK: usize = 5;

/// Where Core ML may run a model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ComputeUnits {
    /// Neural Engine, GPU and CPU: Core ML picks per layer.
    #[default]
    All,
    /// Neural Engine and CPU, never the GPU (leaves the GPU to rendering).
    CpuAndNeuralEngine,
    CpuAndGpu,
    CpuOnly,
}

/// Which engine runs a model that has both a candle port and a Core ML conversion (a setting per model).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Runtime {
    /// Core ML when its model file is there and loads, else candle; candle also takes over when Core ML fails.
    #[default]
    Automatic,
    /// Core ML only: an error when its file is missing or fails.
    CoreMl,
    /// candle only (the reference port).
    Candle,
}

impl Runtime {
    pub const ALL: [Runtime; 3] = [Runtime::Automatic, Runtime::CoreMl, Runtime::Candle];

    /// The id commands and settings files use.
    pub fn id(self) -> &'static str {
        match self {
            Runtime::Automatic => "automatic",
            Runtime::CoreMl => "coreml",
            Runtime::Candle => "candle",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.id() == id)
    }

    /// The name shown in Settings.
    pub fn label(self) -> &'static str {
        match self {
            Runtime::Automatic => "Automatic",
            Runtime::CoreMl => "Core ML",
            Runtime::Candle => "Candle",
        }
    }
}

/// Why a Core ML call failed. Every variant has a message a user can act on; none is a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a Mac, or Core ML lacks a call this crate needs.
    Unavailable,
    /// The model file or folder is not there.
    Missing(PathBuf),
    /// Core ML could not compile a `.mlpackage`.
    Compile(String),
    /// Core ML could not load a compiled model (damaged, foreign, or made for a newer macOS).
    Load(String),
    /// The tensor passed in does not fit the model (name, shape, size).
    Input(String),
    /// Core ML failed while running the model.
    Predict(String),
    /// The model's output is missing or not a numeric tensor.
    Output(String),
    /// A file operation around the model failed.
    Io(String),
    /// Apple Vision could not make a mask ([`vision`]).
    Vision(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unavailable => f.write_str("Core ML runs only on a Mac"),
            Error::Missing(p) => write!(f, "no Core ML model at {}", p.display()),
            Error::Compile(m) => write!(f, "Core ML could not compile the model: {m}"),
            Error::Load(m) => write!(f, "Core ML could not load the model: {m}"),
            Error::Input(m) => write!(f, "Core ML input: {m}"),
            Error::Predict(m) => write!(f, "Core ML could not run the model: {m}"),
            Error::Output(m) => write!(f, "Core ML output: {m}"),
            Error::Io(m) => f.write_str(m),
            Error::Vision(m) => write!(f, "Apple Vision: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Most tensors passed in or read out in one call.
pub const MAX_TENSORS: usize = 16;

/// The numbers of a tensor passed in, row-major.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Data<'a> {
    F32(&'a [f32]),
    /// Whole numbers (token ids, click labels).
    I32(&'a [i32]),
}

impl Data<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Data::F32(d) => d.len(),
            Data::I32(d) => d.len(),
        }
    }
}

/// A named tensor passed in: its shape and its numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tensor<'a> {
    pub name: &'a str,
    pub shape: &'a [usize],
    pub data: Data<'a>,
}

impl<'a> Tensor<'a> {
    pub fn f32(name: &'a str, shape: &'a [usize], data: &'a [f32]) -> Self {
        Self { name, shape, data: Data::F32(data) }
    }

    pub fn i32(name: &'a str, shape: &'a [usize], data: &'a [i32]) -> Self {
        Self { name, shape, data: Data::I32(data) }
    }
}

/// A tensor read out of a model: its shape and its numbers in row-major order.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// True on a Mac whose Core ML has every call this crate makes.
pub fn available() -> bool {
    imp::available()
}

/// Whether one function of a model with several can be loaded ([`Model::load_function`]): macOS 15 or later.
pub fn functions_available() -> bool {
    imp::available() && imp::functions_available()
}

/// Compile a `.mlpackage` (or `.mlmodel`) for this Mac and put the compiled `.mlmodelc` folder at `dest`, replacing
/// what was there. Takes seconds; run it once after a download, off the UI thread.
pub fn compile(package: &Path, dest: &Path) -> Result<()> {
    if !package.exists() {
        return Err(Error::Missing(package.to_path_buf()));
    }
    let built = imp::compile(package)?;
    let moved = layout::replace_dir(&built, dest);
    // Core ML compiles into a temporary folder; nothing of it is kept when the move fails.
    let _ = std::fs::remove_dir_all(&built);
    moved
}

/// A loaded model. Calls are serialised: one prediction at a time per model.
pub struct Model {
    handle: Mutex<imp::Handle>,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model").field("inputs", &self.inputs).field("outputs", &self.outputs).finish()
    }
}

impl Model {
    /// Load a compiled `.mlmodelc` folder.
    pub fn load(compiled: &Path, units: ComputeUnits) -> Result<Self> {
        Self::load_function(compiled, units, None)
    }

    /// Load `function` of a compiled model with several (`None`: its default; macOS 15 or later for another). The
    /// functions of one model share its weights on disk and in memory.
    pub fn load_function(compiled: &Path, units: ComputeUnits, function: Option<&str>) -> Result<Self> {
        if !compiled.is_dir() {
            return Err(Error::Missing(compiled.to_path_buf()));
        }
        let (handle, inputs, outputs) = imp::load(compiled, units, function)?;
        Ok(Self { handle: Mutex::new(handle), inputs, outputs })
    }

    /// The model's input names.
    pub fn inputs(&self) -> &[String] {
        &self.inputs
    }

    /// The model's output names.
    pub fn outputs(&self) -> &[String] {
        &self.outputs
    }

    /// Run the model on one `f32` tensor `input` of `shape` and read the tensor `output`.
    pub fn predict(&self, input: &str, shape: &[usize], data: &[f32], output: &str) -> Result<Output> {
        let mut out = self.predict_many(&[Tensor::f32(input, shape, data)], &[output])?;
        out.pop().ok_or_else(|| Error::Output(format!("the result has no `{output}`")))
    }

    /// Run the model on `inputs` (each of its inputs once) and read `outputs`, in the order asked for. Names and
    /// shapes are checked before Core ML is called.
    pub fn predict_many(&self, inputs: &[Tensor<'_>], outputs: &[&str]) -> Result<Vec<Output>> {
        layout::check_io(&self.inputs, &self.outputs, inputs, outputs)?;
        let handle = self.handle.lock().unwrap_or_else(PoisonError::into_inner);
        imp::predict(&handle, inputs, outputs)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_vision;
