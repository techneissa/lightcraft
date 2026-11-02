//! Small tensor helpers shared by the DIRS network and its Swin prior.
use candle_core::{D, DType, Device, Tensor};
use candle_nn::VarBuilder;

pub(crate) type R<T> = candle_core::Result<T>;

/// Metal on macOS unless the caller asks for the CPU or Metal is unavailable.
pub(crate) fn device(cpu: bool) -> Device {
    #[cfg(target_os = "macos")]
    if !cpu && let Ok(d) = Device::new_metal(0) {
        return d;
    }
    let _ = cpu;
    Device::Cpu
}

/// 1×1 convolution with bias on a batch-1 NCHW tensor, as one matrix product.
pub(crate) fn conv1x1(vb: &VarBuilder<'static>, x: &Tensor, cin: usize, cout: usize) -> R<Tensor> {
    let (_, c, h, w) = x.dims4()?;
    if c != cin {
        candle_core::bail!("1x1 convolution expects {cin} channels, got {c}");
    }
    let weight = vb.get((cout, cin, 1, 1), "weight")?.reshape((cout, cin))?;
    let bias = vb.get(cout, "bias")?.reshape((cout, 1))?;
    weight.matmul(&x.reshape((cin, h * w))?)?.broadcast_add(&bias)?.reshape((1, cout, h, w))
}

/// A k×k convolution with bias.
pub(crate) fn conv(vb: &VarBuilder<'static>, x: &Tensor, dims: (usize, usize, usize), stride: usize, pad: usize) -> R<Tensor> {
    let (cin, cout, k) = dims;
    let weight = vb.get((cout, cin, k, k), "weight")?;
    let y = crate::net::conv2d(x, &weight, pad, stride)?;
    y.broadcast_add(&vb.get(cout, "bias")?.reshape((1, cout, 1, 1))?)
}

/// Per-pixel layer norm over the channels of an NCHW tensor (DIRS `LayerNorm2d`, eps 1e-6).
pub(crate) fn norm2d(vb: &VarBuilder<'static>, x: &Tensor, c: usize) -> R<Tensor> {
    let centered = x.broadcast_sub(&x.mean_keepdim(1)?)?;
    let std = (centered.sqr()?.mean_keepdim(1)? + 1e-6)?.sqrt()?;
    centered
        .broadcast_div(&std)?
        .broadcast_mul(&vb.get(c, "weight")?.reshape((1, c, 1, 1))?)?
        .broadcast_add(&vb.get(c, "bias")?.reshape((1, c, 1, 1))?)
}

/// 3×3 depthwise convolution with zero padding as nine shifted products: candle's grouped
/// convolution dispatches once per channel.
pub(crate) fn depthwise3x3(vb: &VarBuilder<'static>, x: &Tensor, c: usize) -> R<Tensor> {
    let (_, _, h, w) = x.dims4()?;
    let kernel = vb.get((c, 1, 3, 3), "weight")?;
    let padded = x.pad_with_zeros(2, 1, 1)?.pad_with_zeros(3, 1, 1)?;
    let mut y: Option<Tensor> = None;
    for dy in 0..3 {
        for dx in 0..3 {
            let weight = kernel.narrow(2, dy, 1)?.narrow(3, dx, 1)?.reshape((1, c, 1, 1))?;
            let term = padded.narrow(2, dy, h)?.narrow(3, dx, w)?.broadcast_mul(&weight)?;
            y = Some(match y {
                Some(sum) => (sum + term)?,
                None => term,
            });
        }
    }
    let y = y.ok_or_else(|| candle_core::Error::Msg("empty depthwise kernel".into()))?;
    y.broadcast_add(&vb.get(c, "bias")?.reshape((1, c, 1, 1))?)
}

/// Layer norm over the last dimension of a token matrix (`torch.nn.LayerNorm`, eps 1e-5).
pub(crate) fn layer_norm(vb: &VarBuilder<'static>, x: &Tensor, c: usize) -> R<Tensor> {
    layer_norm_eps(vb, x, c, 1e-5)
}

