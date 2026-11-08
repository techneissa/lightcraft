//! Sapiens2 body-part segmentation (Khirodkar et al., "Sapiens2", ICLR 2026): a plain ViT with 2-D rotary positions,
//! RMS norms, query/key norms, grouped-query attention in its middle blocks and SwiGLU feed-forwards, then four 2×
//! transposed convolutions back to the input's size and 29 classes (`docs/SEG.md` of facebookresearch/sapiens2).
//!
//! Written for candle from the paper and the layout of the MLX bf16 checkpoint (`mlx-community/sapiens2-seg-0.4b-bf16`:
//! kernels out, kh, kw, in), checked against Meta's published inference code and mlx-vlm's MIT port (on the same input
//! the two give the same class at 99.8 % of the pixels). The sizes come from the weights, so the 0.8B and 1B
//! checkpoints load too. The weights' licence (Sapiens2 License) forbids identifying people: what this network sees
//! never feeds face recognition.

use candle_core::{D, DType, Device, Tensor};
use candle_nn::{Linear, Module};

use crate::nn::{Conv, ConvOpts, Params};
use crate::{Error, Result};

/// The classes, in the model's order.
pub const CLASSES: usize = 29;
pub const PATCH: usize = 16;
/// The input the model was trained on (height × width).
pub const TRAIN_H: usize = 1024;
pub const TRAIN_W: usize = 768;
const HEAD_DIM: usize = 64;
const EPS: f32 = 1e-6;
/// PyTorch's `InstanceNorm2d` epsilon.
const IN_EPS: f64 = 1e-5;
/// More blocks than any Sapiens2 size has (5B: 56).
const MAX_LAYERS: usize = 128;
/// Query rows per attention step (16 heads × 1,024 rows × 3,081 keys is about 200 MB of scores).
const ATTENTION_ROWS: usize = 1024;

/// Class ids (`docs/SEG.md`).
pub mod class {
    pub const BACKGROUND: usize = 0;
    pub const APPAREL: usize = 1;
    pub const EYEGLASS: usize = 2;
    pub const FACE_NECK: usize = 3;
    pub const HAIR: usize = 4;
    pub const LEFT_FOOT: usize = 5;
    pub const LEFT_HAND: usize = 6;
    pub const LEFT_LOWER_ARM: usize = 7;
    pub const LEFT_LOWER_LEG: usize = 8;
    pub const LEFT_SHOE: usize = 9;
    pub const LEFT_SOCK: usize = 10;
    pub const LEFT_UPPER_ARM: usize = 11;
    pub const LEFT_UPPER_LEG: usize = 12;
    pub const LOWER_CLOTHING: usize = 13;
    pub const RIGHT_FOOT: usize = 14;
    pub const RIGHT_HAND: usize = 15;
    pub const RIGHT_LOWER_ARM: usize = 16;
    pub const RIGHT_LOWER_LEG: usize = 17;
    pub const RIGHT_SHOE: usize = 18;
    pub const RIGHT_SOCK: usize = 19;
    pub const RIGHT_UPPER_ARM: usize = 20;
    pub const RIGHT_UPPER_LEG: usize = 21;
    pub const TORSO: usize = 22;
    pub const UPPER_CLOTHING: usize = 23;
    pub const LOWER_LIP: usize = 24;
    pub const UPPER_LIP: usize = 25;
    pub const LOWER_TEETH: usize = 26;
    pub const UPPER_TEETH: usize = 27;
    pub const TONGUE: usize = 28;
}

/// Round to the nearest bfloat16 (ties to even), as the model's rotary table is computed in bf16.
fn bf16(x: f32) -> f32 {
    if !x.is_finite() {
        return x;
    }
    let b = x.to_bits();
    let r = b.wrapping_add(0x7fff + ((b >> 16) & 1)) & 0xffff_0000;
    f32::from_bits(r)
}

