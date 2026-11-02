//! The Swin Transformer backbone (Liu et al., 2021; MIT), as BiRefNet uses it: four stages of
//! shifted-window attention, each stage's output normalised and handed to the decoder.
//!
//! Ported from BiRefNet's `swin_v1.py` (MIT, Copyright 2024 Peng Zheng) and Microsoft's Swin
//! detection backbone (MIT, Copyright Microsoft). Activations are NHWC inside a stage.

use candle_core::{D, Tensor};
use candle_nn::{LayerNorm, Linear, Module};

use crate::params::{Conv, ConvOpts, Params};
use crate::{Error, Result};

/// `nn.LayerNorm` default epsilon.
const LN_EPS: f64 = 1e-5;
/// The attention mask value for tokens of different shifted regions.
const MASKED: f32 = -100.0;
/// Largest attention-score block computed at once (elements).
const SCORE_BLOCK: usize = 48 << 20;

/// The backbone's size, read from the weights.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwinConfig {
    pub embed: usize,
    pub depths: Vec<usize>,
    pub heads: Vec<usize>,
    pub window: usize,
}

impl SwinConfig {
    /// Reads the size from tensor names and shapes under `bb.`.
    pub fn detect(p: &Params) -> Result<Self> {
        let bb = p.pp("bb");
        let embed = *bb
            .shape("patch_embed.proj.weight")
            .and_then(|s| s.first())
            .ok_or_else(|| Error::Model("no Swin backbone in this file (bb.patch_embed.proj.weight)".into()))?;
        let mut depths = Vec::new();
        let mut heads = Vec::new();
        let mut window = 0;
        for stage in 0..4 {
            let mut depth = 0;
            while bb.has(&format!("layers.{stage}.blocks.{depth}.norm1.weight")) {
                depth += 1;
            }
            if depth == 0 || depth > 64 {
                return Err(Error::Model(format!("the Swin backbone has {depth} blocks in stage {stage}")));
            }
            let rows = *bb
                .shape(&format!("layers.{stage}.blocks.0.attn.relative_position_bias_table"))
                .and_then(|s| s.first())
                .ok_or_else(|| Error::Model(format!("stage {stage} has no relative position bias")))?;
            let h = bb.shape(&format!("layers.{stage}.blocks.0.attn.relative_position_bias_table")).and_then(|s| s.get(1)).copied().unwrap_or(0);
            // rows = (2w - 1)²
            let side = (rows as f64).sqrt().round() as usize;
            if side * side != rows || side.is_multiple_of(2) {
                return Err(Error::Model(format!("stage {stage}: {rows} relative positions is not (2w-1)²")));
            }
            let w = side.div_ceil(2);
            if window != 0 && window != w {
                return Err(Error::Model("the Swin stages disagree on the window size".into()));
            }
            window = w;
            depths.push(depth);
            heads.push(h);
        }
        if embed == 0 || window == 0 || heads.contains(&0) || embed % heads[0] != 0 {
            return Err(Error::Model("the Swin backbone's sizes are not usable".into()));
        }
        Ok(Self { embed, depths, heads, window })
    }
}

struct Block {
    norm1: LayerNorm,
    qkv: Linear,
    proj: Linear,
    /// `[heads, N, N]`
    bias: Tensor,
    norm2: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    heads: usize,
    shift: usize,
}

/// The relative position index of a `w × w` window: for token pair (a, b) the row of the bias table.
fn relative_index(w: usize) -> Vec<u32> {
    let n = w * w;
    let mut out = Vec::with_capacity(n * n);
    for a in 0..n {
        for b in 0..n {
            let dy = (a / w) as isize - (b / w) as isize + w as isize - 1;
            let dx = (a % w) as isize - (b % w) as isize + w as isize - 1;
            out.push((dy * (2 * w as isize - 1) + dx) as u32);
        }
    }
    out
}

impl Block {
    fn new(p: &Params, dim: usize, heads: usize, window: usize, shift: usize) -> Result<Self> {
        let n = window * window;
        let table = p.pp("attn").tensor("relative_position_bias_table", &[(2 * window - 1) * (2 * window - 1), heads])?;
        let idx = Tensor::from_vec(relative_index(window), n * n, table.device())?;
        let bias = table.index_select(&idx, 0)?.reshape((n, n, heads))?.permute((2, 0, 1))?.contiguous()?;
        Ok(Self {
            norm1: p.pp("norm1").layer_norm(dim, LN_EPS)?,
            qkv: p.pp("attn.qkv").linear(dim, dim * 3, true)?,
            proj: p.pp("attn.proj").linear(dim, dim, true)?,
            bias,
            norm2: p.pp("norm2").layer_norm(dim, LN_EPS)?,
            fc1: p.pp("mlp.fc1").linear(dim, dim * 4, true)?,
            fc2: p.pp("mlp.fc2").linear(dim * 4, dim, true)?,
            heads,
            shift,
        })
    }

