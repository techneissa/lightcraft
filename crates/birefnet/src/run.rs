//! Loading the model and running it on a photo: on candle (Metal, else the CPU), or on Core ML when its converted
//! package is there (issue #264, [`crate::coreml`]), candle staying the reference and the fallback.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use candle_core::{Device, Tensor};
pub use lightcraft_coreml::Runtime;

use crate::coreml::CoreMlNet;
use crate::model::Net;
use crate::params::Params;
use crate::weights::Weights;
use crate::{Error, Matte, Result, Variant, WEIGHTS_FILE, is_model_dir};

/// The best device here: Metal on macOS (when a GPU is available), else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

/// The engine a loaded model runs on.
enum Engine {
    Candle(Box<Net>),
    CoreMl(CoreMlNet),
}

/// A loaded model.
pub struct BiRefNet {
    engine: Engine,
    /// candle's device (where candle runs, also when it takes over from Core ML).
    device: Device,
    dir: PathBuf,
    runtime: Runtime,
    side: usize,
    cancel: Option<Arc<AtomicBool>>,
}

impl BiRefNet {
    /// Load from a directory holding `model.safetensors`: on Core ML when `variant`'s converted package is there and
    /// loads ([`Runtime::Automatic`]), else on candle on the best device.
    pub fn load(dir: &Path, variant: Variant) -> Result<Self> {
        Self::load_with(dir, variant, Runtime::Automatic, best_device())
    }

    /// [`BiRefNet::load`] on candle on the CPU, whatever GPU there is.
    pub fn load_cpu(dir: &Path, variant: Variant) -> Result<Self> {
        Self::load_with(dir, variant, Runtime::Candle, Device::Cpu)
    }

    /// candle on `device`.
    pub fn load_on(dir: &Path, variant: Variant, device: Device) -> Result<Self> {
        Self::load_with(dir, variant, Runtime::Candle, device)
    }

    /// Load on `runtime`: Automatic is Core ML when the package is there and loads, else candle on `device`; Core ML
    /// is an error when it can't load ([`Error::CoreMl`]); Candle is candle on `device`.
    pub fn load_with(dir: &Path, variant: Variant, runtime: Runtime, device: Device) -> Result<Self> {
        Self::load_with_units(dir, variant, runtime, crate::coreml::DEFAULT_UNITS, device)
    }

