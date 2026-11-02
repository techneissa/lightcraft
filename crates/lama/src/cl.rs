//! The residual part and the upsampling of the generator, on channels-last maps `(h·w, c)`.
//!
//! Candle's GPU convolution is slow, but its matrix product is fast, so every convolution here is a sum
//! of matrix products: with the map as rows of channels, a `k × k` convolution is `k²` products of a
//! row-shifted view of the (padded) map with one `(c_in, c_out)` tap matrix, and a view of whole rows
//! needs no copy. BatchNorm is folded into the weights; a bias is added as a product with a column of
//! ones (a broadcast add is ten times slower on the GPU).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use candle_core::{Device, Tensor};

use crate::fft::Dft;
use crate::layers::Weights;
use crate::{Error, Result};

/// A feature-map size, with the index lists that pad a map by reflection and read the valid pixels back.
pub struct Geo {
    pub h: usize,
    pub w: usize,
    /// Row stride of the padded map.
    wp: usize,
    /// Rows of the shifted products: `h · wp − 2p`.
    len: usize,
    pad: usize,
    /// For each row of the padded map, its source row in the map (reflecting at the edges).
    pad_idx: Tensor,
    /// For each pixel of the map, its row in the product (which is `wp` wide).
    valid_idx: Tensor,
    /// `(h·w, 1)` ones, for biases.
    ones: Tensor,
    /// `(h·l, 1)` ones, for biases in the Fourier domain.
    ones_spec: Tensor,
    pub dft: Dft,
}

fn reflect(i: i64, n: i64) -> i64 {
    if i < 0 {
        -i
    } else if i >= n {
        2 * (n - 1) - i
    } else {
        i
    }
}

impl Geo {
    pub fn new(h: usize, w: usize, pad: usize, device: &Device) -> Result<Self> {
        if pad >= h || pad >= w {
            return Err(Error::Input(format!("a {w}×{h} map is too small to pad by {pad}")));
        }
        let (hp, wp) = (h + 2 * pad, w + 2 * pad);
        let p = pad as i64;
        let mut pad_idx = Vec::with_capacity(hp * wp);
        for yy in 0..hp as i64 {
            for xx in 0..wp as i64 {
                pad_idx.push((reflect(yy - p, h as i64) * w as i64 + reflect(xx - p, w as i64)) as u32);
            }
        }
        let valid: Vec<u32> = (0..h).flat_map(|y| (0..w).map(move |x| (y * wp + x) as u32)).collect();
        let dft = Dft::new(h, w, device)?;
        Ok(Self {
            h,
            w,
            wp,
            len: h * wp - 2 * pad,
            pad,
            pad_idx: Tensor::from_vec(pad_idx, hp * wp, device)?,
            valid_idx: Tensor::from_vec(valid, h * w, device)?,
            ones: Dft::ones(h * w, device)?,
            ones_spec: Dft::ones(h * dft.half_width(), device)?,
            dft,
        })
    }
}

/// The `Geo`s already built, by size (a fill keeps to one size for the whole network).
pub struct GeoCache {
    device: Device,
    map: Mutex<HashMap<(usize, usize, usize), Arc<Geo>>>,
}

impl GeoCache {
    pub fn new(device: Device) -> Self {
        Self { device, map: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, h: usize, w: usize, pad: usize) -> Result<Arc<Geo>> {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(g) = map.get(&(h, w, pad)) {
            return Ok(g.clone());
        }
        // few sizes are ever used (one per input size); keep the cache from growing without bound anyway
        if map.len() >= 12 {
            map.clear();
        }
        let g = Arc::new(Geo::new(h, w, pad, &self.device)?);
        map.insert((h, w, pad), g.clone());
        Ok(g)
    }
}

/// `row` `(1, o)` added to every row of an `(n, o)` map, as a product with `ones` `(n, 1)`.
fn add_row(x: &Tensor, ones: &Tensor, row: &Tensor) -> Result<Tensor> {
    Ok((x + ones.matmul(row)?)?)
}

/// A bias row `(1, o)` on `device`.
fn row(v: &[f32], device: &Device) -> Result<Tensor> {
    Ok(Tensor::from_slice(v, (1, v.len()), device)?)
}

/// A `k × k` stride-1 convolution with reflect padding, as `k²` tap matrices `(c_in, c_out)`.
pub struct TapConv {
    taps: Vec<Tensor>,
    k: usize,
}

impl TapConv {
    /// From a PyTorch weight `(out, inp, k, k)`, each output channel scaled by `scale` (BatchNorm folded in).
    pub fn new(w: &Tensor, scale: Option<&[f32]>) -> Result<Self> {
        let (o, c, k, k2) = w.dims4()?;
        if k != k2 || k == 0 {
            return Err(Error::Model("a convolution that is not square".into()));
        }
        let scale = match scale {
            Some(s) => Some(Tensor::from_slice(s, (1, o), w.device())?),
            None => None,
        };
        let mut taps = Vec::with_capacity(k * k);
        for ky in 0..k {
            for kx in 0..k {
                let m = w.narrow(2, ky, 1)?.narrow(3, kx, 1)?.reshape((o, c))?.t()?.contiguous()?;
                taps.push(match &scale {
                    Some(s) => m.broadcast_mul(s)?,
                    None => m,
                });
            }
        }
        Ok(Self { taps, k })
    }

