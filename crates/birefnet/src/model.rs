//! BiRefNet: the Swin backbone run on the image and on a half-size copy, a squeeze block over
//! the joined deepest features, and the bilateral-reference decoder that brings the image's own
//! patches back in at every scale.
//!
//! Ported from BiRefNet's `birefnet.py` (MIT, Copyright 2024 Peng Zheng): the inference path with
//! the released settings (`mul_scl_ipt=cat`, `cxt_num=3`, `dec_ipt` with split, `ms_supervision`
//! and `out_ref` gates, deformable ASPP).

use candle_core::{DType, Tensor};
use candle_nn::{Module, ops::sigmoid};

use crate::blocks::DecBlock;
use crate::params::{Conv, ConvOpts, Params, resize};
use crate::swin::{Swin, SwinConfig};
use crate::{Error, Result};

/// A gradient attention gate: `p · sigmoid(attn(relu(conv(p))))`.
struct Gate {
    conv: Conv,
    attn: Conv,
}

impl Gate {
    fn load(p: &Params, level: usize, ch: usize) -> Result<Self> {
        Ok(Self {
            conv: p.pp(&format!("gdt_convs_{level}.0")).conv(ch, 16, 3, ConvOpts::same(3), Some(&p.pp(&format!("gdt_convs_{level}.1"))))?,
            attn: p.pp(&format!("gdt_convs_attn_{level}.0")).conv(16, 1, 1, ConvOpts::same(1), None)?,
        })
    }

    fn apply(&self, p: &Tensor) -> Result<Tensor> {
        let g = sigmoid(&self.attn.forward(&self.conv.forward(p)?.relu()?)?)?;
        Ok(p.broadcast_mul(&g)?)
    }
}

/// The two-convolution block that turns a tile-stacked copy of the image into features.
struct Ipt {
    conv1: Conv,
    out: Conv,
}

impl Ipt {
    fn load(p: &Params, cin: usize, cout: usize) -> Result<Self> {
        Ok(Self { conv1: p.pp("conv1").conv(cin, 64, 3, ConvOpts::same(3), None)?, out: p.pp("conv_out").conv(64, cout, 3, ConvOpts::same(3), None)? })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(self.out.forward(&self.conv1.forward(x)?)?)
    }
}

/// The image cut into a `grid × grid` set of tiles, each tile one channel group:
/// `b c (hg h) (wg w) -> b (c hg wg) h w`.
fn tiles(x: &Tensor, grid: usize) -> Result<Tensor> {
    let (b, c, h, w) = x.dims4()?;
    if grid == 0 || h % grid != 0 || w % grid != 0 {
        return Err(Error::Model(format!("an image of {h} × {w} does not divide into {grid} tiles")));
    }
    let (th, tw) = (h / grid, w / grid);
    Ok(x.reshape((b, c, grid, th, grid, tw))?.permute((0, 1, 2, 4, 3, 5))?.contiguous()?.reshape((b, c * grid * grid, th, tw))?)
}

pub struct Decoder {
    ipt: [Ipt; 5],
    blocks: [DecBlock; 4],
    lateral: [Conv; 3],
    gates: [Gate; 3],
    conv_out: Conv,
}

impl Decoder {
    fn load(p: &Params, ch: [usize; 4]) -> Result<Self> {
        let ipt = [
            Ipt::load(&p.pp("ipt_blk1"), 3, ch[3] / 8)?,
            Ipt::load(&p.pp("ipt_blk2"), 3 * 16, ch[2] / 8)?,
            Ipt::load(&p.pp("ipt_blk3"), 3 * 64, ch[1] / 8)?,
            Ipt::load(&p.pp("ipt_blk4"), 3 * 256, ch[0] / 8)?,
            Ipt::load(&p.pp("ipt_blk5"), 3 * 1024, ch[0] / 8)?,
        ];
        let blocks = [
            DecBlock::load(&p.pp("decoder_block1"), ch[3] + ch[2] / 8, ch[3] / 2)?,
            DecBlock::load(&p.pp("decoder_block2"), ch[2] + ch[1] / 8, ch[3])?,
            DecBlock::load(&p.pp("decoder_block3"), ch[1] + ch[0] / 8, ch[2])?,
            DecBlock::load(&p.pp("decoder_block4"), ch[0] + ch[0] / 8, ch[1])?,
        ];
        let lat = |i: usize, c: usize| -> Result<Conv> { p.pp(&format!("lateral_block{i}.conv")).conv(c, c, 1, ConvOpts::same(1), None) };
        let lateral = [lat(2, ch[3])?, lat(3, ch[2])?, lat(4, ch[1])?];
        let gates = [Gate::load(p, 2, ch[3])?, Gate::load(p, 3, ch[2])?, Gate::load(p, 4, ch[1])?];
        let conv_out = p.pp("conv_out1.0").conv(ch[3] / 2 + ch[3] / 8, 1, 1, ConvOpts::same(1), None)?;
        Ok(Self { ipt, blocks, lateral, gates, conv_out })
    }

