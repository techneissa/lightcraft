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
