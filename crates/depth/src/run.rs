//! Loading the model and running it on a photo, on candle (Metal on macOS, else the CPU).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use candle_core::{Device, Tensor};

use crate::dpt::Dpt;
use crate::image::{PATCH, input_size, preprocess};
use crate::params::Params;
use crate::vit::Vit;
use crate::weights::Weights;
use crate::{Error, Estimate, Result, WEIGHTS_FILE, is_model_dir};

/// A sky score above this is sky (the model's own `compute_sky_mask` threshold).
pub const SKY: f32 = 0.3;

/// The best device here: Metal on macOS (when a GPU is available), else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

/// A loaded model.
pub struct DepthModel {
    vit: Vit,
    dpt: Dpt,
    device: Device,
    cancel: Option<Arc<AtomicBool>>,
}

impl DepthModel {
    /// Load from a directory holding `model.safetensors`, on the best device.
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_on(dir, best_device())
    }

    /// Load on `device`.
    pub fn load_on(dir: &Path, device: Device) -> Result<Self> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let weights = Arc::new(Weights::open(&dir.join(WEIGHTS_FILE))?);
        let p = Params::new(weights, device.clone()).pp("model");
        let vit = Vit::load(&p.pp("backbone.pretrained"))?;
        let dpt = Dpt::load(&p.pp("head"))?;
        Ok(Self { vit, dpt, device, cancel: None })
    }

    /// Install an optional cancellation flag checked between backbone blocks.
    pub fn set_cancel(&mut self, cancel: Option<Arc<AtomicBool>>) {
        self.cancel = cancel;
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The depth of an 8-bit sRGB image (`w × h`, row-major RGB), processed at long side `long` (see
    /// [`crate::image::PROCESS_LONG`]).
    pub fn estimate(&self, rgb: &[u8], w: usize, h: usize, long: usize) -> Result<Estimate> {
        let (iw, ih) = input_size(w, h, long)?;
        let pixels = preprocess(rgb, w, h, (iw, ih))?;
        let flag = self.cancel.clone();
        let cancel = move || flag.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
        let x = Tensor::from_vec(pixels, (1, 3, ih, iw), &self.device)?;
        let feats = self.vit.forward(&x, &cancel)?;
        if cancel() {
            return Err(Error::Cancelled);
        }
        let (depth, sky) = self.dpt.forward(&feats, (ih / PATCH, iw / PATCH))?;
        let depth = depth.flatten_all()?.to_vec1::<f32>()?;
        let sky = sky.flatten_all()?.to_vec1::<f32>()?;
        finish(iw, ih, depth, sky)
    }
}

/// The model's output checked and post-processed as its reference does: the sky gets the 99th percentile of the
/// depth outside it (when each of the two has more than ten pixels).
pub fn finish(width: usize, height: usize, mut depth: Vec<f32>, sky: Vec<f32>) -> Result<Estimate> {
    let n = width.checked_mul(height).ok_or_else(|| Error::Model("output too large".into()))?;
    if n == 0 || depth.len() != n || sky.len() != n {
        return Err(Error::Model(format!("the model gave {} depths and {} sky scores for {width} × {height}", depth.len(), sky.len())));
    }
    if depth.iter().any(|d| !d.is_finite() || *d <= 0.0) || sky.iter().any(|s| !s.is_finite()) {
        return Err(Error::Model("the model produced an invalid depth map".into()));
    }
    let mut ground: Vec<f32> = depth.iter().zip(&sky).filter(|(_, s)| **s < SKY).map(|(d, _)| *d).collect();
    let skies = n - ground.len();
    if ground.len() > 10 && skies > 10 {
        ground.sort_by(f32::total_cmp);
        let at = ((ground.len() - 1) as f64 * 0.99).round() as usize;
        if let Some(far) = ground.get(at).copied() {
            depth.iter_mut().zip(&sky).filter(|(_, s)| **s >= SKY).for_each(|(d, _)| *d = far);
        }
    }
    Ok(Estimate { width, height, depth, sky })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_takes_the_far_end_of_the_ground() {
        let (w, h) = (20, 10);
        let depth: Vec<f32> = (0..w * h).map(|i| 1.0 + (i % w) as f32).collect();
        // the top three rows are sky, with a depth of 1000 the model made up
        let sky: Vec<f32> = (0..w * h).map(|i| if i / w < 3 { 0.9 } else { 0.0 }).collect();
        let mut d = depth.clone();
        d.iter_mut().take(3 * w).for_each(|v| *v = 1000.0);
        let e = finish(w, h, d, sky).unwrap();
        assert!(e.depth[..3 * w].iter().all(|v| (*v - 20.0).abs() < 1e-6), "{:?}", &e.depth[..w]);
        assert_eq!(&e.depth[3 * w..], &depth[3 * w..]);
    }

    #[test]
    fn broken_output_is_an_error() {
        assert!(finish(2, 2, vec![1.0; 3], vec![0.0; 4]).is_err());
        assert!(finish(2, 2, vec![1.0, f32::NAN, 1.0, 1.0], vec![0.0; 4]).is_err());
        assert!(finish(2, 2, vec![1.0, 0.0, 1.0, 1.0], vec![0.0; 4]).is_err());
        assert!(finish(0, 2, vec![], vec![]).is_err());
    }

    #[test]
    fn no_model_is_an_error() {
        let empty = std::env::temp_dir().join(format!("lc-depth-none-{}", std::process::id()));
        assert!(matches!(DepthModel::load_on(&empty, Device::Cpu), Err(Error::Missing(_))));
    }
}
