//! SegFormer (Xie et al., NeurIPS 2021): a Mix Transformer encoder and an all-MLP decoder, as Hugging Face's
//! `SegformerForSemanticSegmentation` stores it. One architecture serves the face parser (MiT-b0, 19 face classes) and
//! the body parsers (MiT-b2 and b3, 18 clothes classes); the sizes come from the weights.
//!
//! Written from the paper and the layout of the published checkpoints (the `transformers` implementation is
//! Apache-2.0); every MiT size shares 1, 2, 5, 8 attention heads and reduction ratios 8, 4, 2, 1 per stage.

use candle_core::{D, Tensor};
use candle_nn::{LayerNorm, Linear, Module};

use crate::nn::{Conv, ConvOpts, Params};
use crate::{Error, Result};

const HEADS: [usize; 4] = [1, 2, 5, 8];
const SR: [usize; 4] = [8, 4, 2, 1];
const PATCH: [usize; 4] = [7, 3, 3, 3];
const STRIDE: [usize; 4] = [4, 2, 2, 2];
const LN_EPS: f64 = 1e-6;
/// More blocks per stage than any MiT size has (b5: 3, 6, 40, 3).
const MAX_DEPTH: usize = 64;

struct Attention {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
    sr: Option<(Conv, LayerNorm)>,
    heads: usize,
}

impl Attention {
    fn forward(&self, x: &Tensor, h: usize, w: usize) -> Result<Tensor> {
        let (b, n, c) = x.dims3()?;
        let d = c / self.heads;
        let split = |t: Tensor, len: usize| -> Result<Tensor> { Ok(t.reshape((b, len, self.heads, d))?.transpose(1, 2)?.contiguous()?) };
        let q = split(self.q.forward(x)?, n)?;
        let kv = match &self.sr {
            Some((conv, ln)) => {
                let img = x.transpose(1, 2)?.reshape((b, c, h, w))?;
                let r = conv.forward(&img)?;
                let m = r.dim(2)? * r.dim(3)?;
                ln.forward(&r.reshape((b, c, m))?.transpose(1, 2)?.contiguous()?)?
            }
            None => x.clone(),
        };
        let m = kv.dim(1)?;
        let k = split(self.k.forward(&kv)?, m)?;
        let v = split(self.v.forward(&kv)?, m)?;
        let scores = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? / (d as f64).sqrt())?;
        let probs = candle_nn::ops::softmax_last_dim(&scores)?;
        let ctx = probs.matmul(&v)?.transpose(1, 2)?.reshape((b, n, c))?;
        Ok(self.out.forward(&ctx)?)
    }
}

struct Block {
    ln1: LayerNorm,
    attn: Attention,
    ln2: LayerNorm,
    fc1: Linear,
    dw: Conv,
    fc2: Linear,
}

impl Block {
    fn load(p: &Params, c: usize, stage: usize) -> Result<Self> {
        let a = p.pp("attention.self");
        let sr = if SR[stage] > 1 {
            let k = SR[stage];
            Some((a.pp("sr").conv(c, c, k, ConvOpts::same(k).stride(k).padding(0), None)?, a.pp("layer_norm").layer_norm(c, LN_EPS)?))
        } else {
            None
        };
        let hidden = c * 4;
        Ok(Self {
            ln1: p.pp("layer_norm_1").layer_norm(c, LN_EPS)?,
            attn: Attention {
                q: a.pp("query").linear(c, c)?,
                k: a.pp("key").linear(c, c)?,
                v: a.pp("value").linear(c, c)?,
                out: p.pp("attention.output.dense").linear(c, c)?,
                sr,
                heads: HEADS[stage],
            },
            ln2: p.pp("layer_norm_2").layer_norm(c, LN_EPS)?,
            fc1: p.pp("mlp.dense1").linear(c, hidden)?,
            dw: p.pp("mlp.dwconv.dwconv").conv(hidden, hidden, 3, ConvOpts::same(3).groups(hidden), None)?,
            fc2: p.pp("mlp.dense2").linear(hidden, c)?,
        })
    }

    fn forward(&self, x: &Tensor, h: usize, w: usize) -> Result<Tensor> {
        let x = (self.attn.forward(&self.ln1.forward(x)?, h, w)? + x)?;
        let y = self.fc1.forward(&self.ln2.forward(&x)?)?;
        let (b, n, c) = y.dims3()?;
        let y = self.dw.forward(&y.transpose(1, 2)?.reshape((b, c, h, w))?)?;
        let y = y.reshape((b, c, n))?.transpose(1, 2)?.gelu_erf()?;
        Ok((self.fc2.forward(&y)? + x)?)
    }
}

struct Stage {
    embed: Conv,
    embed_ln: LayerNorm,
    blocks: Vec<Block>,
    ln: LayerNorm,
}

