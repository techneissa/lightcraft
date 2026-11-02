//! Modulated deformable convolution (DCNv2), the operator torchvision's `deform_conv2d` computes
//! and BiRefNet's decoder uses in its ASPP blocks. Candle has none, so it is built from a
//! gather: each kernel tap reads the input at its own sub-pixel position (the regular position
//! plus a learned offset), blends the four neighbours bilinearly (zero outside the map), scales by
//! a learned modulator, and a matrix product with the kernel sums the taps.
//!
//! Ported from BiRefNet's `deform_conv.py` (MIT, Copyright 2024 Peng Zheng); the sampling follows
//! torchvision's definition (BSD-3-Clause), reimplemented here.

use candle_core::{DType, Device, Tensor};
use candle_nn::{Module, ops::sigmoid};

use crate::Result;
use crate::params::{Conv, ConvOpts, Params};

/// Gathered values computed at once (elements): bounds the temporary memory of the 7×7 blocks.
const GATHER_BLOCK: usize = 48 << 20;

pub struct DeformConv {
    offset: Conv,
    modulator: Conv,
    /// `[k·k·cin, cout]`, rows ordered tap-major (matches the gathered columns).
    weight: Tensor,
    bias: Option<Tensor>,
    k: usize,
    cin: usize,
}

impl DeformConv {
    /// `p` holds `offset_conv`, `modulator_conv` and `regular_conv`; `bn` is the batch norm after
    /// the convolution (folded into `regular_conv`).
    pub fn load(p: &Params, cin: usize, cout: usize, k: usize, bn: Option<&Params>) -> Result<Self> {
        let pad = k / 2;
        let opts = ConvOpts { stride: 1, padding: pad, bias: true };
        let offset = p.pp("offset_conv").conv(cin, 2 * k * k, k, opts, None)?;
        let modulator = p.pp("modulator_conv").conv(cin, k * k, k, opts, None)?;
        let regular = p.pp("regular_conv").conv(cin, cout, k, ConvOpts { stride: 1, padding: pad, bias: false }, bn)?;
        let weight = regular.w.permute((2, 3, 1, 0))?.reshape((k * k * cin, cout))?.contiguous()?;
        Ok(Self { offset, modulator, weight, bias: regular.b, k, cin })
    }

    /// `x` is `[1, cin, H, W]`; the result `[1, cout, H, W]` (stride 1, "same" padding).
    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, c, h, w) = x.dims4()?;
        if b != 1 || c != self.cin {
            return Err(crate::Error::Model(format!("deformable convolution wants [1, {}, H, W], got {:?}", self.cin, x.dims())));
        }
        let hw = h * w;
        let kk = self.k * self.k;
        let offsets = self.offset.forward(x)?.reshape((kk, 2, hw))?;
        let modulator = (sigmoid(&self.modulator.forward(x)?)? * 2.0)?.reshape((kk, hw))?;
        // pixels as rows, so a gathered row is one pixel's channels
        let rows = x.reshape((c, hw))?.t()?.contiguous()?;
        let (base_y, base_x) = base_grid(self.k, h, w, x.device())?;
        let per_tap = (4 * hw * c).max(1);
        let chunk = (GATHER_BLOCK / per_tap).clamp(1, kk);
        let mut acc: Option<Tensor> = None;
        let mut t0 = 0;
        while t0 < kk {
            let n = chunk.min(kk - t0);
            let dy = offsets.narrow(0, t0, n)?.narrow(1, 0, 1)?.reshape((n, hw))?;
            let dx = offsets.narrow(0, t0, n)?.narrow(1, 1, 1)?.reshape((n, hw))?;
            let y = (base_y.narrow(0, t0, n)? + dy)?;
            let xx = (base_x.narrow(0, t0, n)? + dx)?;
            let m = modulator.narrow(0, t0, n)?;
            let sampled = sample(&rows, &y, &xx, &m, h, w)?; // [n, hw, c]
            let sampled = sampled.permute((1, 0, 2))?.contiguous()?.reshape((hw, n * c))?;
            let part = sampled.matmul(&self.weight.narrow(0, t0 * c, n * c)?)?;
            acc = Some(match acc {
                Some(a) => (a + part)?,
                None => part,
            });
            t0 += n;
        }
        let Some(out) = acc else { return Err(crate::Error::Model("deformable convolution without taps".into())) };
        let out = match &self.bias {
            Some(bias) => out.broadcast_add(&bias.reshape((1, bias.dim(0)?))?)?,
            None => out,
        };
        Ok(out.t()?.reshape((1, out.dim(1)?, h, w))?)
    }
}