/// The rotary table for an `h × w` patch grid: `(sin, cos)`, each `h·w × head_dim`. Coordinates are the patch centres
/// in −1 to 1 along each axis ("separate"); each of the `head_dim / 4` periods turns the y coordinate in the first
/// quarter and the x coordinate in the second, and the half repeats. Rounded to bf16 at each step, as trained.
fn rope_table(periods: &[f32], h: usize, w: usize) -> (Vec<f32>, Vec<f32>) {
    let q = periods.len();
    let d = q * 4;
    let (mut sin, mut cos) = (vec![0.0f32; h * w * d], vec![0.0f32; h * w * d]);
    let two_pi = bf16(2.0 * std::f32::consts::PI);
    let coord = |i: usize, n: usize| bf16(2.0 * bf16(bf16(i as f32 + 0.5) / n.max(1) as f32) - 1.0);
    for y in 0..h {
        for x in 0..w {
            let row = (y * w + x) * d;
            for (axis, c) in [coord(y, h), coord(x, w)].into_iter().enumerate() {
                for (k, &p) in periods.iter().enumerate() {
                    let a = bf16(bf16(two_pi * c) / p);
                    let (s, co) = (bf16(a.sin()), bf16(a.cos()));
                    for half in 0..2 {
                        let at = row + half * (d / 2) + axis * q + k;
                        if let (Some(si), Some(ci)) = (sin.get_mut(at), cos.get_mut(at)) {
                            *si = s;
                            *ci = co;
                        }
                    }
                }
            }
        }
    }
    (sin, cos)
}

/// `x · cos + rotate_half(x) · sin` on the patch tokens (`B × heads × N × d`, the first `prefix` tokens untouched).
fn apply_rope(x: &Tensor, sin: &Tensor, cos: &Tensor, prefix: usize) -> Result<Tensor> {
    let n = x.dim(2)?;
    let d = x.dim(3)?;
    let head = x.narrow(2, 0, prefix)?;
    let t = x.narrow(2, prefix, n - prefix)?;
    let (a, b) = (t.narrow(D::Minus1, 0, d / 2)?, t.narrow(D::Minus1, d / 2, d / 2)?);
    let rot = Tensor::cat(&[&b.neg()?, &a], D::Minus1)?;
    let t = (t.broadcast_mul(cos)? + rot.broadcast_mul(sin)?)?;
    Ok(Tensor::cat(&[&head, &t], 2)?.contiguous()?)
}

/// softmax(q·kᵀ / √d)·v for `B × heads × N × d`, `rows` queries at a time.
fn attend(q: &Tensor, k: &Tensor, v: &Tensor, rows: usize) -> Result<Tensor> {
    let n = q.dim(2)?;
    let scale = (q.dim(3)? as f64).sqrt();
    let kt = k.transpose(2, 3)?.contiguous()?;
    let mut parts = Vec::with_capacity(n.div_ceil(rows.max(1)));
    let mut at = 0;
    while at < n {
        let len = rows.max(1).min(n - at);
        let scores = (q.narrow(2, at, len)?.contiguous()?.matmul(&kt)? / scale)?;
        parts.push(candle_nn::ops::softmax_last_dim(&scores)?.matmul(v)?);
        at += len;
    }
    Ok(Tensor::cat(&parts, 2)?)
}

fn rms(x: &Tensor, w: &Tensor) -> Result<Tensor> {
    Ok(candle_nn::ops::rms_norm(&x.contiguous()?, w, EPS)?)
}

struct Block {
    ln1: Tensor,
    wq: Linear,
    wk: Linear,
    wv: Linear,
    q_norm: Tensor,
    k_norm: Tensor,
    proj: Linear,
    gamma: Option<Tensor>,
    ln2: Tensor,
    w12: Linear,
    w3: Linear,
    heads: usize,
    kv_heads: usize,
}

impl Block {
    fn load(p: &Params, c: usize, hidden: usize) -> Result<Self> {
        let a = p.pp("attn");
        let heads = c / HEAD_DIM;
        let kv = match a.shape("wk.weight") {
            Some(&[o, i]) if i == c && o % HEAD_DIM == 0 && o > 0 && heads.is_multiple_of(o / HEAD_DIM) => o,
            other => return Err(Error::Model(format!("not a Sapiens2: key projection is {other:?}"))),
        };
        Ok(Self {
            ln1: p.pp("ln1").tensor("weight", &[c])?,
            wq: a.pp("wq").linear(c, c)?,
            wk: a.pp("wk").linear(c, kv)?,
            wv: a.pp("wv").linear(c, kv)?,
            q_norm: a.pp("q_norm").tensor("weight", &[HEAD_DIM])?,
            k_norm: a.pp("k_norm").tensor("weight", &[HEAD_DIM])?,
            proj: a.pp("proj").linear(c, c)?,
            gamma: if a.has("gamma.weight") { Some(a.pp("gamma").tensor("weight", &[c])?) } else { None },
            ln2: p.pp("ln2").tensor("weight", &[c])?,
            w12: p.pp("ffn.w12").linear(c, 2 * hidden)?,
            w3: p.pp("ffn.w3").linear(hidden, c)?,
            heads,
            kv_heads: kv / HEAD_DIM,
        })
    }