    /// `x` `(h·w, c_in)` -> `(h·w, c_out)`. `geo` must have been built for this kernel's padding.
    pub fn forward(&self, x: &Tensor, geo: &Geo) -> Result<Tensor> {
        if geo.pad * 2 + 1 != self.k {
            return Err(Error::Model("a convolution and its map disagree about padding".into()));
        }
        let xp = x.index_select(&geo.pad_idx, 0)?;
        let mut acc: Option<Tensor> = None;
        for (t, w) in self.taps.iter().enumerate() {
            let off = (t / self.k) * geo.wp + t % self.k;
            let y = xp.narrow(0, off, geo.len)?.matmul(w)?;
            acc = Some(match acc {
                Some(a) => (a + y)?,
                None => y,
            });
        }
        let acc = acc.ok_or_else(|| Error::Model("an empty convolution".into()))?;
        // the product is `wp` pixels wide; keep the first `w` of each row (the last rows' tail is not computed,
        // and is never one of the valid pixels)
        Ok(acc.index_select(&geo.valid_idx, 0)?)
    }
}

/// The Fourier unit's 1×1 convolution on the real and imaginary planes, BatchNorm folded in.
struct FourierUnit {
    /// real -> real, imaginary -> real, real -> imaginary, imaginary -> imaginary; each `(c, c)`.
    rr: Tensor,
    ir: Tensor,
    ri: Tensor,
    ii: Tensor,
    bias_r: Tensor,
    bias_i: Tensor,
}

impl FourierUnit {
    fn load(w: &mut Weights, prefix: &str, c: usize) -> Result<Self> {
        let conv = w.take(&format!("{prefix}.conv_layer.weight"), &[2 * c, 2 * c, 1, 1])?.reshape((2 * c, 2 * c))?.to_vec2::<f32>()?;
        let (scale, shift) = w.bn(&format!("{prefix}.bn"), 2 * c)?;
        // the file interleaves the parts: channel `2i` is the real part of `i`, `2i + 1` its imaginary part
        let block = |part_out: usize, part_in: usize| -> Result<Tensor> {
            let mut m = vec![0f32; c * c];
            for o in 0..c {
                for i in 0..c {
                    let (ro, ci) = (2 * o + part_out, 2 * i + part_in);
                    m[i * c + o] = conv.get(ro).and_then(|r| r.get(ci)).copied().unwrap_or(0.0) * scale.get(ro).copied().unwrap_or(1.0);
                }
            }
            Ok(Tensor::from_vec(m, (c, c), &w.device)?)
        };
        let bias = |part: usize| -> Vec<f32> { (0..c).map(|o| shift.get(2 * o + part).copied().unwrap_or(0.0)).collect() };
        Ok(Self {
            rr: block(0, 0)?,
            ir: block(0, 1)?,
            ri: block(1, 0)?,
            ii: block(1, 1)?,
            bias_r: row(&bias(0), &w.device)?,
            bias_i: row(&bias(1), &w.device)?,
        })
    }

