//! The frozen image encoder: DINOv2-small (ViT-S/14, 12 layers, 384 wide; Oquab et al., "DINOv2: Learning Robust
//! Visual Features without Supervision", 2023; code and weights Apache-2.0, `facebook/dinov2-small` on Hugging Face).
//!
//! Written from the architecture's description in the paper and the published tensor names; no code was copied.
//! It sees the photo squashed to 224 × 224 (16 × 16 patches; the 37 × 37 position table is resampled once, at
//! load) and returns the class token and the mean of the patch tokens after the last layer norm: 768 numbers that
//! say what the photo shows (a face, a beach, a street), the part of a photographer's choices photo statistics
//! can't see.

use std::path::Path;

use candle_core::{D, DType, Device, Module, Tensor};
use candle_nn::{Conv2d, Conv2dConfig, LayerNorm, Linear, VarBuilder};

use crate::{Error, Result};

/// Side of the square the encoder sees.
pub const INPUT: usize = 224;
const PATCH: usize = 14;
const GRID: usize = INPUT / PATCH;
const HIDDEN: usize = 384;
const HEADS: usize = 6;
const HEAD_DIM: usize = HIDDEN / HEADS;
const LAYERS: usize = 12;
const MLP: usize = 4 * HIDDEN;
const EPS: f64 = 1e-6;
/// Patches per side of the published position table (518 / 14).
const PRETRAIN_GRID: usize = 37;
/// Numbers per photo: the class token, then the mean patch token.
pub const DIMS: usize = 2 * HIDDEN;
/// Largest weights file read (the published one is 88 MB).
const MAX_FILE: u64 = 256 << 20;
/// ImageNet mean and deviation the model was trained with.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

struct Block {
    norm1: LayerNorm,
    qkv: [Linear; 3],
    proj: Linear,
    scale1: Tensor,
    norm2: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    scale2: Tensor,
}

impl Block {
    fn load(vb: VarBuilder) -> Result<Self> {
        let a = vb.pp("attention");
        let lin = |i, o, vb: VarBuilder| candle_nn::linear(i, o, vb);
        Ok(Self {
            norm1: candle_nn::layer_norm(HIDDEN, EPS, vb.pp("norm1"))?,
            qkv: [
                lin(HIDDEN, HIDDEN, a.pp("attention.query"))?,
                lin(HIDDEN, HIDDEN, a.pp("attention.key"))?,
                lin(HIDDEN, HIDDEN, a.pp("attention.value"))?,
            ],
            proj: lin(HIDDEN, HIDDEN, a.pp("output.dense"))?,
            scale1: vb.get(HIDDEN, "layer_scale1.lambda1")?,
            norm2: candle_nn::layer_norm(HIDDEN, EPS, vb.pp("norm2"))?,
            fc1: lin(HIDDEN, MLP, vb.pp("mlp.fc1"))?,
            fc2: lin(MLP, HIDDEN, vb.pp("mlp.fc2"))?,
            scale2: vb.get(HIDDEN, "layer_scale2.lambda1")?,
        })
    }

    /// `x`: `[1, N, 384]`.
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, n, _) = x.dims3()?;
        let h = self.norm1.forward(x)?;
        let split = |l: &Linear| -> Result<Tensor> { Ok(l.forward(&h)?.reshape((b, n, HEADS, HEAD_DIM))?.transpose(1, 2)?.contiguous()?) };
        let (q, k, v) = (split(&self.qkv[0])?, split(&self.qkv[1])?, split(&self.qkv[2])?);
        let att = (q.matmul(&k.t()?)? * (HEAD_DIM as f64).powf(-0.5))?;
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        let o = att.matmul(&v)?.transpose(1, 2)?.reshape((b, n, HIDDEN))?;
        let x = (x + self.proj.forward(&o)?.broadcast_mul(&self.scale1)?)?;
        let h = self.fc2.forward(&self.fc1.forward(&self.norm2.forward(&x)?)?.gelu_erf()?)?;
        Ok((&x + h.broadcast_mul(&self.scale2)?)?)
    }
}

/// A loaded encoder.
pub struct Encoder {
    patch: Conv2d,
    cls: Tensor,
    pos: Tensor,
    blocks: Vec<Block>,
    norm: LayerNorm,
    device: Device,
}

impl Encoder {
    /// Load `model.safetensors` on the best device here (Metal on macOS, else the CPU). A missing, truncated or
    /// foreign file is an error.
    pub fn load(path: &Path) -> Result<Self> {
        let tensors = read_weights(path)?;
        #[cfg(target_os = "macos")]
        if let Ok(d) = Device::new_metal(0) {
            match Self::from_tensors(tensors.clone(), &d) {
                Ok(e) => return Ok(e),
                Err(e) => log::warn!("learn: the encoder does not load on the GPU ({e}); using the CPU"),
            }
        }
        Self::from_tensors(tensors, &Device::Cpu)
    }

    /// Load on the CPU only.
    pub fn load_cpu(path: &Path) -> Result<Self> {
        Self::from_tensors(read_weights(path)?, &Device::Cpu)
    }