    fn attention(&self, x: &Tensor, sin: &Tensor, cos: &Tensor, prefix: usize) -> Result<Tensor> {
        let (b, n, c) = x.dims3()?;
        let split = |t: Tensor, heads: usize| -> Result<Tensor> { Ok(t.reshape((b, n, heads, HEAD_DIM))?.transpose(1, 2)?.contiguous()?) };
        let q = rms(&split(self.wq.forward(x)?, self.heads)?, &self.q_norm)?;
        let mut k = rms(&split(self.wk.forward(x)?, self.kv_heads)?, &self.k_norm)?;
        let mut v = split(self.wv.forward(x)?, self.kv_heads)?;
        if self.kv_heads != self.heads {
            // each key/value head serves `factor` neighbouring query heads (PyTorch's repeat_interleave)
            let factor = self.heads / self.kv_heads.max(1);
            let rep = |t: &Tensor| -> Result<Tensor> {
                Ok(t.unsqueeze(2)?.broadcast_as((b, self.kv_heads, factor, n, HEAD_DIM))?.reshape((b, self.heads, n, HEAD_DIM))?)
            };
            k = rep(&k)?;
            v = rep(&v)?;
        }
        let q = apply_rope(&q, sin, cos, prefix)?;
        let k = apply_rope(&k, sin, cos, prefix)?;
        let ctx = attend(&q, &k, &v.contiguous()?, ATTENTION_ROWS)?.transpose(1, 2)?.reshape((b, n, c))?;
        let out = self.proj.forward(&ctx)?;
        Ok(match &self.gamma {
            Some(g) => out.broadcast_mul(g)?,
            None => out,
        })
    }

    fn forward(&self, x: &Tensor, sin: &Tensor, cos: &Tensor, prefix: usize) -> Result<Tensor> {
        let x = (x + self.attention(&rms(x, &self.ln1)?, sin, cos, prefix)?)?;
        let y = self.w12.forward(&rms(&x, &self.ln2)?)?;
        let hidden = y.dim(D::Minus1)? / 2;
        let gated = (candle_nn::ops::silu(&y.narrow(D::Minus1, 0, hidden)?)? * y.narrow(D::Minus1, hidden, hidden)?)?;
        Ok((x + self.w3.forward(&gated)?)?)
    }
}

/// A Sapiens2 segmentation network.
pub struct Sapiens2 {
    patch: Conv,
    prefix_tokens: Tensor,
    periods: Vec<f32>,
    blocks: Vec<Block>,
    ln: Tensor,
    /// The head's 2× transposed convolutions, each as a 3 × 3 convolution to four times the channels and a pixel
    /// shuffle (the same sums; candle's plain convolution is much faster on Metal).
    deconvs: Vec<Conv>,
    convs: Vec<Conv>,
    seg: Conv,
    width: usize,
}