/// A SegFormer for semantic segmentation.
pub struct Segformer {
    stages: Vec<Stage>,
    proj: Vec<Linear>,
    fuse: Conv,
    classifier: Conv,
    classes: usize,
}

impl Segformer {
    /// Read a `SegformerForSemanticSegmentation` checkpoint; its sizes come from the tensors' shapes.
    pub fn load(p: &Params) -> Result<Self> {
        let enc = p.pp("segformer.encoder");
        let mut stages = Vec::with_capacity(4);
        let mut widths = Vec::with_capacity(4);
        let mut input = 3;
        for i in 0..4 {
            let pe = enc.pp(&format!("patch_embeddings.{i}"));
            let c = match pe.shape("proj.weight") {
                Some(&[c, ci, k, _]) if ci == input && k == PATCH[i] && c % HEADS[i] == 0 && (8..=2048).contains(&c) => c,
                other => return Err(Error::Model(format!("not a SegFormer: stage {i} patch embedding is {other:?}"))),
            };
            let depth = (0..MAX_DEPTH).take_while(|j| enc.has(&format!("block.{i}.{j}.layer_norm_1.weight"))).count();
            if depth == 0 {
                return Err(Error::Model(format!("not a SegFormer: stage {i} has no blocks")));
            }
            let blocks = (0..depth).map(|j| Block::load(&enc.pp(&format!("block.{i}.{j}")), c, i)).collect::<Result<Vec<_>>>()?;
            stages.push(Stage {
                embed: pe.pp("proj").conv(input, c, PATCH[i], ConvOpts::same(PATCH[i]).stride(STRIDE[i]), None)?,
                embed_ln: pe.pp("layer_norm").layer_norm(c, LN_EPS)?,
                blocks,
                ln: enc.pp(&format!("layer_norm.{i}")).layer_norm(c, LN_EPS)?,
            });
            widths.push(c);
            input = c;
        }
        let head = p.pp("decode_head");
        let proj_name = if head.has("linear_c.0.proj.weight") { "linear_c" } else { "linear_projections" };
        let dec = match head.shape(&format!("{proj_name}.0.proj.weight")) {
            Some(&[d, _]) if (8..=4096).contains(&d) => d,
            other => return Err(Error::Model(format!("not a SegFormer: decoder projection is {other:?}"))),
        };
        let proj = widths.iter().enumerate().map(|(i, &c)| head.pp(&format!("{proj_name}.{i}.proj")).linear(c, dec)).collect::<Result<Vec<_>>>()?;
        let fuse = head.pp("linear_fuse").conv(4 * dec, dec, 1, ConvOpts::same(1).no_bias(), Some(&head.pp("batch_norm")))?;
        let classes = match head.shape("classifier.weight") {
            Some(&[n, d, 1, 1]) if d == dec && (2..=256).contains(&n) => n,
            other => return Err(Error::Model(format!("not a SegFormer: classifier is {other:?}"))),
        };
        let classifier = head.pp("classifier").conv(dec, classes, 1, ConvOpts::same(1), None)?;
        Ok(Self { stages, proj, fuse, classifier, classes })
    }

    pub fn classes(&self) -> usize {
        self.classes
    }

    /// Logits at a quarter of the input's size, `B × classes × H/4 × W/4`, for a normalised `B × 3 × H × W` input.
    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut x = x.clone();
        let mut maps = Vec::with_capacity(4);
        for s in &self.stages {
            let e = s.embed.forward(&x)?;
            let (b, c, h, w) = e.dims4()?;
            let mut t = s.embed_ln.forward(&e.reshape((b, c, h * w))?.transpose(1, 2)?.contiguous()?)?;
            for blk in &s.blocks {
                t = blk.forward(&t, h, w)?;
            }
            let t = s.ln.forward(&t)?;
            x = t.transpose(1, 2)?.reshape((b, c, h, w))?.contiguous()?;
            maps.push(x.clone());
        }
        let (h0, w0) = match maps.first() {
            Some(m) => (m.dim(2)?, m.dim(3)?),
            None => return Err(Error::Model("SegFormer has no stages".into())),
        };
        let mut ups = Vec::with_capacity(4);
        for (m, proj) in maps.iter().zip(&self.proj).rev() {
            let (b, c, h, w) = m.dims4()?;
            let t = proj.forward(&m.reshape((b, c, h * w))?.transpose(1, 2)?.contiguous()?)?;
            let d = t.dim(D::Minus1)?;
            let t = t.transpose(1, 2)?.reshape((b, d, h, w))?.contiguous()?;
            ups.push(crate::nn::resize(&t, h0, w0, false)?);
        }
        let fused = self.fuse.forward(&Tensor::cat(&ups, 1)?)?.relu()?;
        Ok(self.classifier.forward(&fused)?)
    }
}
