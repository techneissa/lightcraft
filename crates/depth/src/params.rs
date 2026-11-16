//! Loading the network's layers from the weights: convolutions (plain and transposed), linear layers and layer
//! norms, each checked against the shape the architecture wants before any bytes are read.

use std::sync::Arc;

use candle_core::{Device, Tensor};
use candle_nn::{LayerNorm, Linear, Module};

use crate::weights::Weights;
use crate::{Error, Result};

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
        Self { w: self.w.clone(), dev: self.dev.clone(), prefix: self.key(name) }
    }

    fn key(&self, name: &str) -> String {
        if self.prefix.is_empty() { name.to_string() } else { format!("{}.{name}", self.prefix) }
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

    /// Tensor `name` on the CPU as F32, after its stored shape matched `shape`.
    pub fn tensor_cpu(&self, name: &str, shape: &[usize]) -> Result<Tensor> {
        let key = self.key(name);
        match self.w.shape(&key) {
            None => Err(Error::Model(format!("missing tensor `{key}`"))),
            Some(s) if s != shape => Err(Error::Model(format!("shape mismatch for `{key}`: expected {shape:?}, found {s:?}"))),
            Some(_) => self.w.read(&key),
        }
    }

    pub fn layer_norm(&self, dim: usize, eps: f64) -> Result<LayerNorm> {
        Ok(LayerNorm::new(self.tensor("weight", &[dim])?, self.tensor("bias", &[dim])?, eps))
    }

    pub fn linear(&self, input: usize, output: usize) -> Result<Linear> {
        Ok(Linear::new(self.tensor("weight", &[output, input])?, Some(self.tensor("bias", &[output])?)))
    }

    /// A convolution `input → output` with a square kernel `k`, `stride` and `padding`.
    pub fn conv(&self, input: usize, output: usize, k: usize, (stride, padding): (usize, usize), bias: bool) -> Result<Conv> {
        let w = self.tensor("weight", &[output, input, k, k])?;
        let b = if bias { Some(self.tensor("bias", &[output])?) } else { None };
        Ok(Conv { w, b, stride, padding, transposed: false })
    }

    /// A transposed convolution `channels → channels` with kernel and stride `k` (no padding): PyTorch's
    /// `ConvTranspose2d(c, c, k, stride=k)`, whose weight is `[in, out, k, k]`.
    pub fn conv_t(&self, channels: usize, k: usize) -> Result<Conv> {
        let w = self.tensor("weight", &[channels, channels, k, k])?;
        let b = Some(self.tensor("bias", &[channels])?);
        Ok(Conv { w, b, stride: k, padding: 0, transposed: true })
    }
}

/// A 2-D convolution, plain or transposed.
pub struct Conv {
    w: Tensor,
    b: Option<Tensor>,
    stride: usize,
    padding: usize,
    transposed: bool,
}

impl Module for Conv {
    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let y = if self.transposed {
            x.conv_transpose2d(&self.w, self.padding, 0, self.stride, 1)?
        } else {
            x.conv2d(&self.w, self.padding, self.stride, 1, 1)?
        };
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
