//! RDNet reflection removal: a Rust port of lime-j/RDNet (`models/arch/RDnet_.py`, `decode.py`,
//! `modules_sig.py`, `classifier.py`; Hugging Face Space lime-j/RDNet), the path the Space runs: the
//! ConvNeXt classifier gives six numbers that prompt the network, FocalNet-L supplies the features, four
//! NAF sub-networks refine them and the decoder predicts the reflection, which is subtracted from the photo.
//! The published `fp16_check.pt` and `classifier_32.pt` are read as they are (candle's non-executing pickle
//! reader); no conversion. Only the decoder branch that gives the reflection is run, and only after the last
//! sub-network, which is all the Space uses.
use crate::dirs::read_pickle;
use crate::focal::{classifier, focalnet};
use crate::ops::{R, conv1x1, depthwise3x3, device, input_tensor, layer_norm_eps, linear, norm2d, output_pixels};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use std::collections::HashMap;
use std::path::Path;

pub const NETWORK: &str = "fp16_check.pt";
pub const CLASSIFIER: &str = "classifier_32.pt";
const CHANNELS: [usize; 4] = [64, 128, 256, 512];
const LAYERS: [usize; 4] = [2, 2, 4, 2];
const SUBNETS: usize = 4;

pub struct RdNet {
    vb: VarBuilder<'static>,
    device: Device,
}

impl RdNet {
    /// Reads `fp16_check.pt` and `classifier_32.pt` (both hold their weights under `icnn`) from `dir`.
    pub fn load(dir: &Path, cpu: bool) -> Result<Self, String> {
        let device = device(cpu);
        let mut tensors = HashMap::new();
        read_pickle(&dir.join(NETWORK), "icnn", &device, &mut tensors, |n| Some(n.to_string()))?;
        read_pickle(&dir.join(CLASSIFIER), "icnn", &device, &mut tensors, |n| Some(format!("cls.{n}")))?;
        Self::from_tensors(tensors, device).map_err(|e| format!("unsupported RDNet files: {e}"))
    }

    pub(crate) fn from_tensors(tensors: HashMap<String, Tensor>, device: Device) -> R<Self> {
        Self::from_builder(VarBuilder::from_tensors(tensors, DType::F32, &device), device)
    }

    pub(crate) fn from_builder(vb: VarBuilder<'static>, device: Device) -> R<Self> {
        let net = Self { vb, device };
        // Every tensor the forward pass reads is present with the right shape, before the first run.
        net.forward(&Tensor::zeros((1, 3, 32, 32), DType::F32, &net.device)?).map(|_| ())?;
        Ok(net)
    }

    pub fn infer(&self, rgb: &[u8], w: usize, h: usize) -> Result<Vec<[f32; 3]>, String> {
        if w == 0 || h == 0 || w > crate::MAX_EDGE || h > crate::MAX_EDGE || rgb.len() != w * h * 3 {
            return Err("invalid reflection input dimensions".into());
        }
        let run = || -> R<Vec<[f32; 3]>> { output_pixels(&self.forward(&input_tensor(rgb, w, h, &self.device)?)?, w, h) };
        run().map_err(|e| e.to_string())
    }

    /// `input` is `[1, 3, h, w]` in [0, 1] with h and w multiples of 32; returns the clean picture (input minus
    /// the predicted reflection, not yet clamped).
    pub(crate) fn forward(&self, input: &Tensor) -> R<Tensor> {
        let prompt = self.prompt(&classifier(&self.vb.pp("cls"), input)?)?;
        let (features, stem) = focalnet(&self.vb.pp("baseball"), input)?;
        let adapter = |i: usize, t: &Tensor, cin: usize| conv1x1(&self.vb.pp(format!("baseball_adapter.{i}")), t, cin, cin / 3);
        let stem = adapter(0, &stem, 192)?;
        let mut c = [adapter(1, &features[0], 192)?, adapter(2, &features[1], 384)?, adapter(3, &features[2], 768)?, adapter(4, &features[3], 1536)?];
        drop(features);
        let x = stem.broadcast_mul(&prompt)?;
        for i in 0..SUBNETS {
            c = self.subnet(i, &x, c)?;
        }
        let [c0, c1, c2, c3] = c;
        let dec = self.vb.pp("decoder_blocks.2");
        let x = (self.stage(&dec, 3, &c3)? * &c2)?;
        let x = (self.stage(&dec, 4, &x)? * &c1)?;
        let x = (self.stage(&dec, 5, &x)? * &c0)?;
        let reflection = candle_nn::ops::pixel_shuffle(&conv1x1(&dec.pp("projback_2.0"), &x, 64, 12)?, 2)?;
        input - reflection
    }

