//! The FLUX VAE (16 latent channels, 8× smaller), read from the diffusers file FLUX.1 Fill dev ships
//! (`vae/diffusion_pytorch_model.safetensors`).
//!
//! Modified work (MIT OR Apache-2.0): written by the LightCraft contributors in 2026 after candle-transformers 0.9.2's
//! FLUX autoencoder (`src/models/flux/autoencoder.rs`, Copyright The HuggingFace Team and the candle contributors),
//! with the layer names of the diffusers `AutoencoderKL` checkpoint. Changes: every convolution runs in bands of
//! output rows. candle's GPU convolution unrolls its whole input first (im2col); at 768 pixels with 256 channels that
//! is 5.4 GB for one layer, and the decoder's layers together took the process past 40 GB. A convolution is local, so
//! bands with their margin give the same result exactly. See NOTICE.

use std::collections::HashMap;
use std::path::Path;

use candle_core::{D, DType, Device, Module, Result, Tensor};
use candle_nn::{GroupNorm, VarBuilder};

const SCALE: f64 = 0.3611;
const SHIFT: f64 = 0.1159;
const CHANNELS: [usize; 4] = [128, 256, 512, 512];
const LAYERS: usize = 2;
const LATENT: usize = 16;
/// The largest unrolled band a convolution makes, bytes.
const BAND_BYTES: usize = 64 << 20;

thread_local! {
    /// The cancel flag of the fill running on this thread: a convolution asks it between bands.
    static STOP: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> = const { std::cell::RefCell::new(None) };
}

/// Makes the convolutions run on this thread stop (with an error) once `flag` is set, until the guard drops.
pub(crate) fn stop_on(flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) -> StopScope {
    STOP.with(|s| *s.borrow_mut() = flag);
    StopScope
}

pub(crate) struct StopScope;

impl Drop for StopScope {
    fn drop(&mut self) {
        STOP.with(|s| *s.borrow_mut() = None);
    }
}

fn stopped() -> bool {
    STOP.with(|s| s.borrow().as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed)))
}

/// A convolution (square kernel, zero padding) run in bands of output rows.
struct Conv {
    w: Tensor,
    b: Tensor,
    k: usize,
    stride: usize,
    pad: usize,
}

impl Conv {
    fn new(c_in: usize, c_out: usize, k: usize, stride: usize, pad: usize, vb: VarBuilder) -> Result<Self> {
        Ok(Self { w: vb.get((c_out, c_in, k, k), "weight")?, b: vb.get(c_out, "bias")?, k, stride, pad })
    }
}

impl Module for Conv {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        self.run(x, BAND_BYTES)
    }
}

impl Conv {
    /// The convolution, in bands whose unrolled input stays under `budget` bytes.
    fn run(&self, x: &Tensor, budget: usize) -> Result<Tensor> {
        let x = if self.pad > 0 { x.pad_with_zeros(2, self.pad, self.pad)?.pad_with_zeros(3, self.pad, self.pad)? } else { x.clone() };
        let (_, c_in, h, w) = x.dims4()?;
        let (k, s) = (self.k, self.stride);
        if h < k || w < k {
            candle_core::bail!("VAE: a {h}×{w} input is smaller than its {k}×{k} kernel")
        }
        let (h_out, w_out) = ((h - k) / s + 1, (w - k) / s + 1);
        let row_bytes = w_out * c_in * k * k * 4;
        let band = (budget / row_bytes.max(1)).clamp(1, h_out);
        let cfg = candle_nn::Conv2dConfig { padding: 0, stride: s, dilation: 1, groups: 1, cudnn_fwd_algo: None };
        let mut parts = Vec::with_capacity(h_out.div_ceil(band));
        let mut r0 = 0;
        while r0 < h_out {
            if stopped() {
                candle_core::bail!("VAE: stopped between bands")
            }
            let r1 = (r0 + band).min(h_out);
            let rows = x.narrow(2, r0 * s, (r1 - 1) * s + k - r0 * s)?;
            parts.push(rows.conv2d(&self.w, cfg.padding, cfg.stride, cfg.dilation, cfg.groups)?);
            r0 = r1;
        }
        let y = if parts.len() == 1 { parts.remove(0) } else { Tensor::cat(&parts, 2)? };
        y.broadcast_add(&self.b.reshape((1, (), 1, 1))?)
    }
}

fn group_norm(c: usize, vb: VarBuilder) -> Result<GroupNorm> {
    candle_nn::group_norm(32, c, 1e-6, vb)
}

struct Resnet {
    norm1: GroupNorm,
    conv1: Conv,
    norm2: GroupNorm,
    conv2: Conv,
    shortcut: Option<Conv>,
}

