//! Loading the model and running it on a photo.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use candle_core::{Device, Tensor};

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

/// A loaded model.
pub struct BiRefNet {
    net: Net,
    device: Device,
    side: usize,
    cancel: Option<Arc<AtomicBool>>,
}

impl BiRefNet {
    /// Load from a directory holding `model.safetensors`.
    pub fn load(dir: &Path, variant: Variant) -> Result<Self> {
        Self::load_on(dir, variant, best_device())
    }

    /// [`BiRefNet::load`] on the CPU, whatever GPU there is.
    pub fn load_cpu(dir: &Path, variant: Variant) -> Result<Self> {
        Self::load_on(dir, variant, Device::Cpu)
    }

    pub fn load_on(dir: &Path, variant: Variant, device: Device) -> Result<Self> {
        Self::load_with_side(dir, variant.input(), device)
    }

    /// [`BiRefNet::load_on`] with the input side given (tests run the network on small squares).
    pub fn load_with_side(dir: &Path, side: usize, device: Device) -> Result<Self> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        if side == 0 || side % 32 != 0 || side > 4096 {
            return Err(Error::Model(format!("input side {side} is not a multiple of 32 up to 4096")));
        }
        let weights = Arc::new(Weights::open(&dir.join(WEIGHTS_FILE))?);
        let net = Net::load(&Params::new(weights, device.clone()))?;
        Ok(Self { net, device, side, cancel: None })
    }

    /// Install an optional cancellation flag checked between backbone blocks.
    pub fn set_cancel(&mut self, cancel: Option<Arc<AtomicBool>>) {
        self.cancel = cancel;
    }

    pub fn device(&self) -> &Device {
        &self.device
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
    pub fn matte(&self, rgb: &[u8], w: usize, h: usize) -> Result<Matte> {
        let pixels = crate::image::preprocess(rgb, w, h, self.side)?;
        self.matte_pixels(pixels)
    }

    /// The matte of preprocessed pixels (`3 · side²`, channel-first, normalised).
    pub fn matte_pixels(&self, pixels: Vec<f32>) -> Result<Matte> {
        let side = self.side;
        let x = Tensor::from_vec(pixels, (1, 3, side, side), &self.device)?;
        let cancel = || self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
        if cancel() {
            return Err(Error::Cancelled);
        }
        let logits = self.net.forward(&x, &cancel)?;
        let logits = logits.flatten_all()?.to_vec1::<f32>()?;
        if logits.len() != side * side || logits.iter().any(|v| !v.is_finite()) {
            return Err(Error::Model("the model produced an invalid matte".into()));
        }
        Ok(Matte { side, logits })
    }
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
                t.push((format!("bb.layers.{stage}.blocks.{block}.attn.relative_position_bias_table"), vec![(2 * window - 1) * (2 * window - 1), 3 << stage]));
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
}
