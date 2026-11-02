//! The two pretrained backbones RDNet uses: FocalNet-L (the feature extractor, mingcv/RDNet
//! `models/arch/focalnet.py`, from microsoft/FocalNet, MIT) and ConvNeXt-Small (the classifier that turns
//! the picture into the six numbers prompting the network, timm). Inference only.
use crate::ops::{R, conv, conv1x1, depthwise, layer_norm, layer_norm_eps, linear, norm2d};
use candle_core::Tensor;
use candle_nn::VarBuilder;

const EMBED: usize = 192;
const DEPTHS: [usize; 4] = [2, 2, 18, 2];
const FOCAL_LEVELS: usize = 4;

fn tokens(x: &Tensor) -> R<Tensor> {
    let (_, c, h, w) = x.dims4()?;
    x.reshape((c, h * w))?.t()?.contiguous()
}

fn image(t: &Tensor, c: usize, h: usize, w: usize) -> R<Tensor> {
    t.t()?.contiguous()?.reshape((1, c, h, w))
}

/// FocalNet-L (4 focal levels, window 3, post-norm, layer scale, stride-2 stem). Returns the four stage
/// outputs (192, 384, 768, 1536 channels at 1/2, 1/4, 1/8, 1/16 of the input) and the stem output.
pub(crate) fn focalnet(vb: &VarBuilder<'static>, input: &Tensor) -> R<(Vec<Tensor>, Tensor)> {
    let (_, _, h0, w0) = input.dims4()?;
    let (mut h, mut w) = (h0.div_ceil(2), w0.div_ceil(2));
    let embedded = conv(&vb.pp("patch_embed.proj"), input, (3, EMBED, 7), 2, 3)?;
    let stem = layer_norm(&vb.pp("patch_embed.norm"), &tokens(&embedded)?, EMBED)?;
    let mut x = stem.clone();
    let mut outs = Vec::with_capacity(4);
    for stage in 0..4 {
        let dim = EMBED << stage;
        for block in 0..DEPTHS[stage] {
            x = focal_block(&vb.pp(format!("layers.{stage}.blocks.{block}")), &x, (dim, h, w))?;
        }
        outs.push(image(&layer_norm(&vb.pp(format!("norm{stage}")), &x, dim)?, dim, h, w)?);
        if stage < 3 {
            let down = vb.pp(format!("layers.{stage}.downsample"));
            let moved = conv(&down.pp("proj"), &image(&x, dim, h, w)?, (dim, dim * 2, 3), 2, 1)?;
            (h, w) = (h.div_ceil(2), w.div_ceil(2));
            x = layer_norm(&down.pp("norm"), &tokens(&moved)?, dim * 2)?;
        }
    }
    Ok((outs, image(&stem, EMBED, h0.div_ceil(2), w0.div_ceil(2))?))
}

fn focal_block(vb: &VarBuilder<'static>, x: &Tensor, spec: (usize, usize, usize)) -> R<Tensor> {
    let c = spec.0;
    let modulated = layer_norm(&vb.pp("norm1"), &modulation(&vb.pp("modulation"), x, spec)?, c)?;
    let x = (x + modulated.broadcast_mul(&vb.get(c, "gamma_1")?)?)?;
    let hidden = linear(&vb.pp("mlp.fc1"), &x, c, c * 4, true)?.gelu_erf()?;
    let mlp = linear(&vb.pp("mlp.fc2"), &hidden, c * 4, c, true)?;
    &x + layer_norm(&vb.pp("norm2"), &mlp, c)?.broadcast_mul(&vb.get(c, "gamma_2")?)?
}

/// `FocalModulation`: context gathered by depthwise convolutions of growing size, gated per level, then
/// multiplied into the query.
fn modulation(vb: &VarBuilder<'static>, x: &Tensor, spec: (usize, usize, usize)) -> R<Tensor> {
    let (c, h, w) = spec;
    let f = image(&linear(&vb.pp("f"), x, c, 2 * c + FOCAL_LEVELS + 1, true)?, 2 * c + FOCAL_LEVELS + 1, h, w)?;
    let q = f.narrow(1, 0, c)?;
    let mut ctx = f.narrow(1, c, c)?;
    let gates = f.narrow(1, 2 * c, FOCAL_LEVELS + 1)?;
    let mut all: Option<Tensor> = None;
    for level in 0..FOCAL_LEVELS {
        ctx = depthwise(&vb.pp(format!("focal_layers.{level}.0")), &ctx, c, 2 * level + 3, false)?.gelu_erf()?;
        let term = ctx.broadcast_mul(&gates.narrow(1, level, 1)?)?;
        all = Some(match all {
            Some(sum) => (sum + term)?,
            None => term,
        });
    }
    let global = ctx.mean_keepdim((2, 3))?.gelu_erf()?;
    let all = all.ok_or_else(|| candle_core::Error::Msg("no focal levels".into()))?;
    let all = ((all + global.broadcast_mul(&gates.narrow(1, FOCAL_LEVELS, 1)?)?)? / (FOCAL_LEVELS + 1) as f64)?;
    let out = q.broadcast_mul(&conv1x1(&vb.pp("h"), &all, c, c)?)?;
    linear(&vb.pp("proj"), &tokens(&out)?, c, c, true)
}

/// ConvNeXt-Small (ImageNet-22k) as timm builds it, `num_classes = 0`, followed by the 768→6 head:
/// the picture resized to 224×224 (align corners) in, six numbers out.
pub(crate) fn classifier(vb: &VarBuilder<'static>, input: &Tensor) -> R<Tensor> {
    const DIMS: [usize; 4] = [96, 192, 384, 768];
    const BLOCKS: [usize; 4] = [3, 3, 27, 3];
    let model = vb.pp("model");
    let small = input.upsample_bilinear2d(224, 224, true)?;
    let mut x = norm2d(&model.pp("stem.1"), &conv(&model.pp("stem.0"), &small, (3, DIMS[0], 4), 4, 0)?, DIMS[0])?;
    for stage in 0..4 {
        let dim = DIMS[stage];
        let sv = model.pp(format!("stages.{stage}"));
        if stage > 0 {
            let down = sv.pp("downsample");
            x = conv(&down.pp("1"), &norm2d(&down.pp("0"), &x, DIMS[stage - 1])?, (DIMS[stage - 1], dim, 2), 2, 0)?;
        }
        for block in 0..BLOCKS[stage] {
            x = convnext_block(&sv.pp(format!("blocks.{block}")), &x, dim)?;
        }
    }
    let pooled = x.mean_keepdim((2, 3))?;
    let pooled = norm2d(&model.pp("head.norm"), &pooled, DIMS[3])?.reshape((1, DIMS[3]))?;
    linear(&vb.pp("head"), &pooled, DIMS[3], 6, true)
}

fn convnext_block(vb: &VarBuilder<'static>, x: &Tensor, c: usize) -> R<Tensor> {
    let (_, _, h, w) = x.dims4()?;
    let y = depthwise(&vb.pp("conv_dw"), x, c, 7, true)?;
    let y = layer_norm_eps(&vb.pp("norm"), &tokens(&y)?, c, 1e-6)?;
    let y = linear(&vb.pp("mlp.fc1"), &y, c, c * 4, true)?.gelu_erf()?;
    let y = linear(&vb.pp("mlp.fc2"), &y, c * 4, c, true)?.broadcast_mul(&vb.get(c, "gamma")?)?;
    x + image(&y, c, h, w)?
}
