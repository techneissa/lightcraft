//! LaMa through Apple's Core ML on the Neural Engine (issue #268, `fork/coreml.md`), converted once by
//! `tools/convert-coreml-lama.py` from the `big-lama.pt` in the model's folder.
//!
//! The package lives beside the candle weights: `<dir>/coreml/lama.mlpackage`, compiled once for this Mac into
//! `lama.mlmodelc` next to it. It holds one function per square input side ([`SIDES`]), sharing one weights file: Core
//! ML plans a model again on every new input shape, so a crop is mirrored up to the smallest side that holds it (as
//! candle mirrors it up to a multiple of 8). Each takes what candle's [`crate::Lama::generate`] takes (`input`
//! [1, 4, s, s]: the picture with the hole blanked, then the hole) and gives the fill (`fill` [1, 3, s, s] in 0..1).
//! On the Neural Engine by default ([`DEFAULT_UNITS`]); a caller may pass other units at load (#267).

use std::path::{Path, PathBuf};

pub use lightcraft_coreml::{ComputeUnits, Runtime};
use lightcraft_coreml::{Error, Model};

use crate::net::{check_input, planes, put_back};

/// The Core ML files' folder inside the model's folder.
pub const COREML_DIR: &str = "coreml";
const NAME: &str = "lama";
const INPUT: &str = "input";
const OUTPUT: &str = "fill";
/// The package's square input sides, smallest first, each a function `side<n>`. The Remove panel's crops are at most 512
/// on a side and Wrinkles' at most 768; a heal spot's are about 100 to 250. A step costs no disk (the functions share
/// the weights) and saves time: on the Neural Engine 6, 22, 50, 95 and 230 ms a fill (#268).
pub const SIDES: [usize; 5] = [128, 256, 384, 512, 768];
/// The smallest crop side Core ML takes when the engine is Automatic: candle fills a smaller crop in 20 to 80 ms, so the
/// few ms gained are not worth mirroring it to over twice its size (#268).
pub const MIN_SIDE: usize = 96;
/// Where the model runs unless a caller picks other units: the Neural Engine (95 ms a 512 px fill against 0.3 s on the
/// GPU, #268). Issue #267 picks the units per Mac.
pub const DEFAULT_UNITS: ComputeUnits = ComputeUnits::CpuAndNeuralEngine;

/// Where the Core ML package and its compiled copy are in the model's folder `dir`.
pub fn paths(dir: &Path) -> (PathBuf, PathBuf) {
    let folder = dir.join(COREML_DIR);
    (folder.join(format!("{NAME}.mlpackage")), folder.join(format!("{NAME}.mlmodelc")))
}

/// Whether the Core ML version is in the model's folder `dir` (packaged or compiled; whether it loads is only known by
/// loading it).
pub fn installed(dir: &Path) -> bool {
    let (package, compiled) = paths(dir);
    package.is_dir() || compiled.is_dir()
}

/// The side of the package's function a `w × h` crop is filled at: the smallest of [`SIDES`] that holds it.
pub fn side_for(w: usize, h: usize) -> Option<usize> {
    SIDES.into_iter().find(|s| w.max(h) <= *s)
}

/// LaMa loaded on Core ML: one model per side of [`SIDES`].
pub struct CoreMlLama {
    models: Vec<(usize, Model)>,
}

impl std::fmt::Debug for CoreMlLama {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreMlLama").field("sides", &self.sides()).finish()
    }
}

impl CoreMlLama {
    /// Load every function of the package in the model's folder `dir` on `units`, compiling it first when needed
    /// (seconds), each checked to take and give what [`CoreMlLama::inpaint`] sends and reads. The Neural Engine's first
    /// load on a Mac plans each function for it (about 15 s each); later loads take a fraction of a second.
    pub fn load(dir: &Path, units: ComputeUnits) -> Result<Self, Error> {
        if !lightcraft_coreml::available() {
            return Err(Error::Unavailable);
        }
        let (package, compiled) = paths(dir);
        let mut models = Vec::with_capacity(SIDES.len());
        for side in SIDES {
            let model = lightcraft_coreml::load_package_function(&package, &compiled, units, Some(&format!("side{side}")))?;
            if !model.inputs().iter().any(|n| n == INPUT) || !model.outputs().iter().any(|n| n == OUTPUT) {
                return Err(Error::Load(format!(
                    "{} is not the converted LaMa (it takes {:?} and gives {:?})",
                    package.display(),
                    model.inputs(),
                    model.outputs()
                )));
            }
            models.push((side, model));
        }
        Ok(Self { models })
    }

    /// The sides loaded.
    pub fn sides(&self) -> Vec<usize> {
        self.models.iter().map(|(s, _)| *s).collect()
    }

    /// [`crate::Lama::inpaint`] on Core ML: the crop mirrored up to the smallest side that holds it ([`side_for`]).
    /// An error when none does, and when Core ML fails or gives a fill of the wrong size or with a number that is not
    /// finite (float16 can overflow), so the caller hands the run to candle.
    pub fn inpaint(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize) -> crate::Result<Vec<f32>> {
        check_input(rgb, hole, w, h)?;
        let side = side_for(w, h).ok_or_else(|| crate::Error::CoreMl(format!("a {w}×{h} crop is larger than the Core ML model takes")))?;
        let Some((_, model)) = self.models.iter().find(|(s, _)| *s == side) else {
            return Err(crate::Error::CoreMl(format!("the Core ML model has no {side} px function")));
        };
        let input = planes(rgb, hole, (w, h), (side, side));
        let out = model.predict(INPUT, &[1, 4, side, side], &input, OUTPUT).map_err(|e| crate::Error::CoreMl(e.to_string()))?;
        check_output(&out.data, side)?;
        let n = side * side;
        Ok(put_back(rgb, hole, w, h, |x, y, c| out.data.get(c * n + y * side + x).copied().unwrap_or(0.0)))
    }
}

