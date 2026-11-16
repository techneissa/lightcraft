//! The head: Depth Anything 3's DPT (four stages projected and brought to 4×, 2×, 1× and ½× the patch grid,
//! fused coarse to fine by residual convolution units, then a shared neck and two outputs: depth through `exp`,
//! and sky through a ReLU).

use candle_core::{Module, Tensor};

use crate::params::{Conv, Params, resize};
use crate::vit::DIM;
use crate::{Error, Result};

const FEATURES: usize = 256;
const OUT_CHANNELS: [usize; 4] = [256, 512, 1024, 1024];
const HEAD_FEATURES: usize = 32;

/// `ReLU → conv → ReLU → conv`, added to its input.
struct ResUnit {
    conv1: Conv,
    conv2: Conv,
}

impl ResUnit {
    fn load(p: &Params) -> Result<Self> {
        Ok(Self { conv1: p.pp("conv1").conv(FEATURES, FEATURES, 3, (1, 1), true)?, conv2: p.pp("conv2").conv(FEATURES, FEATURES, 3, (1, 1), true)? })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = self.conv2.forward(&self.conv1.forward(&x.relu()?)?.relu()?)?;
        Ok((y + x)?)
    }
}

/// A fusion block: the lateral input's residual unit added (all but the coarsest), a second unit, a bilinear
/// resize (corners aligned) and a 1 × 1 convolution.
struct Fusion {
    unit1: Option<ResUnit>,
    unit2: ResUnit,
    out: Conv,
}

impl Fusion {
    fn load(p: &Params, residual: bool) -> Result<Self> {
        Ok(Self {
            unit1: if residual { Some(ResUnit::load(&p.pp("resConfUnit1"))?) } else { None },
            unit2: ResUnit::load(&p.pp("resConfUnit2"))?,
            out: p.pp("out_conv").conv(FEATURES, FEATURES, 1, (1, 0), true)?,
        })
    }

    fn forward(&self, x: &Tensor, lateral: Option<&Tensor>, size: (usize, usize)) -> Result<Tensor> {
        let y = match (&self.unit1, lateral) {
            (Some(u), Some(l)) => (x + u.forward(l)?)?,
            _ => x.clone(),
        };
        let y = self.unit2.forward(&y)?;
        Ok(self.out.forward(&resize(&y, size.0, size.1)?)?)
    }
}

/// `conv 3 × 3 → ReLU → conv 1 × 1` to one channel.
struct Out {
    a: Conv,
    b: Conv,
}

impl Out {
    fn load(p: &Params) -> Result<Self> {
        Ok(Self { a: p.pp("0").conv(FEATURES / 2, HEAD_FEATURES, 3, (1, 1), true)?, b: p.pp("2").conv(HEAD_FEATURES, 1, 1, (1, 0), true)? })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(self.b.forward(&self.a.forward(x)?.relu()?)?)
    }
}

pub struct Dpt {
    projects: Vec<Conv>,
    up4: Conv,
    up2: Conv,
    down2: Conv,
    rn: Vec<Conv>,
    fusion: Vec<Fusion>,
    neck: Conv,
    depth: Out,
    sky: Out,
}

impl Dpt {
    pub fn load(p: &Params) -> Result<Self> {
        let projects = OUT_CHANNELS
            .iter()
            .enumerate()
            .map(|(i, oc)| p.pp(&format!("projects.{i}")).conv(DIM, *oc, 1, (1, 0), true))
            .collect::<Result<Vec<_>>>()?;
        let s = p.pp("scratch");
        let rn = OUT_CHANNELS
            .iter()
            .enumerate()
            .map(|(i, oc)| s.pp(&format!("layer{}_rn", i + 1)).conv(*oc, FEATURES, 3, (1, 1), false))
            .collect::<Result<Vec<_>>>()?;
        let fusion = (1..=4).map(|i| Fusion::load(&s.pp(&format!("refinenet{i}")), i != 4)).collect::<Result<Vec<_>>>()?;
        Ok(Self {
            projects,
            up4: p.pp("resize_layers.0").conv_t(OUT_CHANNELS[0], 4)?,
            up2: p.pp("resize_layers.1").conv_t(OUT_CHANNELS[1], 2)?,
            down2: p.pp("resize_layers.3").conv(OUT_CHANNELS[3], OUT_CHANNELS[3], 3, (2, 1), true)?,
            rn,
            fusion,
            neck: s.pp("output_conv1").conv(FEATURES, FEATURES / 2, 3, (1, 1), true)?,
            depth: Out::load(&s.pp("output_conv2"))?,
            sky: Out::load(&s.pp("sky_output_conv2"))?,
        })
    }

    /// Depth (after `exp`) and sky (after a ReLU), `[1, 1, 14 gh, 14 gw]` each, from the backbone's four outputs.
    pub fn forward(&self, feats: &[Tensor], (gh, gw): (usize, usize)) -> Result<(Tensor, Tensor)> {
        let [f1, f2, f3, f4] = feats else { return Err(Error::Model("the head takes four feature maps".into())) };
        let grid = |f: &Tensor, i: usize| -> Result<Tensor> {
            let x = f.transpose(1, 2)?.contiguous()?.reshape((1, DIM, gh, gw))?;
            let proj = self.projects.get(i).ok_or_else(|| Error::Model("no projection".into()))?;
            Ok(proj.forward(&x)?)
        };
        let l1 = self.up4.forward(&grid(f1, 0)?)?;
        let l2 = self.up2.forward(&grid(f2, 1)?)?;
        let l3 = grid(f3, 2)?;
        let l4 = self.down2.forward(&grid(f4, 3)?)?;
        let [r1, r2, r3, r4] = &self.rn[..] else { return Err(Error::Model("no stage adapters".into())) };
        let (l1, l2, l3, l4) = (r1.forward(&l1)?, r2.forward(&l2)?, r3.forward(&l3)?, r4.forward(&l4)?);
        let hw = |t: &Tensor| -> Result<(usize, usize)> {
            let (_, _, h, w) = t.dims4()?;
            Ok((h, w))
        };
        let [n1, n2, n3, n4] = &self.fusion[..] else { return Err(Error::Model("no fusion blocks".into())) };
        let out = n4.forward(&l4, None, hw(&l3)?)?;
        let out = n3.forward(&out, Some(&l3), hw(&l2)?)?;
        let out = n2.forward(&out, Some(&l2), hw(&l1)?)?;
        let (h1, w1) = hw(&l1)?;
        let out = n1.forward(&out, Some(&l1), (h1 * 2, w1 * 2))?;
        let neck = resize(&self.neck.forward(&out)?, gh * 14, gw * 14)?;
        let depth = self.depth.forward(&neck)?.exp()?;
        let sky = self.sky.forward(&neck)?.relu()?;
        Ok((depth, sky))
    }
}
