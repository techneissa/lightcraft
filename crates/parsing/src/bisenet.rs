//! BiSeNet face parsing (Yu et al., ECCV 2018), the 19-class CelebAMask-HQ model of `zllrunning/face-parsing.PyTorch`
//! (MIT) with its ResNet-18 context path, as the MLX port `mlx-community/face-parsing-bisenet-bf16` stores it
//! (kernels out, kh, kw, in; the downsample pair named `conv` / `bn`). Written from the paper and that repository's
//! layer layout; batch norms are folded into the convolutions.

use candle_core::Tensor;
use candle_nn::Module;

use crate::nn::{Conv, ConvOpts, Params, global_avg};
use crate::{Error, Result};

/// Classes the published model has.
pub const CLASSES: usize = 19;

/// `ConvBNReLU` without its ReLU (the caller adds it).
fn cbr(p: &Params, i: usize, o: usize, k: usize, stride: usize) -> Result<Conv> {
    p.pp("conv").conv(i, o, k, ConvOpts::same(k).stride(stride).no_bias(), Some(&p.pp("bn")))
}

struct Basic {
    c1: Conv,
    c2: Conv,
    down: Option<Conv>,
}

impl Basic {
    fn load(p: &Params, i: usize, o: usize, stride: usize) -> Result<Self> {
        let down = if i != o || stride != 1 {
            Some(p.pp("downsample.conv").conv(i, o, 1, ConvOpts::same(1).stride(stride).padding(0).no_bias(), Some(&p.pp("downsample.bn")))?)
        } else {
            None
        };
        Ok(Self {
            c1: p.pp("conv1").conv(i, o, 3, ConvOpts::same(3).stride(stride).no_bias(), Some(&p.pp("bn1")))?,
            c2: p.pp("conv2").conv(o, o, 3, ConvOpts::same(3).no_bias(), Some(&p.pp("bn2")))?,
            down,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let r = self.c2.forward(&self.c1.forward(x)?.relu()?)?;
        let s = match &self.down {
            Some(d) => d.forward(x)?,
            None => x.clone(),
        };
        Ok((r + s)?.relu()?)
    }
}

struct Arm {
    conv: Conv,
    atten: Conv,
}

impl Arm {
    fn load(p: &Params, i: usize, o: usize) -> Result<Self> {
        Ok(Self {
            conv: cbr(&p.pp("conv"), i, o, 3, 1)?,
            atten: p.pp("conv_atten").conv(o, o, 1, ConvOpts::same(1).no_bias(), Some(&p.pp("bn_atten")))?,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let f = self.conv.forward(x)?.relu()?;
        let a = candle_nn::ops::sigmoid(&self.atten.forward(&global_avg(&f)?)?)?;
        Ok(f.broadcast_mul(&a)?)
    }
}

/// The face parser.
pub struct BiSeNet {
    stem: Conv,
    layers: Vec<Vec<Basic>>,
    arm16: Arm,
    arm32: Arm,
    head16: Conv,
    head32: Conv,
    avg: Conv,
    ffm_blk: Conv,
    ffm1: Conv,
    ffm2: Conv,
    out_conv: Conv,
    out: Conv,
}

impl BiSeNet {
    pub fn load(p: &Params) -> Result<Self> {
        if p.shape("conv_out.conv_out.weight").is_none_or(|s| s.first() != Some(&CLASSES)) {
            return Err(Error::Model(format!("not a {CLASSES}-class BiSeNet face parser")));
        }
        let r = p.pp("cp.resnet");
        let mut layers = Vec::with_capacity(4);
        let mut i = 64;
        for (n, o) in [(1, 64), (2, 128), (3, 256), (4, 512)] {
            let stride = if n == 1 { 1 } else { 2 };
            let l = r.pp(&format!("layer{n}"));
            layers.push(vec![Basic::load(&l.pp("0"), i, o, stride)?, Basic::load(&l.pp("1"), o, o, 1)?]);
            i = o;
        }
        let cp = p.pp("cp");
        let ffm = p.pp("ffm");
        let co = p.pp("conv_out");
        Ok(Self {
            stem: r.pp("conv1").conv(3, 64, 7, ConvOpts::same(7).stride(2).no_bias(), Some(&r.pp("bn1")))?,
            layers,
            arm16: Arm::load(&cp.pp("arm16"), 256, 128)?,
            arm32: Arm::load(&cp.pp("arm32"), 512, 128)?,
            head16: cbr(&cp.pp("conv_head16"), 128, 128, 3, 1)?,
            head32: cbr(&cp.pp("conv_head32"), 128, 128, 3, 1)?,
            avg: cbr(&cp.pp("conv_avg"), 512, 128, 1, 1)?,
            ffm_blk: cbr(&ffm.pp("convblk"), 256, 256, 1, 1)?,
            ffm1: ffm.pp("conv1").conv(256, 64, 1, ConvOpts::same(1).no_bias(), None)?,
            ffm2: ffm.pp("conv2").conv(64, 256, 1, ConvOpts::same(1).no_bias(), None)?,
            out_conv: cbr(&co.pp("conv"), 256, 256, 3, 1)?,
            out: co.pp("conv_out").conv(256, CLASSES, 1, ConvOpts::same(1).no_bias(), None)?,
        })
    }

    /// Logits at an eighth of the input's size, `B × 19 × H/8 × W/8`, for a normalised `B × 3 × H × W` input
    /// (H and W multiples of 32).
    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let x = self.stem.forward(x)?.relu()?;
        // max pool 3, stride 2, padding 1: after a ReLU, zero padding is the same as minus infinity
        let x = x.pad_with_zeros(2, 1, 1)?.pad_with_zeros(3, 1, 1)?.max_pool2d_with_stride(3, 2)?;
        let mut feats = Vec::with_capacity(4);
        let mut t = x;
        for layer in &self.layers {
            for b in layer {
                t = b.forward(&t)?;
            }
            feats.push(t.clone());
        }
        let [_, f8, f16, f32_] = feats.as_slice() else { return Err(Error::Model("BiSeNet: missing stages".into())) };
        let (h8, w8) = (f8.dim(2)?, f8.dim(3)?);
        let (h16, w16) = (f16.dim(2)?, f16.dim(3)?);
        let (h32, w32) = (f32_.dim(2)?, f32_.dim(3)?);
        let avg = self.avg.forward(&global_avg(f32_)?)?.relu()?.upsample_nearest2d(h32, w32)?;
        let s32 = (self.arm32.forward(f32_)? + avg)?;
        let up32 = self.head32.forward(&s32.upsample_nearest2d(h16, w16)?)?.relu()?;
        let s16 = (self.arm16.forward(f16)? + up32)?;
        let up16 = self.head16.forward(&s16.upsample_nearest2d(h8, w8)?)?.relu()?;
        let feat = self.ffm_blk.forward(&Tensor::cat(&[f8, &up16], 1)?)?.relu()?;
        let a = candle_nn::ops::sigmoid(&self.ffm2.forward(&self.ffm1.forward(&global_avg(&feat)?)?.relu()?)?)?;
        let fused = (feat.broadcast_mul(&a)? + &feat)?;
        Ok(self.out.forward(&self.out_conv.forward(&fused)?.relu()?)?)
    }
}