/// The regular (offset-free) sample position of each tap and pixel: `[k², H·W]` rows and columns.
fn base_grid(k: usize, h: usize, w: usize, dev: &Device) -> Result<(Tensor, Tensor)> {
    let pad = (k / 2) as f32;
    let mut ys = Vec::with_capacity(k * k * h * w);
    let mut xs = Vec::with_capacity(k * k * h * w);
    for i in 0..k {
        for j in 0..k {
            for oy in 0..h {
                for ox in 0..w {
                    ys.push(oy as f32 + i as f32 - pad);
                    xs.push(ox as f32 + j as f32 - pad);
                }
            }
        }
    }
    Ok((Tensor::from_vec(ys, (k * k, h * w), dev)?, Tensor::from_vec(xs, (k * k, h * w), dev)?))
}

/// Bilinear samples of `rows` (`[H·W, C]`) at `[n, H·W]` positions, zero outside, times `m`.
/// Returns `[n, H·W, C]`.
fn sample(rows: &Tensor, y: &Tensor, x: &Tensor, m: &Tensor, h: usize, w: usize) -> Result<Tensor> {
    let (n, hw) = y.dims2()?;
    let c = rows.dim(1)?;
    let y0 = y.floor()?;
    let x0 = x.floor()?;
    let ly = (y - &y0)?;
    let lx = (x - &x0)?;
    let hy = ly.affine(-1.0, 1.0)?;
    let hx = lx.affine(-1.0, 1.0)?;
    let inside = |v: &Tensor, lo: f64, hi: f64| -> Result<Tensor> { Ok(v.ge(lo)?.to_dtype(DType::F32)?.mul(&v.le(hi)?.to_dtype(DType::F32)?)?) };
    let (hmax, wmax) = ((h - 1) as f64, (w - 1) as f64);
    let vy0 = inside(&y0, 0.0, hmax)?;
    let vy1 = inside(&y0, -1.0, hmax - 1.0)?;
    let vx0 = inside(&x0, 0.0, wmax)?;
    let vx1 = inside(&x0, -1.0, wmax - 1.0)?;
    let wy0 = hy.mul(&vy0)?;
    let wy1 = ly.mul(&vy1)?;
    let wx0 = hx.mul(&vx0)?;
    let wx1 = lx.mul(&vx1)?;
    let weights = Tensor::stack(
        &[wy0.mul(&wx0)?, wy0.mul(&wx1)?, wy1.mul(&wx0)?, wy1.mul(&wx1)?],
        0,
    )?
    .broadcast_mul(m)?; // [4, n, hw]
    let iy0 = y0.clamp(0.0, hmax)?;
    let iy1 = (&y0 + 1.0)?.clamp(0.0, hmax)?;
    let ix0 = x0.clamp(0.0, wmax)?;
    let ix1 = (&x0 + 1.0)?.clamp(0.0, wmax)?;
    let wf = w as f64;
    let at = |iy: &Tensor, ix: &Tensor| -> Result<Tensor> { Ok((iy.affine(wf, 0.0)? + ix)?) };
    let idx = Tensor::stack(&[at(&iy0, &ix0)?, at(&iy0, &ix1)?, at(&iy1, &ix0)?, at(&iy1, &ix1)?], 0)?
        .flatten_all()?
        .to_dtype(DType::U32)?;
    let gathered = rows.index_select(&idx, 0)?.reshape((4, n, hw, c))?;
    let weights = weights.reshape((4, n, hw, 1))?;
    Ok(gathered.broadcast_mul(&weights)?.sum(0)?)
}
