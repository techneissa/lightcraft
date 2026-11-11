//! BiRefNet and BiRefNet Matting through Apple's Core ML on the GPU (issue #264, `fork/coreml.md`), converted once by
//! `tools/convert-coreml-birefnet.py` from the weights in the model's folder.
//!
//! The package lives beside the candle weights: `<dir>/coreml/<folder>.mlpackage` (`birefnet` or `birefnet-matting`),
//! compiled once for this Mac into `<folder>.mlmodelc` next to it. It takes the same preprocessed pixels as candle
//! (`pixels` [1, 3, 1024, 1024]) and gives the same logits (`logits` [1, 1, 1024, 1024]), so a [`crate::Matte`] from
//! either engine is used the same way. One fixed input shape: Core ML plans a model on the GPU again on every new
//! shape (#200). On the GPU (`CpuAndGpu`), not the Neural Engine: there it took 11 s a run and minutes to load (#115).

use std::path::{Path, PathBuf};

use lightcraft_coreml::{ComputeUnits, Error, Model};

use crate::Variant;

/// The Core ML files' folder inside a model's folder.
pub const COREML_DIR: &str = "coreml";
const INPUT: &str = "pixels";
const OUTPUT: &str = "logits";

/// Where `variant`'s Core ML package and compiled model are in its folder `dir`.
pub fn paths(dir: &Path, variant: Variant) -> (PathBuf, PathBuf) {
    let folder = dir.join(COREML_DIR);
    (folder.join(format!("{}.mlpackage", variant.dir_name())), folder.join(format!("{}.mlmodelc", variant.dir_name())))
}

/// Whether `variant`'s Core ML version is in its folder `dir` (packaged or compiled; whether it loads is only known
/// by loading it).
pub fn installed(dir: &Path, variant: Variant) -> bool {
    let (package, compiled) = paths(dir, variant);
    package.is_dir() || compiled.is_dir()
}

/// A loaded Core ML BiRefNet.
pub(crate) struct CoreMlNet {
    model: Model,
    side: usize,
}

impl CoreMlNet {
    /// Load `variant`'s converted model from its folder `dir` on the GPU, compiling it first when needed, checked to
    /// take and give what [`CoreMlNet::run`] sends and reads.
    pub(crate) fn load(dir: &Path, variant: Variant) -> Result<Self, Error> {
        if !lightcraft_coreml::available() {
            return Err(Error::Unavailable);
        }
        let (package, compiled) = paths(dir, variant);
        let model = lightcraft_coreml::load_package(&package, &compiled, ComputeUnits::CpuAndGpu)?;
        if !model.inputs().iter().any(|n| n == INPUT) || !model.outputs().iter().any(|n| n == OUTPUT) {
            return Err(Error::Load(format!(
                "{} is not the converted {} (it takes {:?} and gives {:?})",
                package.display(),
                variant.label(),
                model.inputs(),
                model.outputs()
            )));
        }
        Ok(Self { model, side: variant.input() })
    }

    /// The side of the square input the package takes.
    pub(crate) fn side(&self) -> usize {
        self.side
    }

    /// The logits of preprocessed pixels (`3 · side²`, channel-first, normalised), row-major `side × side`.
    pub(crate) fn run(&self, pixels: &[f32]) -> Result<Vec<f32>, Error> {
        let side = self.side;
        let out = self.model.predict(INPUT, &[1, 3, side, side], pixels, OUTPUT)?;
        if out.data.len() != side * side {
            return Err(Error::Output(format!("{} numbers, not {side}²", out.data.len())));
        }
        Ok(out.data)
    }
}