    /// [`BiRefNet::load_with`] with Core ML on `units` (all, the GPU, the Neural Engine or the CPU; issue #267).
    pub fn load_with_units(dir: &Path, variant: Variant, runtime: Runtime, units: crate::coreml::ComputeUnits, device: Device) -> Result<Self> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let side = variant.input();
        if runtime != Runtime::Candle {
            match CoreMlNet::load(dir, variant, units) {
                Ok(net) => {
                    let side = net.side();
                    return Ok(Self { engine: Engine::CoreMl(net), device, dir: dir.to_path_buf(), runtime, side, cancel: None });
                }
                Err(e) if runtime == Runtime::CoreMl => return Err(Error::CoreMl(e.to_string())),
                Err(lightcraft_coreml::Error::Missing(_) | lightcraft_coreml::Error::Unavailable) => {}
                Err(e) => log::warn!("{} on Core ML did not load ({e}); using candle", variant.label()),
            }
        }
        Self::load_with_side(dir, side, device)
    }

    /// candle on `device` with the input side given (tests run the network on small squares).
    pub fn load_with_side(dir: &Path, side: usize, device: Device) -> Result<Self> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        if side == 0 || !side.is_multiple_of(32) || side > 4096 {
            return Err(Error::Model(format!("input side {side} is not a multiple of 32 up to 4096")));
        }
        let net = candle_net(dir, &device)?;
        Ok(Self { engine: Engine::Candle(Box::new(net)), device, dir: dir.to_path_buf(), runtime: Runtime::Candle, side, cancel: None })
    }

    /// Install an optional cancellation flag checked between backbone blocks (on Core ML: before and after the run,
    /// which can't be stopped midway).
    pub fn set_cancel(&mut self, cancel: Option<Arc<AtomicBool>>) {
        self.cancel = cancel;
    }

    /// candle's device: where candle runs (and would take over from Core ML).
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The engine the model runs on: `coreml` or `candle`.
    pub fn backend(&self) -> &'static str {
        match self.engine {
            Engine::Candle(_) => "candle",
            Engine::CoreMl(_) => "coreml",
        }
    }

    /// Side of the square the photo is stretched to.
    pub fn side(&self) -> usize {
        self.side
    }

    /// Wait for queued GPU work to finish (benchmarks).
    pub fn sync(&self) -> Result<()> {
        Ok(self.device.synchronize()?)
    }

    /// The matte of an RGB image (8-bit, row-major, `w × h`), stretched to the square input.
    pub fn matte(&mut self, rgb: &[u8], w: usize, h: usize) -> Result<Matte> {
        let pixels = crate::image::preprocess(rgb, w, h, self.side)?;
        self.matte_pixels(pixels)
    }

    /// The matte of preprocessed pixels (`3 · side²`, channel-first, normalised). On Automatic, a Core ML run that
    /// fails hands this and every later run to candle.
    pub fn matte_pixels(&mut self, pixels: Vec<f32>) -> Result<Matte> {
        let side = self.side;
        let flag = self.cancel.clone();
        let cancel = move || flag.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
        if cancel() {
            return Err(Error::Cancelled);
        }
        let logits = match &self.engine {
            Engine::CoreMl(net) => match net.run(&pixels) {
                Ok(l) => Some(l),
                Err(e) if self.runtime == Runtime::CoreMl => return Err(Error::CoreMl(e.to_string())),
                Err(e) => {
                    log::warn!("BiRefNet on Core ML failed ({e}); using candle from now on");
                    None
                }
            },
            Engine::Candle(_) => None,
        };
        let logits = match logits {
            Some(l) => l,
            None => {
                if let Engine::CoreMl(_) = self.engine {
                    self.engine = Engine::Candle(Box::new(candle_net(&self.dir, &self.device)?));
                }
                if cancel() {
                    return Err(Error::Cancelled);
                }
                let Engine::Candle(net) = &self.engine else { return Err(Error::Model("candle did not load".into())) };
                let x = Tensor::from_vec(pixels, (1, 3, side, side), &self.device)?;
                net.forward(&x, &cancel)?.flatten_all()?.to_vec1::<f32>()?
            }
        };
        if cancel() {
            return Err(Error::Cancelled);
        }
        if logits.len() != side * side || logits.iter().any(|v| !v.is_finite()) {
            return Err(Error::Model("the model produced an invalid matte".into()));
        }
        Ok(Matte { side, logits })
    }
}