    /// `prompt`: Linear, StarReLU, Linear, StarReLU on the classifier's six numbers, as `[1, 64, 1, 1]`.
    fn prompt(&self, alpha: &Tensor) -> R<Tensor> {
        let star = |p: &str, t: &Tensor| -> R<Tensor> {
            let vb = self.vb.pp(p);
            t.relu()?.sqr()?.broadcast_mul(&vb.get(1, "scale")?)?.broadcast_add(&vb.get(1, "bias")?)
        };
        let vb = self.vb.pp("prompt");
        let x = star("prompt.1", &linear(&vb.pp("0"), alpha, 6, 512, true)?)?;
        star("prompt.3", &linear(&vb.pp("2"), &x, 512, CHANNELS[0], true)?)?.reshape((1, CHANNELS[0], 1, 1))
    }

    /// One reversible-column sub-network, run forward: each level adds a scaled copy of its input to the
    /// fused result of the level below and the level above.
    fn subnet(&self, i: usize, x: &Tensor, c: [Tensor; 4]) -> R<[Tensor; 4]> {
        let vb = self.vb.pp(format!("subnet{i}"));
        let alpha = |l: usize| -> R<Tensor> {
            // The reference keeps |alpha| at least 1e-3 (sign kept).
            let a = vb.get((1, CHANNELS[l], 1, 1), &format!("alpha{l}"))?;
            let sign = a.ge(0f32)?.to_dtype(DType::F32)?.affine(2.0, -1.0)?;
            sign * a.abs()?.clamp(1e-3, f64::INFINITY)?
        };
        let first = i == 0;
        let [c0, c1, c2, c3] = c;
        let c0 = (c0.broadcast_mul(&alpha(0)?)? + self.level(&vb, 0, first, x, Some(&c1))?)?;
        let c1 = (c1.broadcast_mul(&alpha(1)?)? + self.level(&vb, 1, first, &c0, Some(&c2))?)?;
        let c2 = (c2.broadcast_mul(&alpha(2)?)? + self.level(&vb, 2, first, &c1, Some(&c3))?)?;
        let c3 = (c3.broadcast_mul(&alpha(3)?)? + self.level(&vb, 3, first, &c2, None)?)?;
        Ok([c0, c1, c2, c3])
    }

    /// `Level`: `Fusion` (downsample the lower level, add the upsampled higher one) then the NAF blocks.
    fn level(&self, sub: &VarBuilder<'static>, l: usize, first_col: bool, down: &Tensor, up: Option<&Tensor>) -> R<Tensor> {
        let vb = sub.pp(format!("level{l}"));
        let fusion = vb.pp("fusion");
        let mut x = if l == 0 {
            down.clone()
        } else {
            let d = fusion.pp("down");
            crate::ops::conv(&d.pp("0"), down, (CHANNELS[l - 1], CHANNELS[l], 2), 2, 0).and_then(|t| norm2d(&d.pp("1"), &t, CHANNELS[l]))?
        };
        if let (false, Some(up)) = (first_col, up) {
            x = (x + self.up(&fusion.pp("up"), up, l)?)?;
        }
        for b in 0..LAYERS[l] {
            x = self.naf(&vb.pp(format!("blocks.{b}")), &x, (CHANNELS[l], CHANNELS[l] * 4))?;
        }
        Ok(x)
    }