impl Resnet {
    fn new(c_in: usize, c_out: usize, vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            norm1: group_norm(c_in, vb.pp("norm1"))?,
            conv1: Conv::new(c_in, c_out, 3, 1, 1, vb.pp("conv1"))?,
            norm2: group_norm(c_out, vb.pp("norm2"))?,
            conv2: Conv::new(c_out, c_out, 3, 1, 1, vb.pp("conv2"))?,
            shortcut: if c_in != c_out { Some(Conv::new(c_in, c_out, 1, 1, 0, vb.pp("conv_shortcut"))?) } else { None },
        })
    }
}

impl Module for Resnet {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let h = x.apply(&self.norm1)?.silu()?.apply(&self.conv1)?.apply(&self.norm2)?.silu()?.apply(&self.conv2)?;
        match &self.shortcut {
            Some(s) => x.apply(s)? + h,
            None => x + h,
        }
    }
}

/// The mid block's single-head self-attention over every pixel.
struct Attention {
    norm: GroupNorm,
    q: candle_nn::Linear,
    k: candle_nn::Linear,
    v: candle_nn::Linear,
    out: candle_nn::Linear,
}

impl Attention {
    fn new(c: usize, vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            norm: group_norm(c, vb.pp("group_norm"))?,
            q: candle_nn::linear(c, c, vb.pp("to_q"))?,
            k: candle_nn::linear(c, c, vb.pp("to_k"))?,
            v: candle_nn::linear(c, c, vb.pp("to_v"))?,
            out: candle_nn::linear(c, c, vb.pp("to_out").pp("0"))?,
        })
    }
}

impl Module for Attention {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, c, h, w) = x.dims4()?;
        let t = x.apply(&self.norm)?.reshape((b, c, h * w))?.transpose(1, 2)?.contiguous()?;
        let (q, k, v) = (t.apply(&self.q)?, t.apply(&self.k)?, t.apply(&self.v)?);
        let att = (q.matmul(&k.t()?)? / (c as f64).sqrt())?;
        let att = candle_nn::ops::softmax_last_dim(&att)?.matmul(&v)?.apply(&self.out)?;
        x + att.transpose(1, 2)?.reshape((b, c, h, w))?
    }
}

struct Mid {
    r1: Resnet,
    attn: Attention,
    r2: Resnet,
}

impl Mid {
    fn new(c: usize, vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            r1: Resnet::new(c, c, vb.pp("resnets").pp(0))?,
            attn: Attention::new(c, vb.pp("attentions").pp(0))?,
            r2: Resnet::new(c, c, vb.pp("resnets").pp(1))?,
        })
    }
}

impl Module for Mid {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        x.apply(&self.r1)?.apply(&self.attn)?.apply(&self.r2)
    }
}

/// One resolution level: its resnets, then a change of size (encoder: halve; decoder: double).
struct Level {
    resnets: Vec<Resnet>,
    resample: Option<Conv>,
}

struct Encoder {
    conv_in: Conv,
    levels: Vec<Level>,
    mid: Mid,
    norm_out: GroupNorm,
    conv_out: Conv,
}

