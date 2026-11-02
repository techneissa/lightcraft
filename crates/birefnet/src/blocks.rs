//! The decoder's building blocks: the deformable ASPP and the basic decoder block.
//!
//! Ported from BiRefNet's `aspp.py` and `decoder_blocks.py` (MIT, Copyright 2024 Peng Zheng).

use candle_core::Tensor;
use candle_nn::Module;

use crate::Result;
use crate::deform::DeformConv;
use crate::params::{Conv, ConvOpts, Params};

/// Width every ASPP branch is brought to.
const BRANCH: usize = 256;

/// Deformable ASPP: a 1×1, a 1×1/3×3/7×7 deformable branch each, and a global-average branch,
/// merged by a 1×1 convolution.
pub struct AsppDeformable {
    aspp1: DeformConv,
    branches: Vec<DeformConv>,
    pool: Conv,
    merge: Conv,
}

impl AsppDeformable {
    pub fn load(p: &Params, ch: usize) -> Result<Self> {
        let aspp1 = DeformConv::load(&p.pp("aspp1.atrous_conv"), ch, BRANCH, 1, Some(&p.pp("aspp1.bn")))?;
        let mut branches = Vec::new();
        for i in 0..8 {
            let q = p.pp(&format!("aspp_deforms.{i}"));
            let Some(shape) = q.shape("atrous_conv.regular_conv.weight") else { break };
            let k = shape.get(2).copied().unwrap_or(1);
            if k == 0 || k % 2 == 0 || k > 15 {
                return Err(crate::Error::Model(format!("aspp_deforms.{i}: unusable kernel size {k}")));
            }
            branches.push(DeformConv::load(&q.pp("atrous_conv"), ch, BRANCH, k, Some(&q.pp("bn")))?);
        }
        if branches.is_empty() {
            return Err(crate::Error::Model("the ASPP has no deformable branches".into()));
        }
        let pool = p.pp("global_avg_pool.1").conv(ch, BRANCH, 1, ConvOpts::same(1).no_bias(), Some(&p.pp("global_avg_pool.2")))?;
        let merge = p.pp("conv1").conv(BRANCH * (2 + branches.len()), ch, 1, ConvOpts::same(1).no_bias(), Some(&p.pp("bn1")))?;
        Ok(Self { aspp1, branches, pool, merge })
    }

    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (_, _, h, w) = x.dims4()?;
        let mut parts = vec![self.aspp1.forward(x)?.relu()?];
        for b in &self.branches {
            parts.push(b.forward(x)?.relu()?);
        }
        let pooled = self.pool.forward(&x.mean_keepdim(2)?.mean_keepdim(3)?)?.relu()?;
        // a 1×1 map interpolated to any size is that value everywhere
        parts.push(pooled.broadcast_as((1, BRANCH, h, w))?.contiguous()?);
        Ok(self.merge.forward(&Tensor::cat(&parts, 1)?)?.relu()?)
    }
}

/// `conv_in` (3×3) → ReLU → deformable ASPP → `conv_out` (3×3), the batch norms folded in.
pub struct DecBlock {
    conv_in: Conv,
    att: Option<AsppDeformable>,
    conv_out: Conv,
}

/// Width inside the block.
const INTER: usize = 64;

impl DecBlock {
    pub fn load(p: &Params, cin: usize, cout: usize) -> Result<Self> {
        let conv_in = p.pp("conv_in").conv(cin, INTER, 3, ConvOpts::same(3), Some(&p.pp("bn_in")))?;
        let att = if p.has("dec_att.conv1.weight") { Some(AsppDeformable::load(&p.pp("dec_att"), INTER)?) } else { None };
        let conv_out = p.pp("conv_out").conv(INTER, cout, 3, ConvOpts::same(3), Some(&p.pp("bn_out")))?;
        Ok(Self { conv_in, att, conv_out })
    }

    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut y = self.conv_in.forward(x)?.relu()?;
        if let Some(att) = &self.att {
            y = att.forward(&y)?;
        }
        Ok(self.conv_out.forward(&y)?)
    }
}
