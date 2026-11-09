//! FLUX.1 Fill: repaint the masked part of a square picture.
//!
//! The steps follow the published FLUX.1 Fill pipeline: the picture with the hole greyed out is encoded by the VAE,
//! the mask is folded 8 × 8 into 64 channels per latent pixel, both are packed 2 × 2 like the latents, and every
//! step the transformer sees the noisy latents beside them (64 + 64 + 256 = 384 channels). Rectified flow from
//! noise to picture, Euler steps on the time-shifted schedule of FLUX.1 dev, guidance embedded (no second pass).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use candle_core::{DType, Device, Tensor};
use candle_transformers::models::flux::sampling;
use candle_transformers::quantized_var_builder::VarBuilder;

use crate::lora::LoraSet;
use crate::model::{Config, Flux};
use crate::text::Prompt;
use crate::vae::Vae;
use crate::{Error, Result, Weights};

/// The side of the square a picture is repainted at must be a multiple of this (VAE 8 × packing 2).
pub const MULTIPLE: usize = 16;
/// The largest side accepted (memory grows with its square; the trials used 768).
pub const MAX_SIDE: usize = 1536;

/// What to repaint.
pub struct Request<'a> {
    /// `side × side` RGB, interleaved, 0…1 (display-encoded, as the model was trained on).
    pub rgb: &'a [f32],
    /// `side × side`, 1 = repaint, 0 = keep (cut at ½, as the pipeline does).
    pub mask: &'a [f32],
    pub side: usize,
    /// Denoising steps (8 with the Turbo LoRA, about 28 without).
    pub steps: usize,
    /// The embedded guidance (FLUX.1 Fill: 30).
    pub guidance: f64,
    pub seed: u64,
    /// Set by the caller to stop: checked between steps and between the VAE's convolution bands.
    pub cancel: Option<Arc<AtomicBool>>,
}

/// The loaded transformer (with its LoRA) and VAE.
pub struct FillModel {
    flux: Flux,
    vae: Vae,
    device: Device,
}

impl FillModel {
    /// Loads the GGUF transformer, the LoRA when there is one, and the VAE onto `dev`.
    pub fn load(w: &Weights, dev: &Device) -> Result<Self> {
        let cfg = Config::fill();
        let lora = match &w.lora {
            Some(p) => LoraSet::load(p, cfg.depth, cfg.depth_single_blocks)?,
            None => LoraSet::none(),
        };
        log::info!("FLUX LoRA: {} layers", lora.len());
        let flux = load_flux(&w.transformer, &cfg, &lora, dev)?;
        drop(lora);
        let vae = Vae::load(&w.vae, dev)?;
        Ok(Self { flux, vae, device: dev.clone() })
    }