    /// `x` `(h·w, c)`.
    fn forward(&self, x: &Tensor, geo: &Geo) -> Result<Tensor> {
        let (h, w) = (geo.h, geo.w);
        let c = self.rr.dim(0)?;
        let spec = geo.dft.rfft2(&x.reshape((h, w, c))?)?; // (2h, l, c)
        let hl = h * geo.dft.half_width();
        let yr = spec.narrow(0, 0, h)?.reshape((hl, c))?;
        let yi = spec.narrow(0, h, h)?.reshape((hl, c))?;
        let or = add_row(&(yr.matmul(&self.rr)? + yi.matmul(&self.ir)?)?, &geo.ones_spec, &self.bias_r)?.relu()?;
        let oi = add_row(&(yr.matmul(&self.ri)? + yi.matmul(&self.ii)?)?, &geo.ones_spec, &self.bias_i)?.relu()?;
        let z = Tensor::cat(&[&or, &oi], 0)?.contiguous()?.reshape((2 * h, geo.dft.half_width(), c))?;
        Ok(geo.dft.irfft2(&z)?.reshape((h * w, c))?)
    }
}

/// An FFC layer inside a residual block (3×3, stride 1, reflect padding) with its BatchNorm and ReLU.
struct BlockLayer {
    l2l: TapConv,
    l2g: TapConv,
    g2l: TapConv,
    bias_l: Tensor,
    bias_g: Tensor,
    conv1: Tensor,
    bias1: Tensor,
    fu: FourierUnit,
    conv2: Tensor,
}

impl BlockLayer {
    fn load(w: &mut Weights, prefix: &str, l: usize, g: usize) -> Result<Self> {
        let (scale_l, shift_l) = w.bn(&format!("{prefix}.bn_l"), l)?;
        let (scale_g, shift_g) = w.bn(&format!("{prefix}.bn_g"), g)?;
        let ffc = format!("{prefix}.ffc");
        let spectral = format!("{ffc}.convg2g");
        let half = g / 2;
        let l2l = TapConv::new(&w.take(&format!("{ffc}.convl2l.weight"), &[l, l, 3, 3])?, Some(&scale_l))?;
        let l2g = TapConv::new(&w.take(&format!("{ffc}.convl2g.weight"), &[g, l, 3, 3])?, Some(&scale_g))?;
        let g2l = TapConv::new(&w.take(&format!("{ffc}.convg2l.weight"), &[l, g, 3, 3])?, Some(&scale_l))?;
        let conv1 = w.take(&format!("{spectral}.conv1.0.weight"), &[half, g, 1, 1])?.reshape((half, g))?;
        let (s1, b1) = w.bn(&format!("{spectral}.conv1.1"), half)?;
        let conv1 = conv1.t()?.contiguous()?.broadcast_mul(&row(&s1, &w.device)?)?;
        let conv2 = w.take(&format!("{spectral}.conv2.weight"), &[g, half, 1, 1])?.reshape((g, half))?;
        let conv2 = conv2.t()?.contiguous()?.broadcast_mul(&row(&scale_g, &w.device)?)?;
        Ok(Self {
            l2l,
            l2g,
            g2l,
            bias_l: row(&shift_l, &w.device)?,
            bias_g: row(&shift_g, &w.device)?,
            conv1,
            bias1: row(&b1, &w.device)?,
            fu: FourierUnit::load(w, &format!("{spectral}.fu"), half)?,
            conv2,
        })
    }

    fn forward(&self, xl: &Tensor, xg: &Tensor, geo: &Geo) -> Result<(Tensor, Tensor)> {
        let out_l = add_row(&(self.l2l.forward(xl, geo)? + self.g2l.forward(xg, geo)?)?, &geo.ones, &self.bias_l)?.relu()?;
        let t = add_row(&xg.matmul(&self.conv1)?, &geo.ones, &self.bias1)?.relu()?;
        let spectral = (&t + self.fu.forward(&t, geo)?)?.matmul(&self.conv2)?;
        let out_g = add_row(&(self.l2g.forward(xl, geo)? + spectral)?, &geo.ones, &self.bias_g)?.relu()?;
        Ok((out_l, out_g))
    }
}

/// Two FFC layers with a residual connection on both branches.
pub struct ResnetBlock {
    conv1: BlockLayer,
    conv2: BlockLayer,
}

impl ResnetBlock {
    pub fn load(w: &mut Weights, prefix: &str, l: usize, g: usize) -> Result<Self> {
        if g == 0 || !g.is_multiple_of(2) {
            return Err(Error::Model("a residual block needs an even, non-empty global branch".into()));
        }
        Ok(Self { conv1: BlockLayer::load(w, &format!("{prefix}.conv1"), l, g)?, conv2: BlockLayer::load(w, &format!("{prefix}.conv2"), l, g)? })
    }

