//! The backbone: DINOv2 ViT-L/14 as Depth Anything 3 Mono uses it (no camera token, no RoPE, no QK norm, no
//! register tokens): patch embedding, a class token, the learned position embedding resized to the photo's patch
//! grid, 24 pre-norm blocks with layer scale, and the final norm applied to the four blocks the head reads.

use candle_core::{D, Module, Tensor};
use candle_nn::{LayerNorm, Linear};

use crate::params::{Conv, Params};
use crate::{Error, Result};

/// ViT-L's width, depth and heads.
pub const DIM: usize = 1024;
const DEPTH: usize = 24;
const HEADS: usize = 16;
/// The blocks whose outputs the head reads (`out_layers` in the model's config).
pub const OUT_LAYERS: [usize; 4] = [4, 11, 17, 23];
/// The position embedding's grid: 37 × 37 patches (518 pixels) plus the class token.
const POS_GRID: usize = 37;
/// DINOv2's offset when it resizes the position embedding (`interpolate_offset`).
const POS_OFFSET: f64 = 0.1;

struct Block {
    norm1: LayerNorm,
    qkv: Linear,
    proj: Linear,
    ls1: Tensor,
    norm2: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    ls2: Tensor,
}

impl Block {
    fn load(p: &Params) -> Result<Self> {
        Ok(Self {
            // (the blocks' norms use 1e-6; the final norm PyTorch's default 1e-5)
            norm1: p.pp("norm1").layer_norm(DIM, 1e-6)?,
            qkv: p.pp("attn.qkv").linear(DIM, 3 * DIM)?,
            proj: p.pp("attn.proj").linear(DIM, DIM)?,
            ls1: p.tensor("ls1.gamma", &[DIM])?,
            norm2: p.pp("norm2").layer_norm(DIM, 1e-6)?,
            fc1: p.pp("mlp.fc1").linear(DIM, 4 * DIM)?,
            fc2: p.pp("mlp.fc2").linear(4 * DIM, DIM)?,
            ls2: p.tensor("ls2.gamma", &[DIM])?,
        })
    }

    fn attention(&self, x: &Tensor) -> Result<Tensor> {
        let (b, n, _) = x.dims3()?;
        let hd = DIM / HEADS;
        let qkv = self.qkv.forward(x)?.reshape((b, n, 3, HEADS, hd))?.permute((2, 0, 3, 1, 4))?;
        let q = (qkv.get(0)?.contiguous()? * (hd as f64).powf(-0.5))?;
        let k = qkv.get(1)?.contiguous()?;
        let v = qkv.get(2)?.contiguous()?;
        let attn = candle_nn::ops::softmax_last_dim(&q.matmul(&k.t()?.contiguous()?)?)?;
        let y = attn.matmul(&v)?.transpose(1, 2)?.reshape((b, n, DIM))?;
        Ok(self.proj.forward(&y)?)
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let a = self.attention(&self.norm1.forward(x)?)?.broadcast_mul(&self.ls1)?;
        let x = (x + a)?;
        let m = self.fc2.forward(&self.fc1.forward(&self.norm2.forward(&x)?)?.gelu_erf()?)?.broadcast_mul(&self.ls2)?;
        Ok((x + m)?)
    }
}

pub struct Vit {
    patch: Conv,
    cls: Tensor,
    /// `[1 + 37², DIM]` on the CPU, resized per input in [`Vit::pos_embed`].
    pos: Vec<f32>,
    blocks: Vec<Block>,
    norm: LayerNorm,
}

impl Vit {
    pub fn load(p: &Params) -> Result<Self> {
        let n = 1 + POS_GRID * POS_GRID;
        let pos = p.tensor_cpu("pos_embed", &[1, n, DIM])?.flatten_all()?.to_vec1::<f32>()?;
        let blocks = (0..DEPTH).map(|i| Block::load(&p.pp(&format!("blocks.{i}")))).collect::<Result<Vec<_>>>()?;
        Ok(Self {
            patch: p.pp("patch_embed.proj").conv(3, DIM, 14, (14, 0), true)?,
            cls: p.tensor("cls_token", &[1, 1, DIM])?,
            pos,
            blocks,
            norm: p.pp("norm").layer_norm(DIM, 1e-5)?,
        })
    }