/// The same with the epsilon of the network (1e-6 in ConvNeXt and RDNet).
pub(crate) fn layer_norm_eps(vb: &VarBuilder<'static>, x: &Tensor, c: usize, eps: f64) -> R<Tensor> {
    let centered = x.broadcast_sub(&x.mean_keepdim(D::Minus1)?)?;
    let std = (centered.sqr()?.mean_keepdim(D::Minus1)? + eps)?.sqrt()?;
    centered.broadcast_div(&std)?.broadcast_mul(&vb.get(c, "weight")?)?.broadcast_add(&vb.get(c, "bias")?)
}

/// `torch.nn.Linear` on a token matrix `[n, cin]`.
pub(crate) fn linear(vb: &VarBuilder<'static>, x: &Tensor, cin: usize, cout: usize, bias: bool) -> R<Tensor> {
    let weight = vb.get((cout, cin), "weight")?.t()?.contiguous()?;
    let y = x.matmul(&weight)?;
    if bias { y.broadcast_add(&vb.get(cout, "bias")?) } else { Ok(y) }
}

pub(crate) fn to_f32_vec(t: &Tensor) -> R<Vec<f32>> {
    t.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()
}

/// k×k depthwise convolution (odd k, zero padding) as k² shifted products; the bias is optional.
pub(crate) fn depthwise(vb: &VarBuilder<'static>, x: &Tensor, c: usize, k: usize, bias: bool) -> R<Tensor> {
    let (_, _, h, w) = x.dims4()?;
    let kernel = vb.get((c, 1, k, k), "weight")?;
    let r = k / 2;
    let padded = x.pad_with_zeros(2, r, r)?.pad_with_zeros(3, r, r)?;
    let mut y: Option<Tensor> = None;
    for dy in 0..k {
        for dx in 0..k {
            let weight = kernel.narrow(2, dy, 1)?.narrow(3, dx, 1)?.reshape((1, c, 1, 1))?;
            let term = padded.narrow(2, dy, h)?.narrow(3, dx, w)?.broadcast_mul(&weight)?;
            y = Some(match y {
                Some(sum) => (sum + term)?,
                None => term,
            });
        }
    }
    let y = y.ok_or_else(|| candle_core::Error::Msg("empty depthwise kernel".into()))?;
    if bias { y.broadcast_add(&vb.get(c, "bias")?.reshape((1, c, 1, 1))?) } else { Ok(y) }
}

/// A `[1, 3, h, w]` input in [0, 1] from 8-bit RGB, edge-replicated to multiples of 32.
pub(crate) fn input_tensor(rgb: &[u8], w: usize, h: usize, device: &Device) -> R<Tensor> {
    let data: Vec<f32> = rgb.iter().map(|x| *x as f32 / 255.0).collect();
    let mut input = Tensor::from_vec(data, (1, h, w, 3), device)?.permute((0, 3, 1, 2))?.contiguous()?;
    if !w.is_multiple_of(32) {
        let n = 32 - w % 32;
        input = Tensor::cat(&[&input, &input.narrow(3, w - 1, 1)?.repeat((1, 1, 1, n))?], 3)?;
    }
    if !h.is_multiple_of(32) {
        let n = 32 - h % 32;
        input = Tensor::cat(&[&input, &input.narrow(2, h - 1, 1)?.repeat((1, 1, n, 1))?], 2)?;
    }
    Ok(input)
}

/// The padded `[1, 3, H, W]` result cropped to `w`×`h` as clamped RGB pixels; a non-finite value is an error.
pub(crate) fn output_pixels(out: &Tensor, w: usize, h: usize) -> R<Vec<[f32; 3]>> {
    let flat = to_f32_vec(&out.narrow(2, 0, h)?.narrow(3, 0, w)?.permute((0, 2, 3, 1))?)?;
    let mut pixels = Vec::with_capacity(w * h);
    for p in flat.as_chunks::<3>().0 {
        if !p.iter().all(|x| x.is_finite()) {
            candle_core::bail!("non-finite model output");
        }
        pixels.push(p.map(|x| x.clamp(0.0, 1.0)));
    }
    Ok(pixels)
}