    /// `UpSampleConvnext`: per-pixel linear map to the lower level's width, layer norm, ×2 bilinear.
    fn up(&self, vb: &VarBuilder<'static>, x: &Tensor, l: usize) -> R<Tensor> {
        let (_, c, h, w) = x.dims4()?;
        let rs = vb.pp("channel_reschedule");
        let t = x.reshape((c, h * w))?.t()?.contiguous()?;
        let t = layer_norm_eps(&rs.pp("1"), &linear(&rs.pp("0"), &t, c, CHANNELS[l], true)?, CHANNELS[l], 1e-6)?;
        t.t()?.contiguous()?.reshape((1, CHANNELS[l], h, w))?.upsample_bilinear2d(h * 2, w * 2, false)
    }

    /// `NAFBlock` with equal input and output width.
    fn naf(&self, vb: &VarBuilder<'static>, x: &Tensor, spec: (usize, usize)) -> R<Tensor> {
        let (c, e) = spec;
        let half = e / 2;
        let gate = |t: &Tensor| -> R<Tensor> { t.narrow(1, 0, half)?.mul(&t.narrow(1, half, half)?) };
        let y = norm2d(&vb.pp("norm1"), x, c)?;
        let y = gate(&depthwise3x3(&vb.pp("conv2"), &conv1x1(&vb.pp("conv1"), &y, c, e)?, e)?)?;
        let y = y.broadcast_mul(&conv1x1(&vb.pp("sca.1"), &y.mean_keepdim((2, 3))?, half, half)?)?;
        let y = (x + conv1x1(&vb.pp("conv3"), &y, half, c)?.broadcast_mul(&vb.get((1, c, 1, 1), "beta")?)?)?;
        let z = gate(&conv1x1(&vb.pp("conv4"), &norm2d(&vb.pp("norm2"), &y, c)?, c, e)?)?;
        &y + conv1x1(&vb.pp("conv5"), &z, half, c)?.broadcast_mul(&vb.get((1, c, 1, 1), "gamma")?)?
    }

    /// One decoder stage of the reflection branch: 1×1 projection and layer norm, ×2 bilinear (align corners)
    /// and one NAF block.
    fn stage(&self, dec: &VarBuilder<'static>, s: usize, x: &Tensor) -> R<Tensor> {
        let (_, c, h, w) = x.dims4()?;
        let cout = c / 2;
        let proj = dec.pp(format!("proj_layers.{s}"));
        let y = norm2d(&proj.pp("1"), &conv1x1(&proj.pp("0"), x, c, cout)?, cout)?;
        let y = y.upsample_bilinear2d(h * 2, w * 2, true)?;
        self.naf(&dec.pp(format!("normal_layers.{s}.0")), &y, (cout, cout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_weights_are_an_error_not_a_panic() {
        let err = match RdNet::from_tensors(HashMap::new(), Device::Cpu) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("cannot find tensor"), "{err}");
    }

    #[test]
    fn damaged_files_are_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("lc-rdnet-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(RdNet::load(&dir, true).is_err());
        std::fs::write(dir.join(NETWORK), vec![b'x'; 4096]).unwrap();
        std::fs::write(dir.join(CLASSIFIER), b"PK\x03\x04\x14\x00\x00\x00\x00\x00").unwrap();
        assert!(RdNet::load(&dir, true).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folder_needs_both_published_files() {
        let dir = std::env::temp_dir().join(format!("lc-rdnet-have-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!crate::installed_model(&dir, crate::RDNET));
        std::fs::write(dir.join(NETWORK), b"x").unwrap();
        assert!(!crate::installed_model(&dir, crate::RDNET));
        std::fs::write(dir.join(CLASSIFIER), b"x").unwrap();
        assert!(crate::installed_model(&dir, crate::RDNET));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