impl Encoder {
    fn new(vb: VarBuilder) -> Result<Self> {
        let mut levels = Vec::new();
        let mut c_in = CHANNELS[0];
        for (i, &c) in CHANNELS.iter().enumerate() {
            let vb_l = vb.pp("down_blocks").pp(i);
            let resnets = (0..LAYERS).map(|j| Resnet::new(if j == 0 { c_in } else { c }, c, vb_l.pp("resnets").pp(j))).collect::<Result<_>>()?;
            // stride 2 after padding one row and column at the bottom and right
            let resample = if i + 1 < CHANNELS.len() { Some(Conv::new(c, c, 3, 2, 0, vb_l.pp("downsamplers").pp(0).pp("conv"))?) } else { None };
            levels.push(Level { resnets, resample });
            c_in = c;
        }
        let top = CHANNELS[CHANNELS.len() - 1];
        Ok(Self {
            conv_in: Conv::new(3, CHANNELS[0], 3, 1, 1, vb.pp("conv_in"))?,
            levels,
            mid: Mid::new(top, vb.pp("mid_block"))?,
            norm_out: group_norm(top, vb.pp("conv_norm_out"))?,
            conv_out: Conv::new(top, 2 * LATENT, 3, 1, 1, vb.pp("conv_out"))?,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut h = x.apply(&self.conv_in)?;
        for l in &self.levels {
            for r in &l.resnets {
                h = h.apply(r)?;
            }
            if let Some(d) = &l.resample {
                h = h.pad_with_zeros(D::Minus1, 0, 1)?.pad_with_zeros(D::Minus2, 0, 1)?.apply(d)?;
            }
        }
        h.apply(&self.mid)?.apply(&self.norm_out)?.silu()?.apply(&self.conv_out)
    }
}

struct Decoder {
    conv_in: Conv,
    mid: Mid,
    levels: Vec<Level>,
    norm_out: GroupNorm,
    conv_out: Conv,
}

impl Decoder {
    fn new(vb: VarBuilder) -> Result<Self> {
        let rev: Vec<usize> = CHANNELS.iter().rev().copied().collect();
        let top = CHANNELS[CHANNELS.len() - 1];
        let mut levels = Vec::new();
        let mut c_in = top;
        for (i, &c) in rev.iter().enumerate() {
            let vb_l = vb.pp("up_blocks").pp(i);
            let resnets = (0..=LAYERS).map(|j| Resnet::new(if j == 0 { c_in } else { c }, c, vb_l.pp("resnets").pp(j))).collect::<Result<_>>()?;
            let resample = if i + 1 < rev.len() { Some(Conv::new(c, c, 3, 1, 1, vb_l.pp("upsamplers").pp(0).pp("conv"))?) } else { None };
            levels.push(Level { resnets, resample });
            c_in = c;
        }
        Ok(Self {
            conv_in: Conv::new(LATENT, top, 3, 1, 1, vb.pp("conv_in"))?,
            mid: Mid::new(top, vb.pp("mid_block"))?,
            levels,
            norm_out: group_norm(CHANNELS[0], vb.pp("conv_norm_out"))?,
            conv_out: Conv::new(CHANNELS[0], 3, 3, 1, 1, vb.pp("conv_out"))?,
        })
    }

    fn forward(&self, z: &Tensor) -> Result<Tensor> {
        let mut h = z.apply(&self.conv_in)?.apply(&self.mid)?;
        for l in &self.levels {
            for r in &l.resnets {
                h = h.apply(r)?;
            }
            if let Some(u) = &l.resample {
                let (_, _, hh, ww) = h.dims4()?;
                h = h.upsample_nearest2d(hh * 2, ww * 2)?.apply(u)?;
            }
        }
        h.apply(&self.norm_out)?.silu()?.apply(&self.conv_out)
    }
}

/// The VAE's encoder and decoder, f32.
pub struct Vae {
    encoder: Encoder,
    decoder: Decoder,
}

impl Vae {
    /// Reads the diffusers VAE file onto `dev`.
    pub fn load(path: &Path, dev: &Device) -> Result<Self> {
        let tensors: HashMap<String, Tensor> = candle_core::safetensors::load(path, &Device::Cpu)?;
        Self::from_tensors(tensors, dev)
    }

    fn from_tensors(tensors: HashMap<String, Tensor>, dev: &Device) -> Result<Self> {
        let vb = VarBuilder::from_tensors(tensors, DType::F32, dev);
        Ok(Self { encoder: Encoder::new(vb.pp("encoder"))?, decoder: Decoder::new(vb.pp("decoder"))? })
    }

    /// Latents (scaled and shifted, as the transformer takes them) of `xs` (`b × 3 × h × w`, −1…1): the mean of
    /// the encoder's distribution, so the same picture always gives the same latents.
    pub fn encode(&self, xs: &Tensor) -> Result<Tensor> {
        let z = self.encoder.forward(xs)?;
        let mean = z.narrow(1, 0, LATENT)?;
        (mean - SHIFT)? * SCALE
    }

    /// The picture (−1…1) of latents made by the transformer.
    pub fn decode(&self, z: &Tensor) -> Result<Tensor> {
        let z = ((z / SCALE)? + SHIFT)?;
        self.decoder.forward(&z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stop asked for between bands ends the convolution with an error, and the flag is gone with its scope.
    #[test]
    fn a_stop_between_bands_ends_the_convolution() {
        let dev = Device::Cpu;
        let x = Tensor::randn(0f32, 1., (1, 3, 23, 17), &dev).unwrap();
        let c =
            Conv { w: Tensor::randn(0f32, 1., (4, 3, 3, 3), &dev).unwrap(), b: Tensor::zeros(4, DType::F32, &dev).unwrap(), k: 3, stride: 1, pad: 1 };
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let _scope = stop_on(Some(flag.clone()));
            assert!(c.run(&x, 1).is_ok(), "not stopped yet");
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            let e = c.run(&x, 1).unwrap_err().to_string();
            assert!(e.contains("stopped"), "{e}");
        }
        assert!(c.run(&x, 1).is_ok(), "the scope ended, so did the stop");
    }

    /// Banded convolution equals one convolution over the whole input, for every stride and padding used here.
    #[test]
    fn bands_equal_the_whole_convolution() {
        let dev = Device::Cpu;
        let x = Tensor::randn(0f32, 1., (1, 3, 23, 17), &dev).unwrap();
        for (k, s, p) in [(3, 1, 1), (3, 2, 0), (1, 1, 0)] {
            let w = Tensor::randn(0f32, 1., (4, 3, k, k), &dev).unwrap();
            let b = Tensor::randn(0f32, 1., 4, &dev).unwrap();
            let whole = x.conv2d(&w, p, s, 1, 1).unwrap().broadcast_add(&b.reshape((1, 4, 1, 1)).unwrap()).unwrap();
            let c = Conv { w: w.clone(), b: b.clone(), k, stride: s, pad: p };
            let one_row = c.run(&x, 1).unwrap();
            let few_rows = c.run(&x, 3 * 17 * k * k * 4 * 4).unwrap();
            let d1 = (&whole - &one_row).unwrap().abs().unwrap().max_all().unwrap().to_scalar::<f32>().unwrap();
            let d2 = (&whole - &few_rows).unwrap().abs().unwrap().max_all().unwrap().to_scalar::<f32>().unwrap();
            assert!(d1 < 1e-4 && d2 < 1e-4, "k{k} s{s} p{p}: {d1} {d2}");
        }
    }

    /// A VAE with weights of the right names and shapes loads, and its encoder and decoder keep the sizes.
    #[test]
    fn layout_loads_and_keeps_sizes() {
        let dev = Device::Cpu;
        let mut names: Vec<(String, Vec<usize>)> = Vec::new();
        let conv = |v: &mut Vec<(String, Vec<usize>)>, n: String, i: usize, o: usize, k: usize| {
            v.push((format!("{n}.weight"), vec![o, i, k, k]));
            v.push((format!("{n}.bias"), vec![o]));
        };
        let gn = |v: &mut Vec<(String, Vec<usize>)>, n: String, c: usize| {
            v.push((format!("{n}.weight"), vec![c]));
            v.push((format!("{n}.bias"), vec![c]));
        };
        let res = |v: &mut Vec<(String, Vec<usize>)>, n: String, i: usize, o: usize| {
            gn(v, format!("{n}.norm1"), i);
            conv(v, format!("{n}.conv1"), i, o, 3);
            gn(v, format!("{n}.norm2"), o);
            conv(v, format!("{n}.conv2"), o, o, 3);
            if i != o {
                conv(v, format!("{n}.conv_shortcut"), i, o, 1);
            }
        };
        let mid = |v: &mut Vec<(String, Vec<usize>)>, n: String, c: usize| {
            res(v, format!("{n}.resnets.0"), c, c);
            res(v, format!("{n}.resnets.1"), c, c);
            gn(v, format!("{n}.attentions.0.group_norm"), c);
            for l in ["to_q", "to_k", "to_v", "to_out.0"] {
                v.push((format!("{n}.attentions.0.{l}.weight"), vec![c, c]));
                v.push((format!("{n}.attentions.0.{l}.bias"), vec![c]));
            }
        };
        conv(&mut names, "encoder.conv_in".into(), 3, 128, 3);
        let mut c_in = 128;
        for (i, &c) in CHANNELS.iter().enumerate() {
            for j in 0..LAYERS {
                res(&mut names, format!("encoder.down_blocks.{i}.resnets.{j}"), if j == 0 { c_in } else { c }, c);
            }
            if i < 3 {
                conv(&mut names, format!("encoder.down_blocks.{i}.downsamplers.0.conv"), c, c, 3);
            }
            c_in = c;
        }
        mid(&mut names, "encoder.mid_block".into(), 512);
        gn(&mut names, "encoder.conv_norm_out".into(), 512);
        conv(&mut names, "encoder.conv_out".into(), 512, 32, 3);
        conv(&mut names, "decoder.conv_in".into(), 16, 512, 3);
        mid(&mut names, "decoder.mid_block".into(), 512);
        let mut c_in = 512;
        for (i, &c) in CHANNELS.iter().rev().enumerate() {
            for j in 0..=LAYERS {
                res(&mut names, format!("decoder.up_blocks.{i}.resnets.{j}"), if j == 0 { c_in } else { c }, c);
            }
            if i < 3 {
                conv(&mut names, format!("decoder.up_blocks.{i}.upsamplers.0.conv"), c, c, 3);
            }
            c_in = c;
        }
        gn(&mut names, "decoder.conv_norm_out".into(), 128);
        conv(&mut names, "decoder.conv_out".into(), 128, 3, 3);
        let tensors = names.into_iter().map(|(n, s)| (n, (Tensor::ones(s, DType::F32, &dev).unwrap() * 0.01).unwrap())).collect();
        let vae = Vae::from_tensors(tensors, &dev).unwrap();
        let x = Tensor::zeros((1, 3, 16, 16), DType::F32, &dev).unwrap();
        let z = vae.encode(&x).unwrap();
        assert_eq!(z.dims(), &[1, 16, 2, 2]);
        assert_eq!(vae.decode(&z).unwrap().dims(), &[1, 3, 16, 16]);
    }
}