impl Sapiens2 {
    /// Read a Sapiens2 segmentation checkpoint in MLX layout; its sizes come from the tensors' shapes.
    pub fn load(p: &Params) -> Result<Self> {
        let bb = p.pp("backbone");
        let c = match bb.shape("patch_embed.projection.weight") {
            Some(&[c, PATCH, PATCH, 3]) if c % HEAD_DIM == 0 && (256..=4096).contains(&c) => c,
            other => return Err(Error::Model(format!("not a Sapiens2: patch embedding is {other:?}"))),
        };
        let hidden = match bb.shape("blocks.0.ffn.w3.weight") {
            Some(&[o, h]) if o == c && (c..=8 * c).contains(&h) => h,
            other => return Err(Error::Model(format!("not a Sapiens2: feed-forward is {other:?}"))),
        };
        let depth = (0..MAX_LAYERS).take_while(|i| bb.has(&format!("blocks.{i}.ln1.weight"))).count();
        if depth == 0 {
            return Err(Error::Model("not a Sapiens2: no blocks".into()));
        }
        let storage = match bb.shape("storage_tokens") {
            Some(&[1, n, cc]) if cc == c && n <= 64 => n,
            other => return Err(Error::Model(format!("not a Sapiens2: storage tokens are {other:?}"))),
        };
        let cls = bb.tensor("cls_token", &[1, 1, c])?;
        let regs = bb.tensor("storage_tokens", &[1, storage, c])?;
        let periods = bb.tensor("rope_embed.periods", &[HEAD_DIM / 4])?.to_device(&Device::Cpu)?.to_vec1::<f32>()?;
        if periods.iter().any(|p| !p.is_finite() || *p <= 0.0) {
            return Err(Error::Model("Sapiens2: bad rotary periods".into()));
        }
        let blocks = (0..depth).map(|i| Block::load(&bb.pp(&format!("blocks.{i}")), c, hidden)).collect::<Result<Vec<_>>>()?;
        let head = p.pp("decode_head");
        let mut deconvs = Vec::new();
        let mut input = c;
        while let Some(&[o, 4, 4, i]) = head.shape(&format!("deconv_layers.{}.weight", deconvs.len() * 3)) {
            if i != input || o == 0 {
                return Err(Error::Model(format!("not a Sapiens2: deconvolution {} is {o} × {i}", deconvs.len())));
            }
            // MLX keeps transposed kernels as out, kh, kw, in (PyTorch's in, out, kh, kw, not flipped)
            let k = head.tensor(&format!("deconv_layers.{}.weight", deconvs.len() * 3), &[o, 4, 4, i])?;
            deconvs.push(up2_as_conv(&k)?);
            input = o;
        }
        let mut convs = Vec::new();
        while let Some(&[o, 1, 1, i]) = head.shape(&format!("conv_layers.{}.weight", convs.len() * 3)) {
            if i != input {
                return Err(Error::Model(format!("not a Sapiens2: head convolution {} is {o} × {i}", convs.len())));
            }
            convs.push(head.pp(&format!("conv_layers.{}", convs.len() * 3)).conv(i, o, 1, ConvOpts::same(1), None)?);
            input = o;
        }
        let classes = match head.shape("conv_seg.weight") {
            Some(&[n, 1, 1, i]) if i == input => n,
            other => return Err(Error::Model(format!("not a Sapiens2: classifier is {other:?}"))),
        };
        if classes != CLASSES {
            return Err(Error::Model(format!("Sapiens2: {classes} classes, not {CLASSES}")));
        }
        Ok(Self {
            patch: bb.pp("patch_embed.projection").conv(3, c, PATCH, ConvOpts::same(PATCH).stride(PATCH).padding(0), None)?,
            prefix_tokens: Tensor::cat(&[&cls, &regs], 1)?,
            periods,
            blocks,
            ln: bb.pp("ln1").tensor("weight", &[c])?,
            deconvs,
            convs,
            seg: head.pp("conv_seg").conv(input, classes, 1, ConvOpts::same(1), None)?,
            width: c,
        })
    }

    /// Logits at the input's size, `1 × 29 × H × W`, for a normalised `1 × 3 × H × W` input (sides multiples of 16).
    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, _, h, w) = x.dims4()?;
        if b != 1 || h % PATCH != 0 || w % PATCH != 0 || h == 0 || w == 0 {
            return Err(Error::Input(format!("Sapiens2 input must be 1 × 3 × H × W in multiples of {PATCH}, not {:?}", x.dims())));
        }
        let dev = x.device();
        let (gh, gw) = (h / PATCH, w / PATCH);
        let e = self.patch.forward(x)?;
        let tokens = e.reshape((1, self.width, gh * gw))?.transpose(1, 2)?;
        let prefix = self.prefix_tokens.dim(1)?;
        let mut t = Tensor::cat(&[&self.prefix_tokens, &tokens], 1)?.contiguous()?;
        let (sin, cos) = rope_table(&self.periods, gh, gw);
        let shape = (gh * gw, HEAD_DIM);
        let sin = Tensor::from_vec(sin, shape, dev)?;
        let cos = Tensor::from_vec(cos, shape, dev)?;
        for blk in &self.blocks {
            t = blk.forward(&t, &sin, &cos, prefix)?;
        }
        let t = rms(&t, &self.ln)?;
        let mut y = t.narrow(1, prefix, gh * gw)?.transpose(1, 2)?.reshape((1, self.width, gh, gw))?.contiguous()?;
        for k in &self.deconvs {
            y = candle_nn::ops::pixel_shuffle(&k.forward(&y)?, 2)?;
            y = candle_nn::ops::silu(&instance_norm(&y)?)?;
        }
        for c in &self.convs {
            y = candle_nn::ops::silu(&instance_norm(&c.forward(&y)?)?)?;
        }
        Ok(self.seg.forward(&y)?)
    }
}