    /// Window attention over `[nW, N, C]` windows, a block of windows at a time. `mask` is `[nW, N, N]`.
    fn attention(&self, windows: &Tensor, mask: Option<&Tensor>) -> Result<Tensor> {
        let (nw, n, c) = windows.dims3()?;
        let hd = c / self.heads;
        let scale = (hd as f64).powf(-0.5);
        let per_window = (self.heads * n * n).max(1);
        let group = (SCORE_BLOCK / per_window).clamp(1, nw.max(1));
        let mut outs = Vec::with_capacity(nw.div_ceil(group));
        let mut start = 0;
        while start < nw {
            let g = group.min(nw - start);
            let xs = windows.narrow(0, start, g)?;
            let qkv = self.qkv.forward(&xs)?.reshape((g, n, 3, self.heads, hd))?.permute((2, 0, 3, 1, 4))?;
            let q = (qkv.get(0)? * scale)?;
            let k = qkv.get(1)?;
            let v = qkv.get(2)?;
            let mut att = q.matmul(&k.transpose(D::Minus2, D::Minus1)?.contiguous()?)?;
            att = att.broadcast_add(&self.bias.unsqueeze(0)?)?;
            if let Some(m) = mask {
                att = att.broadcast_add(&m.narrow(0, start, g)?.unsqueeze(1)?)?;
            }
            let att = candle_nn::ops::softmax_last_dim(&att)?;
            outs.push(att.matmul(&v.contiguous()?)?.transpose(1, 2)?.reshape((g, n, c))?);
            start += g;
        }
        let out = if outs.len() == 1 { outs.remove(0) } else { Tensor::cat(&outs, 0)? };
        Ok(self.proj.forward(&out)?)
    }

    /// `x` is `[B, H, W, C]`.
    fn forward(&self, x: &Tensor, window: usize, mask: Option<&Tensor>) -> Result<Tensor> {
        let (b, h, w, c) = x.dims4()?;
        let shortcut = x;
        let mut y = self.norm1.forward(x)?;
        let pad_r = (window - w % window) % window;
        let pad_b = (window - h % window) % window;
        if pad_r > 0 {
            y = y.pad_with_zeros(2, 0, pad_r)?;
        }
        if pad_b > 0 {
            y = y.pad_with_zeros(1, 0, pad_b)?;
        }
        let (hp, wp) = (h + pad_b, w + pad_r);
        let shifted = self.shift > 0;
        if shifted {
            y = y.roll(-(self.shift as i32), 1)?.roll(-(self.shift as i32), 2)?;
        }
        let windows = y.reshape((b, hp / window, window, wp / window, window, c))?.permute((0, 1, 3, 2, 4, 5))?.contiguous()?.reshape((
            b * (hp / window) * (wp / window),
            window * window,
            c,
        ))?;
        let att = self.attention(&windows, if shifted { mask } else { None })?;
        let mut y =
            att.reshape((b, hp / window, wp / window, window, window, c))?.permute((0, 1, 3, 2, 4, 5))?.contiguous()?.reshape((b, hp, wp, c))?;
        if shifted {
            y = y.roll(self.shift as i32, 1)?.roll(self.shift as i32, 2)?;
        }
        if pad_r > 0 || pad_b > 0 {
            y = y.narrow(1, 0, h)?.narrow(2, 0, w)?.contiguous()?;
        }
        let x = (shortcut + y)?;
        let m = self.fc2.forward(&self.fc1.forward(&self.norm2.forward(&x)?)?.gelu_erf()?)?;
        Ok((x + m)?)
    }
}

struct Merging {
    norm: LayerNorm,
    reduction: Linear,
}

impl Merging {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, h, w, c) = x.dims4()?;
        let mut x = x.clone();
        if w % 2 == 1 {
            x = x.pad_with_zeros(2, 0, 1)?;
        }
        if h % 2 == 1 {
            x = x.pad_with_zeros(1, 0, 1)?;
        }
        let (h2, w2) = (x.dim(1)? / 2, x.dim(2)? / 2);
        let x = x.reshape((b, h2, 2, w2, 2, c))?;
        let pick = |i: usize, j: usize| -> Result<Tensor> { Ok(x.narrow(2, i, 1)?.narrow(4, j, 1)?.reshape((b, h2, w2, c))?) };
        let cat = Tensor::cat(&[pick(0, 0)?, pick(1, 0)?, pick(0, 1)?, pick(1, 1)?], D::Minus1)?;
        Ok(self.reduction.forward(&self.norm.forward(&cat)?)?)
    }
}

struct Stage {
    blocks: Vec<Block>,
    merging: Option<Merging>,
    out_norm: LayerNorm,
    dim: usize,
}