    /// The position embedding for a grid of `gh × gw` patches: the class token's, then the 37 × 37 grid's resized
    /// bicubically with DINOv2's offset (`F.interpolate(scale_factor=((gh + 0.1) / 37, (gw + 0.1) / 37))`, A = −0.75,
    /// border samples repeated), row-major.
    fn pos_embed(&self, gh: usize, gw: usize) -> Vec<f32> {
        let m = POS_GRID;
        // (the grid it was trained at is used as it is, as DINOv2 does)
        if (gh, gw) == (m, m) {
            return self.pos.clone();
        }
        let mut out = vec![0f32; (1 + gh * gw) * DIM];
        if let (Some(dst), Some(src)) = (out.get_mut(..DIM), self.pos.get(..DIM)) {
            dst.copy_from_slice(src);
        }
        let taps = |dst: usize, scale: f64| -> Vec<([usize; 4], [f64; 4])> {
            (0..dst)
                .map(|o| {
                    let s = (o as f64 + 0.5) / scale - 0.5;
                    let f = s.floor();
                    let t = s - f;
                    let a = -0.75;
                    let w1 = |x: f64| ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0;
                    let w2 = |x: f64| ((a * x - 5.0 * a) * x + 8.0 * a) * x - 4.0 * a;
                    let ws = [w2(t + 1.0), w1(t), w1(1.0 - t), w2(2.0 - t)];
                    let idx = [-1.0, 0.0, 1.0, 2.0].map(|d: f64| (f + d).clamp(0.0, (m - 1) as f64) as usize);
                    (idx, ws)
                })
                .collect()
        };
        let (ty, tx) = (taps(gh, (gh as f64 + POS_OFFSET) / m as f64), taps(gw, (gw as f64 + POS_OFFSET) / m as f64));
        let mut acc = vec![0f64; DIM];
        for (oy, (iy, wy)) in ty.iter().enumerate() {
            for (ox, (ix, wx)) in tx.iter().enumerate() {
                acc.iter_mut().for_each(|v| *v = 0.0);
                for (r, w_r) in iy.iter().zip(wy) {
                    for (c, w_c) in ix.iter().zip(wx) {
                        let w = w_r * w_c;
                        let at = (1 + r * m + c) * DIM;
                        if let Some(src) = self.pos.get(at..at + DIM) {
                            acc.iter_mut().zip(src).for_each(|(a, s)| *a += w * f64::from(*s));
                        }
                    }
                }
                let at = (1 + oy * gw + ox) * DIM;
                if let Some(dst) = out.get_mut(at..at + DIM) {
                    dst.iter_mut().zip(&acc).for_each(|(d, a)| *d = *a as f32);
                }
            }
        }
        out
    }

    /// The patch tokens after the final norm at each of [`OUT_LAYERS`], `[1, gh · gw, DIM]` each, for input
    /// `x` `[1, 3, 14 gh, 14 gw]`. `cancel` is checked before each block.
    pub fn forward(&self, x: &Tensor, cancel: &dyn Fn() -> bool) -> Result<Vec<Tensor>> {
        let (_, _, h, w) = x.dims4()?;
        let (gh, gw) = (h / 14, w / 14);
        let n = gh * gw;
        let tokens = self.patch.forward(x)?.flatten_from(2)?.transpose(1, 2)?;
        let pos = Tensor::from_vec(self.pos_embed(gh, gw), (1, 1 + n, DIM), x.device())?;
        let mut t = (Tensor::cat(&[&self.cls, &tokens], 1)? + pos)?;
        let mut out = Vec::with_capacity(OUT_LAYERS.len());
        for (i, b) in self.blocks.iter().enumerate() {
            if cancel() {
                return Err(Error::Cancelled);
            }
            t = b.forward(&t)?;
            if OUT_LAYERS.contains(&i) {
                out.push(self.norm.forward(&t)?.narrow(1, 1, n)?);
            }
        }
        if out.len() != OUT_LAYERS.len() || out.iter().any(|o| o.dim(D::Minus1).ok() != Some(DIM)) {
            return Err(Error::Model("the backbone gave the wrong features".into()));
        }
        Ok(out)
    }
}