/// A transposed convolution with a 4 × 4 kernel, stride 2 and padding 1 (`k`: out, kh, kw, in, as MLX stores it) as
/// a 3 × 3 convolution (padding 1) to `out × 2 × 2` channels; a pixel shuffle by 2 then gives the transposed
/// convolution's output. Output row `2m + a` takes input rows `m` (kernel row 1 for `a = 0`, 2 for `a = 1`) and
/// `m − 1` (row 3, `a = 0`) or `m + 1` (row 0, `a = 1`); the same for columns.
fn up2_as_conv(k: &Tensor) -> Result<Conv> {
    let (o, kh, kw, i) = k.dims4()?;
    if (kh, kw) != (4, 4) {
        return Err(Error::Model(format!("Sapiens2: a {kh} × {kw} upsampling kernel, not 4 × 4")));
    }
    let dev = k.device().clone();
    let w = k.to_device(&Device::Cpu)?.flatten_all()?.to_vec1::<f32>()?;
    // the kernel tap for output phase `a` and input offset `d` (−1, 0, 1)
    let tap = |a: usize, d: isize| -> Option<usize> {
        match (a, d) {
            (0, 0) => Some(1),
            (0, -1) => Some(3),
            (1, 0) => Some(2),
            (1, 1) => Some(0),
            _ => None,
        }
    };
    let mut out = vec![0.0f32; o * 4 * i * 9];
    for oc in 0..o {
        for a in 0..2 {
            for b in 0..2 {
                let c = oc * 4 + a * 2 + b;
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let (Some(ky), Some(kx)) = (tap(a, dy), tap(b, dx)) else { continue };
                        let at = (dy + 1) as usize * 3 + (dx + 1) as usize;
                        for ic in 0..i {
                            if let (Some(dst), Some(&v)) = (out.get_mut((c * i + ic) * 9 + at), w.get(((oc * 4 + ky) * 4 + kx) * i + ic)) {
                                *dst = v;
                            }
                        }
                    }
                }
            }
        }
    }
    let kernel = Tensor::from_vec(out, (o * 4, i, 3, 3), &Device::Cpu)?.to_device(&dev)?;
    Ok(Conv::from_parts(kernel, None, 1, 1, 1))
}

/// `InstanceNorm2d` without learned scale: each channel to mean 0, variance 1 over its pixels.
fn instance_norm(x: &Tensor) -> Result<Tensor> {
    let (b, c, h, w) = x.dims4()?;
    let flat = x.reshape((b, c, h * w))?;
    let mean = flat.mean_keepdim(2)?;
    let centred = flat.broadcast_sub(&mean)?;
    let var = centred.sqr()?.mean_keepdim(2)?;
    Ok(centred.broadcast_div(&(var + IN_EPS)?.sqrt()?)?.reshape((b, c, h, w))?)
}