    pub fn forward(&self, xl: &Tensor, xg: &Tensor, geo: &Geo) -> Result<(Tensor, Tensor)> {
        let (l, g) = self.conv1.forward(xl, xg, geo)?;
        let (l, g) = self.conv2.forward(&l, &g, geo)?;
        Ok(((xl + l)?, (xg + g)?))
    }
}

/// A transposed convolution (stride 2, padding 1, output padding 1) with BatchNorm and ReLU: each output
/// pixel is the sum of at most four shifted input pixels times one of the nine taps.
pub struct Up {
    /// `taps[ky][kx]`: `(c_in, c_out)`.
    taps: Vec<Tensor>,
    bias: Tensor,
}

impl Up {
    pub fn load(w: &mut Weights, conv: usize, bn: usize, inp: usize, out: usize) -> Result<Self> {
        let weight = w.take(&format!("{conv}.weight"), &[inp, out, 3, 3])?;
        let bias = w.take(&format!("{conv}.bias"), &[out])?.to_vec1::<f32>()?;
        let (scale, shift) = w.bn(&bn.to_string(), out)?;
        let scale_row = row(&scale, &w.device)?;
        let mut taps = Vec::with_capacity(9);
        for ky in 0..3 {
            for kx in 0..3 {
                taps.push(weight.narrow(2, ky, 1)?.narrow(3, kx, 1)?.reshape((inp, out))?.contiguous()?.broadcast_mul(&scale_row)?);
            }
        }
        let folded: Vec<f32> = bias.iter().zip(&scale).zip(&shift).map(|((b, s), t)| b * s + t).collect();
        Ok(Self { taps, bias: row(&folded, &w.device)? })
    }

    /// `x` `(h·w, c_in)` of a map `h × w` -> `(4·h·w, c_out)` of the map `2h × 2w`.
    pub fn forward(&self, x: &Tensor, h: usize, w: usize) -> Result<Tensor> {
        let c = x.dim(1)?;
        let (hp, wp) = (h + 1, w + 1);
        let xp = x.reshape((h, w, c))?.pad_with_zeros(0, 0, 1)?.pad_with_zeros(1, 0, 1)?.reshape((hp * wp, c))?;
        let len = h * wp - 1;
        // (shift, tap index) pairs that feed an output row / column of parity 0 and 1
        const PARITY: [&[(usize, usize)]; 2] = [&[(0, 1)], &[(0, 2), (1, 0)]];
        let mut phases = Vec::with_capacity(4);
        for ya in PARITY {
            for xb in PARITY {
                let mut acc: Option<Tensor> = None;
                for &(dy, ky) in ya {
                    for &(dx, kx) in xb {
                        let tap = self.taps.get(ky * 3 + kx).ok_or_else(|| Error::Model("missing upsampling tap".into()))?;
                        let y = xp.narrow(0, dy * wp + dx, len)?.matmul(tap)?;
                        acc = Some(match acc {
                            Some(a) => (a + y)?,
                            None => y,
                        });
                    }
                }
                phases.push(acc.ok_or_else(|| Error::Model("an empty upsampling phase".into()))?);
            }
        }
        let stacked = Tensor::cat(&phases, 0)?.contiguous()?;
        // output pixel (2m + a, 2n + b) is row `m·wp + n` of phase `2a + b`
        let mut perm = Vec::with_capacity(4 * h * w);
        for oy in 0..2 * h {
            for ox in 0..2 * w {
                let phase = (oy % 2) * 2 + ox % 2;
                perm.push((phase * len + (oy / 2) * wp + ox / 2) as u32);
            }
        }
        let perm = Tensor::from_vec(perm, 4 * h * w, x.device())?;
        let out = stacked.index_select(&perm, 0)?;
        let ones = Dft::ones(4 * h * w, x.device())?;
        Ok(add_row(&out, &ones, &self.bias)?.relu()?)
    }
}

/// The output convolution: reflect padded, with a bias.
pub struct Head {
    conv: TapConv,
    bias: Tensor,
}

impl Head {
    pub fn load(w: &mut Weights, prefix: &str, inp: usize, k: usize) -> Result<Self> {
        let weight = w.take(&format!("{prefix}.weight"), &[3, inp, k, k])?;
        let bias = w.take(&format!("{prefix}.bias"), &[3])?.to_vec1::<f32>()?;
        Ok(Self { conv: TapConv::new(&weight, None)?, bias: row(&bias, &w.device)? })
    }

    /// `x` `(h·w, c)` -> `(h·w, 3)`, before the sigmoid.
    pub fn forward(&self, x: &Tensor, geo: &Geo) -> Result<Tensor> {
        add_row(&self.conv.forward(x, geo)?, &geo.ones, &self.bias)
    }
}

/// `(1, c, h, w)` as channels-last rows `(h·w, c)`.
pub fn to_rows(x: &Tensor) -> Result<Tensor> {
    let (_, c, h, w) = x.dims4()?;
    Ok(x.permute((0, 2, 3, 1))?.contiguous()?.reshape((h * w, c))?)
}