    /// Repaints the masked part of `req.rgb`; gives the whole picture back (`side × side` RGB, 0…1), the kept part
    /// as the VAE gives it back (the caller pastes only the repainted part). `progress(done, total)` is called after
    /// each step and once before the first; returning `false` stops with [`Error::Cancelled`].
    pub fn fill(&self, req: &Request<'_>, prompt: &Prompt, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<Vec<f32>> {
        let s = req.side;
        if s == 0 || !s.is_multiple_of(MULTIPLE) || s > MAX_SIDE {
            return Err(Error::Input(format!("side {s} is not a multiple of {MULTIPLE} up to {MAX_SIDE}")));
        }
        let px = s * s;
        if req.rgb.len() != px * 3 || req.mask.len() != px {
            return Err(Error::Input(format!("{} values of RGB and {} of mask for a {s}² picture", req.rgb.len(), req.mask.len())));
        }
        if req.steps == 0 || req.steps > 100 || !req.guidance.is_finite() {
            return Err(Error::Input(format!("{} steps, guidance {}", req.steps, req.guidance)));
        }
        if prompt.t5.len() != prompt.tokens * 4096 || prompt.clip.len() != 768 {
            return Err(Error::Input("prompt embeddings of the wrong size".into()));
        }
        let total = req.steps;
        if !progress(0, total) {
            return Err(Error::Cancelled);
        }
        let dev = &self.device;
        let _scope = crate::vae::stop_on(req.cancel.clone());
        // a convolution that stopped between bands reports an error; to the caller it is a cancel
        let cancelled = || req.cancel.as_ref().is_some_and(|f| f.load(Ordering::Relaxed));
        let or_cancel = |e: Error| if cancelled() { Error::Cancelled } else { e };
        let (img_cond, h, w) = self.condition(req).map_err(or_cancel)?;
        let noise = noise(req.seed, 16 * h * w);
        let noise = Tensor::from_vec(noise, (1, 16, h, w), dev)?;
        let state_img = pack(&noise)?;
        let (_, l, _) = state_img.dims3()?;
        let img_ids = img_ids(h, w, dev)?;
        let txt = Tensor::from_slice(&prompt.t5, (1, prompt.tokens, 4096), dev)?;
        let txt_ids = Tensor::zeros((1, prompt.tokens, 3), DType::F32, dev)?;
        let vec_ = Tensor::from_slice(&prompt.clip, (1, 768), dev)?;
        let guidance = Tensor::full(req.guidance as f32, 1, dev)?;
        let timesteps = sampling::get_schedule(req.steps, Some((l, 0.5, 1.15)));
        let mut img = state_img;
        for (i, win) in timesteps.windows(2).enumerate() {
            let (t_curr, t_prev) = match win {
                [a, b] => (*a, *b),
                _ => continue,
            };
            let t_vec = Tensor::full(t_curr as f32, 1, dev)?;
            let x = Tensor::cat(&[&img, &img_cond], 2)?;
            let pred = self.flux.forward(&x, &img_ids, &txt, &txt_ids, &t_vec, &vec_, Some(&guidance))?;
            img = (img + (pred * (t_prev - t_curr))?)?;
            dev.synchronize()?;
            if !progress(i + 1, total) {
                return Err(Error::Cancelled);
            }
        }
        let latents = sampling::unpack(&img, s, s)?;
        let out = self.vae.decode(&latents).map_err(|e| or_cancel(e.into()))?;
        let out = ((out.clamp(-1f32, 1f32)? + 1.0)? * 0.5)?;
        let out = out.squeeze(0)?.permute((1, 2, 0))?.flatten_all()?.to_dtype(DType::F32)?.to_vec1::<f32>()?;
        if out.len() != px * 3 {
            return Err(Error::Compute(format!("the VAE gave {} values for {s}²", out.len())));
        }
        Ok(out)
    }

    /// The masked picture's latents and the folded mask, packed side by side (`1 × L × 320`), and the latent size.
    fn condition(&self, req: &Request<'_>) -> Result<(Tensor, usize, usize)> {
        let dev = &self.device;
        let s = req.side;
        let (h, w) = (s / 8, s / 8);
        let mask: Vec<f32> = req.mask.iter().map(|m| if *m >= 0.5 { 1.0 } else { 0.0 }).collect();
        let m = Tensor::from_vec(mask, (1, 1, s, s), dev)?;
        let rgb: Vec<f32> = req.rgb.iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) * 2.0 - 1.0 } else { 0.0 }).collect();
        let img = Tensor::from_vec(rgb, (s, s, 3), dev)?.permute((2, 0, 1))?.unsqueeze(0)?.contiguous()?;
        let masked = img.broadcast_mul(&(1.0 - &m)?)?;
        let latents = pack(&self.vae.encode(&masked)?)?;
        // b × 1 × (h·8) × (w·8) → b × 64 × h × w (8 × 8 pixels per latent pixel), then packed like the latents
        let folded = m.reshape((1, h, 8, w, 8))?.permute((0, 2, 4, 1, 3))?.reshape((1, 64, h, w))?;
        let folded = pack(&folded)?;
        Ok((Tensor::cat(&[latents, folded], 2)?, h, w))
    }
}

fn load_flux(path: &Path, cfg: &Config, lora: &LoraSet, dev: &Device) -> Result<Flux> {
    // the header is checked against the file's size before candle allocates anything from it
    crate::gguf::check(path)?;
    let vb = VarBuilder::from_gguf(path, dev)?;
    if !vb.contains_key("img_in.weight") || !vb.contains_key("double_blocks.0.img_attn.qkv.weight") {
        return Err(Error::Model(format!("{} is not a FLUX.1 GGUF", path.display())));
    }
    let shape = vb.get_no_shape("img_in.weight")?.shape().dims().to_vec();
    if shape.last() != Some(&cfg.in_channels) {
        return Err(Error::Model(format!("{} is not FLUX.1 Fill (input layer {shape:?})", path.display())));
    }
    Ok(Flux::new(cfg, vb, lora)?)
}

