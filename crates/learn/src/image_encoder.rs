//! The image encoder as the app runs it: through Core ML on the Mac when its converted model is installed (Neural
//! Engine, issue #105, `fork/coreml.md`), else through the candle port ([`crate::encoder`]), which stays the
//! reference.
//!
//! The model folder holds `model.safetensors` (candle) and, optionally, `coreml/dinov2-small.mlpackage` (made once by
//! `tools/convert-coreml-dinov2.py`, or downloaded). The package is compiled for this Mac once, into
//! `coreml/dinov2-small.mlmodelc`, the first time it is loaded, and again when the package changes. With [`Runtime::Automatic`] a missing, damaged or failing Core ML model means candle,
//! never a panic; [`Runtime::CoreMl`] makes that an error instead.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub use lightcraft_coreml::Runtime;
use lightcraft_coreml::{ComputeUnits, Model, Sample, UnitOrder};

use crate::encoder::{DIMS, Encoder, INPUT, checked, pixels};
use crate::{Error, Result};

/// The Core ML files' folder inside the model folder.
pub const COREML_DIR: &str = "coreml";
/// The converted model as downloaded or installed.
pub const COREML_PACKAGE: &str = "dinov2-small.mlpackage";
/// The same, compiled for this Mac.
pub const COREML_COMPILED: &str = "dinov2-small.mlmodelc";
const INPUT_NAME: &str = "pixels";
const OUTPUT_NAME: &str = "features";
/// Where the encoder runs until the app has measured this Mac (issue #267): Core ML's choice of unit per layer, mostly
/// the Neural Engine, for this plain ViT.
pub const DEFAULT_UNITS: ComputeUnits = ComputeUnits::All;

/// Which engine computed (or will compute) the features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    CoreMl,
    Candle,
}

impl Backend {
    pub fn id(self) -> &'static str {
        match self {
            Backend::CoreMl => "coreml",
            Backend::Candle => "candle",
        }
    }
}

/// The package and compiled paths of the Core ML model in `model_dir`.
pub fn coreml_paths(model_dir: &Path) -> (PathBuf, PathBuf) {
    let dir = model_dir.join(COREML_DIR);
    (dir.join(COREML_PACKAGE), dir.join(COREML_COMPILED))
}

/// Whether a Core ML model is in `model_dir` (packaged or compiled; whether it loads is only known by loading it).
pub fn coreml_installed(model_dir: &Path) -> bool {
    let (package, compiled) = coreml_paths(model_dir);
    package.is_dir() || compiled.is_dir()
}

/// The call a measurement of the Core ML model in `model_dir` makes (issue #267): one photo's pixels.
pub fn coreml_sample(model_dir: &Path) -> Sample {
    let (package, compiled) = coreml_paths(model_dir);
    Sample { package, compiled, function: None, input: INPUT_NAME, shape: vec![1, 3, INPUT, INPUT], output: OUTPUT_NAME }
}

/// The Core ML model of `model_dir` on the first of `units` that loads it, checked to answer with [`DIMS`] numbers.
fn load_coreml(model_dir: &Path, units: UnitOrder) -> std::result::Result<Model, lightcraft_coreml::Error> {
    if !lightcraft_coreml::available() {
        return Err(lightcraft_coreml::Error::Unavailable);
    }
    let (package, compiled) = coreml_paths(model_dir);
    let (model, _) = lightcraft_coreml::first_that_loads(units, |u| lightcraft_coreml::load_package(&package, &compiled, u))?;
    let out = model.predict(INPUT_NAME, &[1, 3, INPUT, INPUT], &vec![0.0; 3 * INPUT * INPUT], OUTPUT_NAME)?;
    if out.data.len() != DIMS {
        return Err(lightcraft_coreml::Error::Output(format!("{} numbers, not {DIMS}", out.data.len())));
    }
    Ok(model)
}

/// A loaded encoder (Core ML, candle, or Core ML with candle behind it).
pub struct ImageEncoder {
    runtime: Runtime,
    weights: PathBuf,
    coreml: Option<Model>,
    /// Set when Core ML failed on a photo: candle runs from then on.
    coreml_failed: AtomicBool,
    candle: OnceLock<std::result::Result<Encoder, String>>,
}

impl ImageEncoder {
    /// Load the encoder of `model_dir` for `runtime`. Candle is loaded now when it is the one that runs, else only if
    /// Core ML fails later.
    pub fn load(model_dir: &Path, runtime: Runtime) -> Result<Self> {
        Self::load_on_units(model_dir, runtime, UnitOrder::one(DEFAULT_UNITS))
    }

    /// [`ImageEncoder::load`] with Core ML on the first of `units` that loads it (issue #267).
    pub fn load_on_units(model_dir: &Path, runtime: Runtime, units: UnitOrder) -> Result<Self> {
        let weights = model_dir.join(crate::ENCODER_FILE);
        let coreml = match runtime {
            Runtime::Candle => None,
            Runtime::CoreMl => Some(load_coreml(model_dir, units).map_err(|e| Error::Model(e.to_string()))?),
            Runtime::Automatic => match load_coreml(model_dir, units) {
                Ok(m) => Some(m),
                Err(lightcraft_coreml::Error::Unavailable | lightcraft_coreml::Error::Missing(_)) => None,
                Err(e) => {
                    log::warn!("learn: the Core ML image model does not load ({e}); using candle");
                    None
                }
            },
        };
        let me = Self { runtime, weights, coreml, coreml_failed: AtomicBool::new(false), candle: OnceLock::new() };
        if me.coreml.is_none() {
            me.candle()?;
        }
        Ok(me)
    }

    /// The runtime asked for.
    pub fn runtime(&self) -> Runtime {
        self.runtime
    }

    /// The engine the next photo runs on.
    pub fn backend(&self) -> Backend {
        if self.coreml.is_some() && !self.coreml_failed.load(Ordering::Relaxed) { Backend::CoreMl } else { Backend::Candle }
    }

    fn candle(&self) -> Result<&Encoder> {
        self.candle.get_or_init(|| Encoder::load(&self.weights).map_err(|e| e.to_string())).as_ref().map_err(|e| Error::Model(e.clone()))
    }

    /// The features of [`pixels`]' output.
    pub fn run(&self, planar: Vec<f32>) -> Result<Vec<f32>> {
        if let (Some(model), Backend::CoreMl) = (&self.coreml, self.backend()) {
            let out = model
                .predict(INPUT_NAME, &[1, 3, INPUT, INPUT], &planar, OUTPUT_NAME)
                .map_err(|e| Error::Model(e.to_string()))
                .and_then(|o| checked(o.data));
            match out {
                Ok(f) => return Ok(f),
                Err(e) if self.runtime == Runtime::CoreMl => return Err(e),
                Err(e) => {
                    log::warn!("learn: Core ML failed on a photo ({e}); using candle from now on");
                    self.coreml_failed.store(true, Ordering::Relaxed);
                }
            }
        }
        self.candle()?.run(planar)
    }
}

impl crate::ImageFeatures for ImageEncoder {
    fn features(&self, width: usize, height: usize, rgb: &[u8]) -> Result<Vec<f32>> {
        self.run(pixels(width, height, rgb)?)
    }
}

#[cfg(test)]
#[path = "image_encoder_tests.rs"]
mod tests;