/// The model's input from planes 0 to 1 (channel-major): ImageNet normalisation, F32 on `dev`.
pub fn input(mut planes: Vec<f32>, w: usize, h: usize, dev: &Device) -> Result<Tensor> {
    crate::image::imagenet(&mut planes, w * h);
    Ok(Tensor::from_vec(planes, (1, 3, h, w), dev)?.to_dtype(DType::F32)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bf16_rounds_to_nearest_even() {
        assert_eq!(bf16(1.0), 1.0);
        assert_eq!(bf16(1.0 + 1.0 / 512.0), 1.0, "a tie goes to the even mantissa");
        assert_eq!(bf16(1.0 + 3.0 / 512.0), 1.0 + 4.0 / 512.0);
        assert_eq!(bf16(std::f32::consts::PI), 3.140625);
        assert!(bf16(f32::NAN).is_nan());
    }

    #[test]
    fn the_rotary_table_turns_y_then_x_and_repeats() {
        let periods = [1.0f32, 2.0];
        let (sin, cos) = rope_table(&periods, 2, 3);
        let d = 8;
        assert_eq!(sin.len(), 2 * 3 * d);
        // patch (y 1, x 0): y coordinate +0.5, x coordinate −2/3
        let row = 3 * d; // y 1, x 0 of a 3-wide grid
        let y = bf16(bf16(2.0 * std::f32::consts::PI) * 0.5) / 1.0;
        assert!((sin[row] - bf16(bf16(y).sin())).abs() < 1e-6);
        assert_eq!(sin[row], sin[row + 4], "the second half repeats the first");
        assert_eq!(cos[row + 2], cos[row + 6]);
        let x = bf16(2.0 * bf16(bf16(0.5) / 3.0) - 1.0);
        let ax = bf16(bf16(bf16(2.0 * std::f32::consts::PI) * x) / 1.0);
        assert!((cos[row + 2] - bf16(ax.cos())).abs() < 1e-6);
    }

    #[test]
    fn rope_leaves_the_prefix_and_keeps_the_norm_of_each_pair() {
        let dev = Device::Cpu;
        let x = Tensor::arange(0f32, 2.0 * 5.0 * 8.0, &dev).unwrap().affine(0.1, -1.0).unwrap().reshape((1, 2, 5, 8)).unwrap();
        let (s, c) = rope_table(&[1.0, 3.0], 2, 2);
        let (s, c) = (Tensor::from_vec(s, (4, 8), &dev).unwrap(), Tensor::from_vec(c, (4, 8), &dev).unwrap());
        let y = apply_rope(&x, &s, &c, 1).unwrap();
        let (xv, yv) = (x.flatten_all().unwrap().to_vec1::<f32>().unwrap(), y.flatten_all().unwrap().to_vec1::<f32>().unwrap());
        assert_eq!(&xv[..8], &yv[..8], "the class token is not turned");
        // element i pairs with i + 4: their squared sum stays (cos² + sin² = 1, up to bf16 rounding)
        for tok in 1..5 {
            for i in 0..4 {
                let a = tok * 8 + i;
                let before = xv[a].powi(2) + xv[a + 4].powi(2);
                let after = yv[a].powi(2) + yv[a + 4].powi(2);
                assert!((before - after).abs() < 0.02 * before.max(1.0), "{tok} {i}: {before} {after}");
            }
        }
    }

    #[test]
    fn the_upsampling_convolution_equals_the_transposed_one() {
        let dev = Device::Cpu;
        let (o, i) = (3, 2);
        let k = Tensor::arange(0f32, (o * 16 * i) as f32, &dev).unwrap().affine(0.71, 0.3).unwrap().sin().unwrap().reshape((o, 4, 4, i)).unwrap();
        let x = Tensor::arange(0f32, (i * 5 * 4) as f32, &dev).unwrap().affine(0.37, 1.0).unwrap().cos().unwrap().reshape((1, i, 5, 4)).unwrap();
        let want = x.conv_transpose2d(&k.permute((3, 0, 1, 2)).unwrap().contiguous().unwrap(), 1, 0, 2, 1).unwrap();
        let got = candle_nn::ops::pixel_shuffle(&up2_as_conv(&k).unwrap().forward(&x).unwrap(), 2).unwrap();
        assert_eq!(want.dims(), &[1, o, 10, 8]);
        assert_eq!(got.dims(), want.dims());
        let diff = (want - got).unwrap().abs().unwrap().flatten_all().unwrap().max(0).unwrap().to_scalar::<f32>().unwrap();
        assert!(diff < 1e-5, "{diff}");
    }

    #[test]
    fn instance_norm_centres_each_channel() {
        let dev = Device::Cpu;
        let x = Tensor::arange(0f32, 2.0 * 3.0 * 4.0, &dev).unwrap().reshape((1, 2, 3, 4)).unwrap();
        let y = instance_norm(&x).unwrap();
        let mean = y.mean_keepdim(3).unwrap().mean_keepdim(2).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert!(mean.iter().all(|m| m.abs() < 1e-5));
    }
}