/// The shifted-window attention mask `[nW, N, N]` for a `hp × wp` padded feature map.
fn shift_mask(hp: usize, wp: usize, window: usize, shift: usize, dev: &candle_core::Device) -> Result<Tensor> {
    let region = |i: usize, len: usize| -> usize {
        // slices (0, -window), (-window, -shift), (-shift, None)
        if i + window < len {
            0
        } else if i + shift < len {
            1
        } else {
            2
        }
    };
    let (nh, nw) = (hp / window, wp / window);
    let n = window * window;
    let mut out = vec![0f32; nh * nw * n * n];
    for wy in 0..nh {
        for wx in 0..nw {
            let base = (wy * nw + wx) * n * n;
            for a in 0..n {
                let ra = region(wy * window + a / window, hp) * 3 + region(wx * window + a % window, wp);
                for b in 0..n {
                    let rb = region(wy * window + b / window, hp) * 3 + region(wx * window + b % window, wp);
                    if ra != rb
                        && let Some(o) = out.get_mut(base + a * n + b)
                    {
                        *o = MASKED;
                    }
                }
            }
        }
    }
    Ok(Tensor::from_vec(out, (nh * nw, n, n), dev)?)
}

pub struct Swin {
    patch: Conv,
    patch_norm: LayerNorm,
    stages: Vec<Stage>,
    window: usize,
}

impl Swin {
    pub fn load(p: &Params, cfg: &SwinConfig) -> Result<Self> {
        let bb = p.pp("bb");
        let patch = bb.pp("patch_embed.proj").conv(3, cfg.embed, 4, ConvOpts { stride: 4, padding: 0, bias: true }, None)?;
        let patch_norm = bb.pp("patch_embed.norm").layer_norm(cfg.embed, LN_EPS)?;
        let mut stages = Vec::new();
        for (i, (&depth, &heads)) in cfg.depths.iter().zip(&cfg.heads).enumerate() {
            let dim = cfg.embed << i;
            let sp = bb.pp(&format!("layers.{i}"));
            let mut blocks = Vec::with_capacity(depth);
            for j in 0..depth {
                let shift = if j % 2 == 0 { 0 } else { cfg.window / 2 };
                blocks.push(Block::new(&sp.pp(&format!("blocks.{j}")), dim, heads, cfg.window, shift)?);
            }
            let merging = if i + 1 < cfg.depths.len() {
                Some(Merging {
                    norm: sp.pp("downsample.norm").layer_norm(dim * 4, LN_EPS)?,
                    reduction: sp.pp("downsample.reduction").linear(dim * 4, dim * 2, false)?,
                })
            } else {
                None
            };
            let out_norm = bb.pp(&format!("norm{i}")).layer_norm(dim, LN_EPS)?;
            stages.push(Stage { blocks, merging, out_norm, dim });
        }
        Ok(Self { patch, patch_norm, stages, window: cfg.window })
    }

    /// The four stage outputs `[B, C, H, W]` at 1/4, 1/8, 1/16 and 1/32 of the input.
    /// `cancel` is polled between blocks.
    pub fn forward(&self, x: &Tensor, cancel: &dyn Fn() -> bool) -> Result<Vec<Tensor>> {
        let (_, _, h, w) = x.dims4()?;
        // the patch embedding pads to a multiple of 4
        let (ph, pw) = ((4 - h % 4) % 4, (4 - w % 4) % 4);
        let mut x = x.clone();
        if pw > 0 {
            x = x.pad_with_zeros(3, 0, pw)?;
        }
        if ph > 0 {
            x = x.pad_with_zeros(2, 0, ph)?;
        }
        let mut x = self.patch_norm.forward(&self.patch.forward(&x)?.permute((0, 2, 3, 1))?.contiguous()?)?;
        let mut outs = Vec::with_capacity(self.stages.len());
        for stage in &self.stages {
            let (_, sh, sw, _) = x.dims4()?;
            let shift = self.window / 2;
            let mask = if stage.blocks.len() > 1 {
                let hp = sh.div_ceil(self.window) * self.window;
                let wp = sw.div_ceil(self.window) * self.window;
                Some(shift_mask(hp, wp, self.window, shift, x.device())?)
            } else {
                None
            };
            for blk in &stage.blocks {
                if cancel() {
                    return Err(Error::Cancelled);
                }
                x = blk.forward(&x, self.window, mask.as_ref())?;
            }
            outs.push(stage.out_norm.forward(&x)?.permute((0, 3, 1, 2))?.contiguous()?);
            debug_assert_eq!(outs.last().map(|t| t.dim(1).unwrap_or(0)), Some(stage.dim));
            if let Some(m) = &stage.merging {
                x = m.forward(&x)?;
            }
        }
        Ok(outs)
    }
}
