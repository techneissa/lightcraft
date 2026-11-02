//! Loading the model's layers from the weights: convolutions (with their batch norm folded in),
//! linear layers and layer norms, each checked against the shape the architecture wants before
//! any bytes are read.

use std::sync::Arc;

use candle_core::{Device, Tensor};
use candle_nn::{Conv2dConfig, LayerNorm, Linear, Module};

use crate::weights::Weights;
use crate::{Error, Result};

/// PyTorch's default `BatchNorm2d` epsilon.
const BN_EPS: f64 = 1e-5;

/// A place in the weights: the file, the device the layers go to, and the name prefix.
#[derive(Clone)]
pub struct Params {
    w: Arc<Weights>,
    dev: Device,
    prefix: String,
}

impl Params {
    pub fn new(w: Arc<Weights>, dev: Device) -> Self {
        Self { w, dev, prefix: String::new() }
    }

    pub fn pp(&self, name: &str) -> Self {
        let prefix = if self.prefix.is_empty() { name.to_string() } else { format!("{}.{name}", self.prefix) };
        Self { w: self.w.clone(), dev: self.dev.clone(), prefix }
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

    pub fn layer_norm(&self, dim: usize, eps: f64) -> Result<LayerNorm> {
        Ok(LayerNorm::new(self.tensor("weight", &[dim])?, self.tensor("bias", &[dim])?, eps))
    }

    pub fn linear(&self, input: usize, output: usize, bias: bool) -> Result<Linear> {
        let w = self.tensor("weight", &[output, input])?;
        let b = if bias { Some(self.tensor("bias", &[output])?) } else { None };
        Ok(Linear::new(w, b))
    }

    /// A convolution `input → output` with a square kernel `k`. `bn` is the name of a batch norm
    /// that follows it in the model (folded into the weights; absent from models trained with a
    /// batch size of one, which have none).
    pub fn conv(&self, input: usize, output: usize, k: usize, opts: ConvOpts, bn: Option<&Params>) -> Result<Conv> {
        let mut w = self.tensor("weight", &[output, input, k, k])?;
        let mut b = if opts.bias { Some(self.tensor("bias", &[output])?) } else { None };
        if let Some(bn) = bn
            && bn.has("running_mean")
        {
            let gamma = bn.tensor("weight", &[output])?;
            let beta = bn.tensor("bias", &[output])?;
            let mean = bn.tensor("running_mean", &[output])?;
            let var = bn.tensor("running_var", &[output])?;
            let scale = (gamma / (var + BN_EPS)?.sqrt()?)?;
            w = w.broadcast_mul(&scale.reshape((output, 1, 1, 1))?)?;
            let shifted = match &b {
                Some(b) => (b - &mean)?,
                None => mean.neg()?,
            };
            b = Some((shifted * &scale)?.add(&beta)?);
        }
        let cfg = Conv2dConfig { padding: opts.padding, stride: opts.stride, dilation: 1, groups: 1, cudnn_fwd_algo: None };
        Ok(Conv { w, b, cfg })
    }
}

/// Convolution settings beyond the channels and kernel.
#[derive(Clone, Copy)]
pub struct ConvOpts {
    pub stride: usize,
    pub padding: usize,
    pub bias: bool,
}

impl ConvOpts {
    /// Stride 1, `same` padding for the odd kernel `k`, with a bias.
    pub fn same(k: usize) -> Self {
        Self { stride: 1, padding: k / 2, bias: true }
    }

    pub fn no_bias(self) -> Self {
        Self { bias: false, ..self }
    }
}

/// A 2-D convolution (batch norm already folded in where the model has one).
pub struct Conv {
    pub w: Tensor,
    pub b: Option<Tensor>,
    cfg: Conv2dConfig,
}

impl Module for Conv {
    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let y = x.conv2d(&self.w, self.cfg.padding, self.cfg.stride, self.cfg.dilation, self.cfg.groups)?;
        match &self.b {
            Some(b) => y.broadcast_add(&b.reshape((1, b.dim(0)?, 1, 1))?),
            None => Ok(y),
        }
    }
}

/// `F.interpolate(x, size, mode="bilinear", align_corners=True)`.
pub fn resize(x: &Tensor, h: usize, w: usize) -> Result<Tensor> {
    let (_, _, ih, iw) = x.dims4()?;
    if (ih, iw) == (h, w) {
        return Ok(x.clone());
    }
    Ok(x.upsample_bilinear2d(h, w, true)?)
}
