//! Weights, and the few layers that run on the plain `(1, c, h, w)` layout: the stem and the three
//! downsampling layers (the channels-last layers of the residual part are in [`crate::cl`]).

use std::collections::BTreeMap;

use candle_core::{Device, Tensor};

use crate::{Error, Result};

/// BatchNorm's epsilon in PyTorch.
const BN_EPS: f32 = 1e-5;

/// The named weights of the generator, taken out one by one so a leftover or missing name is noticed.
pub struct Weights {
    map: BTreeMap<String, Tensor>,
    pub device: Device,
}

impl Weights {
    pub fn new(map: BTreeMap<String, Tensor>, device: Device) -> Self {
        Self { map, device }
    }

    /// Names not taken yet.
    pub fn leftover(&self) -> Vec<&str> {
        self.map.keys().map(String::as_str).collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    /// The tensor `name`, which must have exactly `shape`.
    pub fn take(&mut self, name: &str, shape: &[usize]) -> Result<Tensor> {
        let t = self.map.remove(name).ok_or_else(|| Error::Model(format!("the checkpoint has no weight `{name}`")))?;
        if t.dims() != shape {
            return Err(Error::Model(format!("weight `{name}` has shape {:?}, expected {shape:?}", t.dims())));
        }
        Ok(t)
    }

    /// The shape of `name`, without taking it.
    pub fn shape(&self, name: &str) -> Result<Vec<usize>> {
        self.map.get(name).map(|t| t.dims().to_vec()).ok_or_else(|| Error::Model(format!("the checkpoint has no weight `{name}`")))
    }

    /// BatchNorm `prefix` over `c` channels in inference mode: `y = x · scale + shift`.
    pub fn bn(&mut self, prefix: &str, c: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let mut get = |n: &str| -> Result<Vec<f32>> { Ok(self.take(&format!("{prefix}.{n}"), &[c])?.to_vec1::<f32>()?) };
        let (gamma, beta, mean, var) = (get("weight")?, get("bias")?, get("running_mean")?, get("running_var")?);
        let scale: Vec<f32> = gamma.iter().zip(&var).map(|(g, v)| g / (v + BN_EPS).sqrt()).collect();
        let shift: Vec<f32> = beta.iter().zip(&mean).zip(&scale).map(|((b, m), s)| b - m * s).collect();
        if scale.iter().chain(&shift).any(|v| !v.is_finite()) {
            return Err(Error::Model(format!("BatchNorm `{prefix}` has a value that is not a number")));
        }
        Ok((scale, shift))
    }
}

/// Mirror padding (without repeating the edge row), as `ReflectionPad2d` / `padding_mode="reflect"`, of a
/// `(1, c, h, w)` map.
pub fn reflect_pad(x: &Tensor, p: usize) -> Result<Tensor> {
    let (_, _, h, w) = x.dims4()?;
    if p >= h || p >= w {
        return Err(Error::Input(format!("a {w}×{h} map is too small to pad by {p}")));
    }
    let index = |n: usize| -> Vec<u32> {
        let mut v: Vec<u32> = (1..=p as u32).rev().collect();
        v.extend(0..n as u32);
        v.extend((0..p as u32).map(|i| (n as u32).saturating_sub(2 + i)));
        v
    };
    let dev = x.device();
    let rows = Tensor::from_vec(index(h), h + 2 * p, dev)?;
    let cols = Tensor::from_vec(index(w), w + 2 * p, dev)?;
    Ok(x.index_select(&rows, 2)?.index_select(&cols, 3)?)
}

/// One of the stem or downsampling layers: a convolution from the local branch (reflect padded),
/// BatchNorm and ReLU, giving the local branch and, for the last downsampling layer, the global one.
pub struct Down {
    l: Tensor,
    g: Option<Tensor>,
    bn_l: (Tensor, Tensor),
    bn_g: Option<(Tensor, Tensor)>,
    stride: usize,
    pad: usize,
}

impl Down {
    /// Layer `prefix` taking `inp` channels; `out_l` local and `out_g` global channels out; `k`, `stride`, `pad` of the convolutions.
    pub fn load(w: &mut Weights, prefix: &str, inp: usize, out: (usize, usize), k: usize, stride: usize, pad: usize) -> Result<Self> {
        let (out_l, out_g) = out;
        let pair = |w: &mut Weights, name: &str, c: usize| -> Result<(Tensor, Tensor)> {
            let (s, b) = w.bn(&format!("{prefix}.{name}"), c)?;
            Ok((Tensor::from_vec(s, (1, c, 1, 1), &w.device)?, Tensor::from_vec(b, (1, c, 1, 1), &w.device)?))
        };
        Ok(Self {
            l: w.take(&format!("{prefix}.ffc.convl2l.weight"), &[out_l, inp, k, k])?,
            g: if out_g > 0 { Some(w.take(&format!("{prefix}.ffc.convl2g.weight"), &[out_g, inp, k, k])?) } else { None },
            bn_l: pair(w, "bn_l", out_l)?,
            bn_g: if out_g > 0 { Some(pair(w, "bn_g", out_g)?) } else { None },
            stride,
            pad,
        })
    }

    /// `(local, global)` outputs of the input `x` `(1, c, h, w)`.
    pub fn forward(&self, x: &Tensor) -> Result<(Tensor, Option<Tensor>)> {
        let x = if self.pad > 0 { reflect_pad(x, self.pad)? } else { x.clone() };
        let run = |w: &Tensor, (scale, shift): &(Tensor, Tensor)| -> Result<Tensor> {
            Ok(x.conv2d(w, 0, self.stride, 1, 1)?.broadcast_mul(scale)?.broadcast_add(shift)?.relu()?)
        };
        let l = run(&self.l, &self.bn_l)?;
        let g = match (&self.g, &self.bn_g) {
            (Some(w), Some(bn)) => Some(run(w, bn)?),
            _ => None,
        };
        Ok((l, g))
    }
}