/// candle's network from the weights in `dir`, on `device`.
fn candle_net(dir: &Path, device: &Device) -> Result<Net> {
    let weights = Arc::new(Weights::open(&dir.join(WEIGHTS_FILE))?);
    Net::load(&Params::new(weights, device.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swin::SwinConfig;

    /// A directory holding a safetensors file of zero-filled F32 tensors with these names and shapes.
    fn synthetic(name: &str, tensors: &[(String, Vec<usize>)]) -> std::path::PathBuf {
        let mut header = serde_json::Map::new();
        let mut at = 0u64;
        for (n, shape) in tensors {
            let bytes = shape.iter().product::<usize>() as u64 * 4;
            header.insert(n.clone(), serde_json::json!({"dtype": "F32", "shape": shape, "data_offsets": [at, at + bytes]}));
            at += bytes;
        }
        let dir = std::env::temp_dir().join(format!("lc-birefnet-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let header = serde_json::to_string(&header).unwrap();
        let mut file = (header.len() as u64).to_le_bytes().to_vec();
        file.extend_from_slice(header.as_bytes());
        file.extend(std::iter::repeat_n(0u8, at as usize));
        std::fs::write(dir.join(WEIGHTS_FILE), file).unwrap();
        dir
    }

    /// The tensors `SwinConfig::detect` looks at, for a backbone of embedding width `e`, `window`, and `depths` blocks per stage.
    fn backbone(e: usize, window: usize, depths: [usize; 4]) -> Vec<(String, Vec<usize>)> {
        let mut t = vec![("bb.patch_embed.proj.weight".to_string(), vec![e, 3, 4, 4])];
        for (stage, depth) in depths.iter().enumerate() {
            for block in 0..*depth {
                t.push((format!("bb.layers.{stage}.blocks.{block}.norm1.weight"), vec![e << stage]));
                t.push((
                    format!("bb.layers.{stage}.blocks.{block}.attn.relative_position_bias_table"),
                    vec![(2 * window - 1) * (2 * window - 1), 3 << stage],
                ));
            }
        }
        t
    }

    #[test]
    fn the_backbone_size_is_read_from_the_weights() {
        let dir = synthetic("detect", &backbone(96, 7, [2, 2, 6, 2]));
        let weights = Arc::new(Weights::open(&dir.join(WEIGHTS_FILE)).unwrap());
        let cfg = SwinConfig::detect(&Params::new(weights, Device::Cpu)).unwrap();
        assert_eq!(cfg, SwinConfig { embed: 96, depths: vec![2, 2, 6, 2], heads: vec![3, 6, 12, 24], window: 7 });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_is_not_birefnet_is_an_error_not_a_panic() {
        // no backbone at all
        let dir = synthetic("other", &[("something.weight".to_string(), vec![2, 2])]);
        let err = BiRefNet::load_with_side(&dir, 64, Device::Cpu).err().unwrap().to_string();
        assert!(err.contains("no Swin backbone"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
        // a backbone whose bias table is not (2w-1)²
        let mut t = backbone(96, 7, [1, 1, 1, 1]);
        t[2].1 = vec![100, 3];
        let dir = synthetic("table", &t);
        let err = BiRefNet::load_with_side(&dir, 64, Device::Cpu).err().unwrap().to_string();
        assert!(err.contains("not (2w-1)"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
        // a stage without blocks
        let dir = synthetic("empty-stage", &backbone(96, 7, [2, 0, 2, 2]));
        assert!(BiRefNet::load_with_side(&dir, 64, Device::Cpu).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_and_wrong_shaped_tensors_are_named() {
        // a plausible backbone that stops short: the first tensor the loader wants after the sizes
        let dir = synthetic("short", &backbone(96, 7, [1, 1, 1, 1]));
        let err = BiRefNet::load_with_side(&dir, 64, Device::Cpu).err().unwrap().to_string();
        assert!(err.contains("missing tensor") || err.contains("shape mismatch"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
        // a tensor with the wrong shape is refused before its bytes are read
        let mut t = backbone(96, 7, [1, 1, 1, 1]);
        t.push(("bb.patch_embed.proj.bias".to_string(), vec![5]));
        let dir = synthetic("shape", &t);
        let err = BiRefNet::load_with_side(&dir, 64, Device::Cpu).err().unwrap().to_string();
        assert!(err.contains("shape mismatch for `bb.patch_embed.proj.bias`") && err.contains("[96]"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_model_and_bad_input_sizes_are_errors() {
        let empty = std::env::temp_dir().join(format!("lc-birefnet-none-{}", std::process::id()));
        assert!(matches!(BiRefNet::load_with_side(&empty, 1024, Device::Cpu), Err(Error::Missing(_))));
        let dir = synthetic("side", &backbone(96, 7, [1, 1, 1, 1]));
        for side in [0, 100, 8192] {
            let err = BiRefNet::load_with_side(&dir, side, Device::Cpu).err().unwrap().to_string();
            assert!(err.contains("multiple of 32"), "{side}: {err}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A Core ML package that does not load: Automatic goes on to candle (here candle's own error about the short
    /// checkpoint, not Core ML's), Core ML only is a Core ML error. No panic; runs everywhere (off the Mac Core ML is
    /// unavailable).
    #[test]
    fn a_damaged_core_ml_package_falls_back_to_candle() {
        let dir = synthetic("coreml-damaged", &backbone(96, 7, [1, 1, 1, 1]));
        let (package, _) = crate::coreml::paths(&dir, Variant::General);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("Manifest.json"), b"not a manifest").unwrap();
        assert!(crate::coreml::installed(&dir, Variant::General));
        let e = BiRefNet::load_with(&dir, Variant::General, Runtime::Automatic, Device::Cpu).err().unwrap();
        assert!(!matches!(e, Error::CoreMl(_)), "{e}");
        assert!(e.to_string().contains("missing tensor") || e.to_string().contains("shape mismatch"), "{e}");
        let e = BiRefNet::load_with(&dir, Variant::General, Runtime::CoreMl, Device::Cpu).err().unwrap();
        assert!(matches!(e, Error::CoreMl(_)), "{e}");
        // and with no package at all
        std::fs::remove_dir_all(dir.join(crate::coreml::COREML_DIR)).unwrap();
        let e = BiRefNet::load_with(&dir, Variant::General, Runtime::Automatic, Device::Cpu).err().unwrap();
        assert!(!matches!(e, Error::CoreMl(_)), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The real models from the shared folder (`~/Library/Application Support/ArtCraft/models`), through links in a
    /// temporary folder (Core ML compiles there; nothing is written in the shared folder): on Core ML each gives
    /// candle's matte of a made-up photo, and without its package Automatic is candle.
    #[test]
    fn core_ml_gives_candles_matte() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let shared = std::path::PathBuf::from(home).join("Library/Application Support/ArtCraft/models");
        if !lightcraft_coreml::available() {
            lightcraft_testkit::skip!(Model, "Core ML runs only on a Mac");
            return;
        }
        // a bright disc on a darker gradient
        let (w, h) = (640, 480);
        let rgb: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                let disc = if (x - 0.5).powi(2) + (y - 0.5).powi(2) < 0.06 { 120.0 } else { 0.0 };
                [40.0 + 80.0 * x + disc, 60.0 + 50.0 * y + disc, 70.0 + disc].map(|v: f32| v.min(255.0) as u8)
            })
            .collect();
        for variant in Variant::ALL {
            let from = variant.dir_in(&shared);
            let (package, _) = crate::coreml::paths(&from, variant);
            if !is_model_dir(&from) || !package.is_dir() {
                lightcraft_testkit::skip!(Model, "{} and its Core ML package are not in the shared models folder", variant.label());
                continue;
            }
            let dir = std::env::temp_dir().join(format!("lc-birefnet-coreml-{}-{}", variant.dir_name(), std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(from.join(WEIGHTS_FILE), dir.join(WEIGHTS_FILE)).unwrap();
            let mut candle = BiRefNet::load(&dir, variant).unwrap();
            assert_eq!(candle.backend(), "candle", "no package: Automatic is candle");
            let want = candle.matte(&rgb, w, h).unwrap();
            drop(candle);
            std::fs::create_dir_all(dir.join(crate::coreml::COREML_DIR)).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&package, crate::coreml::paths(&dir, variant).0).unwrap();
            let mut coreml = BiRefNet::load_with(&dir, variant, Runtime::CoreMl, Device::Cpu).unwrap();
            assert_eq!(coreml.backend(), "coreml");
            let got = coreml.matte(&rgb, w, h).unwrap();
            assert_eq!((got.side, got.logits.len()), (want.side, want.logits.len()));
            let sig = |v: f32| 1.0 / (1.0 + (-v).exp());
            let diffs: Vec<f32> = got.logits.iter().zip(&want.logits).map(|(a, b)| (sig(*a) - sig(*b)).abs()).collect();
            let mean = diffs.iter().sum::<f32>() / diffs.len() as f32;
            let max = diffs.iter().copied().fold(0.0, f32::max);
            eprintln!("{}: Core ML against candle, matte max |diff| {max:.4}, mean {mean:.6}", variant.label());
            assert!(mean < 0.005, "{}: mean difference {mean}", variant.label());
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