    fn from_tensors(tensors: std::collections::HashMap<String, Tensor>, device: &Device) -> Result<Self> {
        let vb = VarBuilder::from_tensors(tensors, DType::F32, device);
        let e = vb.pp("embeddings");
        let cfg = Conv2dConfig { stride: PATCH, ..Default::default() };
        let patch = candle_nn::conv2d(3, HIDDEN, PATCH, cfg, e.pp("patch_embeddings.projection"))?;
        let cls = e.get((1, 1, HIDDEN), "cls_token")?;
        let table = e.get((1, PRETRAIN_GRID * PRETRAIN_GRID + 1, HIDDEN), "position_embeddings")?;
        let pos = resample_positions(&table, device)?;
        let mut blocks = Vec::with_capacity(LAYERS);
        for i in 0..LAYERS {
            blocks.push(Block::load(vb.pp(format!("encoder.layer.{i}")))?);
        }
        let norm = candle_nn::layer_norm(HIDDEN, EPS, vb.pp("layernorm"))?;
        Ok(Self { patch, cls, pos, blocks, norm, device: device.clone() })
    }

    /// Whether the encoder runs on a GPU.
    pub fn on_gpu(&self) -> bool {
        !matches!(self.device, Device::Cpu)
    }

    /// The [`DIMS`] features of an sRGB photo (`rgb`: `width × height × 3` bytes). The photo is squashed to
    /// 224 × 224; any size from 1 × 1 up works.
    pub fn features(&self, width: usize, height: usize, rgb: &[u8]) -> Result<Vec<f32>> {
        let pixels = crate::features::resample_rgb(width, height, rgb, INPUT, INPUT)?;
        let mut planar = vec![0f32; 3 * INPUT * INPUT];
        for (i, px) in pixels.as_chunks::<3>().0.iter().enumerate() {
            for (c, v) in px.iter().enumerate() {
                if let Some(slot) = planar.get_mut(c * INPUT * INPUT + i) {
                    *slot = (v - MEAN[c]) / STD[c];
                }
            }
        }
        let x = Tensor::from_vec(planar, (1, 3, INPUT, INPUT), &self.device)?;
        let x = self.patch.forward(&x)?.flatten_from(2)?.transpose(1, 2)?; // [1, 256, 384]
        let mut x = (Tensor::cat(&[&self.cls, &x], 1)?.broadcast_add(&self.pos))?;
        for b in &self.blocks {
            x = b.forward(&x)?;
        }
        let x = self.norm.forward(&x)?;
        let cls = x.narrow(1, 0, 1)?.flatten_all()?;
        let mean = x.narrow(1, 1, GRID * GRID)?.mean(1)?.flatten_all()?;
        let out = Tensor::cat(&[&cls, &mean], D::Minus1)?.to_dtype(DType::F32)?.to_vec1::<f32>()?;
        if out.len() != DIMS || out.iter().any(|v| !v.is_finite()) {
            return Err(Error::Model("the encoder gave no usable features".into()));
        }
        Ok(out)
    }
}

/// The weights file, read whole (88 MB) and checked: its size, then candle's own header checks.
fn read_weights(path: &Path) -> Result<std::collections::HashMap<String, Tensor>> {
    let meta = std::fs::metadata(path).map_err(|e| Error::Model(format!("{}: {e}", path.display())))?;
    if meta.len() > MAX_FILE {
        return Err(Error::Model(format!("{}: {} bytes is not the encoder's weights", path.display(), meta.len())));
    }
    let bytes = std::fs::read(path).map_err(|e| Error::Model(format!("{}: {e}", path.display())))?;
    candle_core::safetensors::load_buffer(&bytes, &Device::Cpu).map_err(|e| Error::Model(format!("{}: {e}", path.display())))
}

/// The class position and the 37 × 37 patch table resampled to 16 × 16 (antialiased bicubic): `[1, 257, 384]`.
fn resample_positions(table: &Tensor, device: &Device) -> Result<Tensor> {
    let table = table.to_device(&Device::Cpu)?.flatten_all()?.to_vec1::<f32>()?;
    let (cls, grid) = table.split_at_checked(HIDDEN).ok_or_else(|| Error::Model("position table too small".into()))?;
    let w = crate::features::resample_weights(PRETRAIN_GRID, GRID);
    let mut out = cls.to_vec();
    out.reserve(GRID * GRID * HIDDEN);
    let at = |r: usize, c: usize, d: usize| grid.get((r * PRETRAIN_GRID + c) * HIDDEN + d).copied().unwrap_or(0.0);
    for ry in &w {
        for rx in &w {
            for d in 0..HIDDEN {
                let mut v = 0f32;
                for &(r, wr) in ry {
                    for &(c, wc) in rx {
                        v += wr * wc * at(r, c, d);
                    }
                }
                out.push(v);
            }
        }
    }
    Ok(Tensor::from_vec(out, (1, GRID * GRID + 1, HIDDEN), device)?)
}