/// A fill of `3 · side²` finite numbers, else an error.
pub(crate) fn check_output(data: &[f32], side: usize) -> crate::Result<()> {
    if data.len() != 3 * side * side {
        return Err(crate::Error::CoreMl(format!("the fill has {} numbers, not 3 × {side}²", data.len())));
    }
    if let Some(i) = data.iter().position(|v| !v.is_finite()) {
        return Err(crate::Error::CoreMl(format!("the fill has a number that is not finite at {i}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_takes_the_smallest_side_that_holds_it() {
        assert_eq!(side_for(1, 1), Some(128));
        assert_eq!(side_for(128, 40), Some(128));
        assert_eq!(side_for(129, 40), Some(256));
        assert_eq!(side_for(400, 300), Some(512));
        assert_eq!(side_for(512, 768), Some(768));
        assert_eq!(side_for(769, 10), None);
    }

    /// A fill Core ML gives of the wrong size or with a number that is not finite is a Core ML error (so Automatic
    /// hands the run to candle), never a fill.
    #[test]
    fn a_bad_fill_is_a_core_ml_error() {
        assert!(check_output(&[0.5; 3 * 128 * 128], 128).is_ok());
        assert!(matches!(check_output(&[0.5; 3 * 128 * 127], 128), Err(crate::Error::CoreMl(_))));
        let mut nan = vec![0.5; 3 * 128 * 128];
        nan[77] = f32::NAN;
        assert!(matches!(check_output(&nan, 128), Err(crate::Error::CoreMl(m)) if m.contains("not finite")));
        nan[77] = f32::INFINITY;
        assert!(check_output(&nan, 128).is_err());
    }

    /// No package, and a package that is not one: errors, never a panic (off the Mac Core ML is unavailable).
    #[test]
    fn a_missing_or_damaged_package_does_not_load() {
        let dir = std::env::temp_dir().join(format!("lc-lama-coreml-damaged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!installed(&dir));
        assert!(CoreMlLama::load(&dir, DEFAULT_UNITS).is_err());
        let (package, _) = paths(&dir);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("Manifest.json"), b"not a manifest").unwrap();
        assert!(installed(&dir));
        assert!(CoreMlLama::load(&dir, DEFAULT_UNITS).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The real model from the shared folder (`~/Library/Application Support/ArtCraft/models/lama`), through links in a
    /// temporary folder (Core ML compiles there; nothing is written in the shared folder): on the Neural Engine each
    /// side fills a made-up crop as candle does, to a mean of a few levels in 255 inside the hole.
    #[test]
    fn core_ml_fills_as_candle_does() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let shared = PathBuf::from(home).join("Library/Application Support/ArtCraft/models/lama");
        if !lightcraft_coreml::available() {
            lightcraft_testkit::skip!(Model, "Core ML runs only on a Mac");
            return;
        }
        let (package, _) = paths(&shared);
        let weights = shared.join("big-lama.pt");
        if !package.is_dir() || !weights.is_file() {
            lightcraft_testkit::skip!(Model, "LaMa and its Core ML package are not in the shared models folder");
            return;
        }
        // (one folder for every run: the Neural Engine's plan of each function is then made once)
        let dir = std::env::temp_dir().join("lc-lama-coreml-test");
        std::fs::create_dir_all(dir.join(COREML_DIR)).unwrap();
        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(&package, paths(&dir).0);
        }
        let coreml = CoreMlLama::load(&dir, DEFAULT_UNITS).unwrap();
        assert_eq!(coreml.sides(), SIDES.to_vec());
        let candle = crate::Lama::load(&weights, &crate::best_device()).unwrap();
        // smooth gradients and stripes with a round hole, at a size between two sides (so mirrored up)
        for (w, h) in [(200, 150), (440, 330)] {
            let rgb: Vec<f32> = (0..w * h)
                .flat_map(|i| {
                    let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                    [0.3 + 0.5 * x, 0.2 + 0.4 * y, 0.5 + 0.3 * (12.0 * x + 5.0 * y).sin()]
                })
                .collect();
            let hole: Vec<f32> = (0..w * h)
                .map(|i| (((i % w) as f32 / w as f32 - 0.5).powi(2) + ((i / w) as f32 / h as f32 - 0.5).powi(2) < 0.02) as u8 as f32)
                .collect();
            let got = coreml.inpaint(&rgb, &hole, w, h).unwrap();
            let want = candle.inpaint(&rgb, &hole, w, h, &|| false).unwrap();
            let (mut sum, mut n, mut max) = (0f32, 0, 0f32);
            for (i, m) in hole.iter().enumerate() {
                for c in 0..3 {
                    let d = (got[i * 3 + c] - want[i * 3 + c]).abs() * 255.0;
                    if *m > 0.0 {
                        (sum, n, max) = (sum + d, n + 1, max.max(d));
                    } else {
                        assert_eq!(d, 0.0, "a pixel outside the hole changed");
                    }
                }
            }
            let mean = sum / n.max(1) as f32;
            eprintln!("{w}×{h}: Core ML against candle in the hole, max {max:.1}, mean {mean:.2} (0..255)");
            assert!(mean < 4.0, "{w}×{h}: mean difference {mean}");
        }
    }
}
