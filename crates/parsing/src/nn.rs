//! Loading layers from the weights: linear layers, layer norms and convolutions (batch norm folded in), each checked
//! against the shape the architecture wants before any bytes are read. After `lightcraft-birefnet`'s `params.rs`.

use std::sync::Arc;

use candle_core::{Device, Tensor};
use candle_nn::{LayerNorm, Linear, Module};

use crate::weights::Weights;
use crate::{Error, Result};

/// PyTorch's default `BatchNorm2d` epsilon.
const BN_EPS: f64 = 1e-5;

/// How a file stores convolution kernels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// PyTorch: out, in, kh, kw.
    Oihw,
    /// MLX: out, kh, kw, in.
    Ohwi,
}

/// A place in the weights: the file, the device the layers go to, and the name prefix.
#[derive(Clone)]
pub struct Params {
    w: Arc<Weights>,
    dev: Device,
    prefix: String,
    layout: Layout,
}

impl Params {
    pub fn new(w: Arc<Weights>, dev: Device, layout: Layout) -> Self {
        Self { w, dev, prefix: String::new(), layout }
    }

    pub fn pp(&self, name: &str) -> Self {
        Self { w: self.w.clone(), dev: self.dev.clone(), prefix: self.key(name), layout: self.layout }
    }

    fn key(&self, name: &str) -> String {
        if self.prefix.is_empty() { name.to_string() } else { format!("{}.{name}", self.prefix) }
    }

    pub fn has(&self, name: &str) -> bool {
        self.w.contains(&self.key(name))
    }

    /// The stored shape of `name`, if it is there.
    pub fn shape(&self, name: &str) -> Option<&[usize]> {
        self.w.shape(&self.key(name))
    }

    /// Tensor `name` on the device as F32, after its stored shape matched `shape`.
    pub fn tensor(&self, name: &str, shape: &[usize]) -> Result<Tensor> {
        let key = self.key(name);
        match self.w.shape(&key) {
            None => return Err(Error::Model(format!("missing tensor `{key}`"))),
            Some(s) if s != shape => return Err(Error::Model(format!("shape mismatch for `{key}`: expected {shape:?}, found {s:?}"))),
            Some(_) => {}
        }
        Ok(self.w.read(&key)?.to_device(&self.dev)?)
    }

    /// Tensor `name` on the device in `dtype` (converted on the CPU first, so the device never holds a wider copy).
    pub fn tensor_as(&self, name: &str, shape: &[usize], dtype: candle_core::DType) -> Result<Tensor> {
        let key = self.key(name);
        match self.w.shape(&key) {
            None => return Err(Error::Model(format!("missing tensor `{key}`"))),
            Some(s) if s != shape => return Err(Error::Model(format!("shape mismatch for `{key}`: expected {shape:?}, found {s:?}"))),
            Some(_) => {}
        }
        Ok(self.w.read(&key)?.to_dtype(dtype)?.to_device(&self.dev)?)
    }

    pub fn linear(&self, input: usize, output: usize) -> Result<Linear> {
        Ok(Linear::new(self.tensor("weight", &[output, input])?, Some(self.tensor("bias", &[output])?)))
    }

    pub fn layer_norm(&self, dim: usize, eps: f64) -> Result<LayerNorm> {
        Ok(LayerNorm::new(self.tensor("weight", &[dim])?, self.tensor("bias", &[dim])?, eps))
    }

    /// A convolution kernel stored as `[output, input / groups, k, k]` (PyTorch) or its MLX order.
    fn kernel(&self, input: usize, output: usize, k: usize, groups: usize) -> Result<Tensor> {
        let per = input / groups.max(1);
        Ok(match self.layout {
            Layout::Oihw => self.tensor("weight", &[output, per, k, k])?,
            Layout::Ohwi => self.tensor("weight", &[output, k, k, per])?.permute((0, 3, 1, 2))?.contiguous()?,
        })
    }

    /// A convolution `input → output` with a square kernel `k`; `bn`, a batch norm that follows it, is folded in.
    pub fn conv(&self, input: usize, output: usize, k: usize, opts: ConvOpts, bn: Option<&Params>) -> Result<Conv> {
        let mut w = self.kernel(input, output, k, opts.groups)?;
        let mut b = if opts.bias { Some(self.tensor("bias", &[output])?) } else { None };
        if let Some(bn) = bn {
            let (gamma, beta) = (bn.tensor("weight", &[output])?, bn.tensor("bias", &[output])?);
            let (mean, var) = (bn.tensor("running_mean", &[output])?, bn.tensor("running_var", &[output])?);
            let scale = (gamma / (var + BN_EPS)?.sqrt()?)?;
            w = w.broadcast_mul(&scale.reshape((output, 1, 1, 1))?)?;
            let shifted = match &b {
                Some(b) => (b - &mean)?,
                None => mean.neg()?,
            };
            b = Some((shifted * &scale)?.add(&beta)?);
        }
        Ok(Conv { w, b, stride: opts.stride, padding: opts.padding, groups: opts.groups.max(1) })
    }
}

/// Convolution settings beyond the channels and kernel.
#[derive(Clone, Copy, Debug)]
pub struct ConvOpts {
    pub stride: usize,
    pub padding: usize,
    pub groups: usize,
    pub bias: bool,
}

impl ConvOpts {
    /// Stride 1, `same` padding for the odd kernel `k`, with a bias.
    pub fn same(k: usize) -> Self {
        Self { stride: 1, padding: k / 2, groups: 1, bias: true }
    }

    pub fn stride(self, stride: usize) -> Self {
        Self { stride, ..self }
    }

    pub fn padding(self, padding: usize) -> Self {
        Self { padding, ..self }
    }

    pub fn groups(self, groups: usize) -> Self {
        Self { groups, ..self }
    }

    pub fn no_bias(self) -> Self {
        Self { bias: false, ..self }
    }
}

/// A 2-D convolution (batch norm already folded in where the model has one).
pub struct Conv {
    pub w: Tensor,
    pub b: Option<Tensor>,
    stride: usize,
    padding: usize,
    groups: usize,
}

impl Conv {
    /// A convolution from a kernel and bias already in PyTorch order.
    pub fn from_parts(w: Tensor, b: Option<Tensor>, stride: usize, padding: usize, groups: usize) -> Self {
        Self { w, b, stride, padding, groups: groups.max(1) }
    }
}

impl Module for Conv {
    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let y = x.conv2d(&self.w, self.padding, self.stride, 1, self.groups)?;
        match &self.b {
            Some(b) => y.broadcast_add(&b.reshape((1, b.dim(0)?, 1, 1))?),
            None => Ok(y),
        }
    }
}

/// Bilinear resize of an NCHW tensor (`align_corners` as PyTorch's flag).
pub fn resize(x: &Tensor, h: usize, w: usize, align_corners: bool) -> Result<Tensor> {
    let (_, _, ih, iw) = x.dims4()?;
    if (ih, iw) == (h, w) {
        return Ok(x.clone());
    }
    Ok(x.upsample_bilinear2d(h, w, align_corners)?)
}

/// Global average pool of an NCHW tensor to N × C × 1 × 1.
pub fn global_avg(x: &Tensor) -> Result<Tensor> {
    Ok(x.mean_keepdim(3)?.mean_keepdim(2)?)
}