    /// `x` is the image, `x1..x3` the joined backbone features and `x4` the squeezed deepest one.
    /// Returns the final logits at the image's size.
    fn forward(&self, x: &Tensor, feats: [&Tensor; 4]) -> Result<Tensor> {
        let [x1, x2, x3, x4] = feats;
        let (_, _, ih, iw) = x.dims4()?;
        let dims = |t: &Tensor| -> Result<(usize, usize)> {
            let (_, _, h, w) = t.dims4()?;
            Ok((h, w))
        };
        // grid of tiles that matches a feature map of the given height
        let grid = |h: usize| -> Result<usize> { if h == 0 { Err(Error::Model("empty feature map".into())) } else { Ok(ih / h) } };

        let (h4, _) = dims(x4)?;
        let p4 = self.blocks[3].forward(&Tensor::cat(&[x4.clone(), self.ipt[4].forward(&tiles(x, grid(h4)?)?)?], 1)?)?;
        let p4 = self.gates[2].apply(&p4)?;
        let (h3, w3) = dims(x3)?;
        let up3 = (resize(&p4, h3, w3)? + self.lateral[2].forward(x3)?)?;

        let p3 = self.blocks[2].forward(&Tensor::cat(&[up3.clone(), self.ipt[3].forward(&tiles(x, grid(h3)?)?)?], 1)?)?;
        let p3 = self.gates[1].apply(&p3)?;
        let (h2, w2) = dims(x2)?;
        let up2 = (resize(&p3, h2, w2)? + self.lateral[1].forward(x2)?)?;

        let p2 = self.blocks[1].forward(&Tensor::cat(&[up2.clone(), self.ipt[2].forward(&tiles(x, grid(h2)?)?)?], 1)?)?;
        let p2 = self.gates[0].apply(&p2)?;
        let (h1, w1) = dims(x1)?;
        let up1 = (resize(&p2, h1, w1)? + self.lateral[0].forward(x1)?)?;

        let p1 = self.blocks[0].forward(&Tensor::cat(&[up1, self.ipt[1].forward(&tiles(x, grid(h1)?)?)?], 1)?)?;
        let p1 = resize(&p1, ih, iw)?;
        let p1 = Tensor::cat(&[p1, self.ipt[0].forward(x)?], 1)?;
        Ok(self.conv_out.forward(&p1)?)
    }
}

/// The whole network.
pub struct Net {
    swin: Swin,
    squeeze: DecBlock,
    decoder: Decoder,
}

impl Net {
    pub fn load(p: &Params) -> Result<Self> {
        let cfg = SwinConfig::detect(p)?;
        let e = cfg.embed;
        // joined channels of the four stages (image and half-size image side by side)
        let ch = [e * 16, e * 8, e * 4, e * 2];
        let swin = Swin::load(p, &cfg)?;
        // the deepest features joined with the three shallower ones
        let squeeze = DecBlock::load(&p.pp("squeeze_module.0"), ch[0] + ch[1] + ch[2] + ch[3], ch[0])?;
        let decoder = Decoder::load(&p.pp("decoder"), ch)?;
        Ok(Self { swin, squeeze, decoder })
    }

    /// Logits `[1, 1, S, S]` for the normalised image `x` `[1, 3, S, S]`. `cancel` is polled
    /// between backbone blocks.
    pub fn forward(&self, x: &Tensor, cancel: &dyn Fn() -> bool) -> Result<Tensor> {
        let x = x.to_dtype(DType::F32)?;
        let (_, _, h, w) = x.dims4()?;
        let full = self.swin.forward(&x, cancel)?;
        let half = self.swin.forward(&resize(&x, h / 2, w / 2)?, cancel)?;
        let mut joined = Vec::with_capacity(4);
        for (a, b) in full.iter().zip(&half) {
            let (_, _, fh, fw) = a.dims4()?;
            joined.push(Tensor::cat(&[a.clone(), resize(b, fh, fw)?], 1)?);
        }
        let [x1, x2, x3, x4] = <[Tensor; 4]>::try_from(joined).map_err(|_| Error::Model("the backbone returned the wrong number of stages".into()))?;
        let (h4, w4) = (x4.dim(2)?, x4.dim(3)?);
        let ctx = Tensor::cat(&[resize(&x1, h4, w4)?, resize(&x2, h4, w4)?, resize(&x3, h4, w4)?, x4], 1)?;
        let x4 = self.squeeze.forward(&ctx)?;
        self.decoder.forward(&x, [&x1, &x2, &x3, &x4])
    }
}