/// `b × c × h × w` → `b × (h/2·w/2) × (c·4)`, 2 × 2 patches.
fn pack(x: &Tensor) -> candle_core::Result<Tensor> {
    let (b, c, h, w) = x.dims4()?;
    x.reshape((b, c, h / 2, 2, w / 2, 2))?.permute((0, 2, 4, 1, 3, 5))?.reshape((b, (h / 2) * (w / 2), c * 4))
}

/// The packed tokens' positions: (0, row, column).
fn img_ids(h: usize, w: usize, dev: &Device) -> candle_core::Result<Tensor> {
    let (h2, w2) = (h / 2, w / 2);
    let mut ids = Vec::with_capacity(h2 * w2 * 3);
    for r in 0..h2 {
        for c in 0..w2 {
            ids.extend_from_slice(&[0.0, r as f32, c as f32]);
        }
    }
    Tensor::from_vec(ids, (1, h2 * w2, 3), dev)
}

/// `n` standard normal values from `seed` (the same on every device and machine).
pub(crate) fn noise(seed: u64, n: usize) -> Vec<f32> {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        // splitmix64
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut out = Vec::with_capacity(n + 1);
    while out.len() < n {
        // Box–Muller on two uniforms in (0, 1]
        let u1 = ((next() >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
        let u2 = (next() >> 11) as f64 / (1u64 << 53) as f64;
        let r = (-2.0 * u1.ln()).sqrt();
        let a = std::f64::consts::TAU * u2;
        out.push((r * a.cos()) as f32);
        out.push((r * a.sin()) as f32);
    }
    out.truncate(n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_seeded_and_standard() {
        let a = noise(7, 10_000);
        assert_eq!(a, noise(7, 10_000));
        assert_ne!(a, noise(8, 10_000));
        let mean = a.iter().map(|v| f64::from(*v)).sum::<f64>() / a.len() as f64;
        let var = a.iter().map(|v| (f64::from(*v) - mean).powi(2)).sum::<f64>() / a.len() as f64;
        assert!(mean.abs() < 0.05 && (var - 1.0).abs() < 0.05, "{mean} {var}");
        assert!(a.iter().all(|v| v.is_finite()));
    }

    /// Packing matches the reference layout: token (r, c) holds channel-major 2 × 2 patches.
    #[test]
    fn packing_and_unpacking_agree() {
        let dev = Device::Cpu;
        let x = Tensor::arange(0f32, 2.0 * 4.0 * 4.0, &dev).unwrap().reshape((1, 2, 4, 4)).unwrap();
        let p = pack(&x).unwrap();
        assert_eq!(p.dims(), &[1, 4, 8]);
        // token 0 = rows 0-1, cols 0-1: channel 0 → 0, 1, 4, 5; channel 1 → 16, 17, 20, 21
        assert_eq!(p.get(0).unwrap().get(0).unwrap().to_vec1::<f32>().unwrap(), vec![0., 1., 4., 5., 16., 17., 20., 21.]);
        let back = sampling::unpack(&p, 32, 32).unwrap();
        assert_eq!(back.flatten_all().unwrap().to_vec1::<f32>().unwrap(), x.flatten_all().unwrap().to_vec1::<f32>().unwrap());
    }

    /// The mask fold puts the 8 × 8 pixels of a latent pixel in its 64 channels, row-major.
    #[test]
    fn mask_fold_is_per_latent_pixel() {
        let dev = Device::Cpu;
        let s = 16;
        let mut m = vec![0f32; s * s];
        m[3 * s + 9] = 1.0; // pixel (row 3, col 9): latent pixel (0, 1), sub-position (3, 1)
        let t = Tensor::from_vec(m, (1, 1, s, s), &dev).unwrap();
        let f = t.reshape((1, 2, 8, 2, 8)).unwrap().permute((0, 2, 4, 1, 3)).unwrap().reshape((1, 64, 2, 2)).unwrap();
        let v = f.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let at = |ch: usize, r: usize, c: usize| v[ch * 4 + r * 2 + c];
        assert_eq!(at(3 * 8 + 1, 0, 1), 1.0);
        assert_eq!(v.iter().sum::<f32>(), 1.0);
    }

    #[test]
    fn img_ids_are_rows_and_columns() {
        let ids = img_ids(4, 6, &Device::Cpu).unwrap();
        assert_eq!(ids.dims(), &[1, 6, 3]);
        assert_eq!(ids.get(0).unwrap().get(4).unwrap().to_vec1::<f32>().unwrap(), vec![0., 1., 1.]);
    }
}
