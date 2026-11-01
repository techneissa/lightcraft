//! DIRS-MuGI and DIRS-PAIR transmission inference: a Rust port of the released networks in mingcv/DIRS
//! (`xreflection/archs/dirs_mugi_arch.py`, `dirs_pair_arch.py`, `dirs_lors_arch.py`; see the crate docs for
//! licences). The two share the Swin prior, the feature extractor and the dual-stream decoder; MuGI exchanges
//! the streams with a sigmoid gate in every block, PAIR with window attention (`dirs/pair.rs`) in the wide ones.
//! Only the path that gives the transmission layer: the Swin prior, the feature extractor and the
//! dual-stream decoder. The training-only LORS heads (`lnr`, `lzo`) and the reflection output are not run.
//! The weights are read as the author published them: a Lightning `.ckpt` and the Swin `.pth`,
//! both PyTorch pickles read by candle's non-executing pickle reader. No conversion step.
use crate::MAX_EDGE;
use crate::ops::{R, conv, conv1x1, depthwise3x3, device, norm2d, to_f32_vec};
use crate::swin::{Swin, SwinConfig};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use std::collections::HashMap;
use std::path::Path;

/// The two files of an installed DIRS-MuGI model, named as on Hugging Face (huqiming513/DIRS-Models).
pub const MUGI_CHECKPOINT: &str = "dirs_mugi_lors.ckpt";
pub const PAIR_CHECKPOINT: &str = "dirs_pair_lors.ckpt";
pub const SWIN_PRIOR: &str = "swin_large_o365.pth";
const MAX_CHECKPOINT_BYTES: u64 = 1 << 30;
const MAX_TENSORS: usize = 8192;
const MAX_TENSOR_ELEMENTS: usize = 1 << 28;
const CHECKPOINT_PREFIX: &str = "net_g.backbone.";

#[derive(Clone, Debug)]
pub(crate) struct DirsConfig {
    /// Width of the finest scale (48 in the release); the others are 2×, 4×, 8×, 16× of it.
    pub dim: usize,
    /// Wide blocks per decoder stage (`lib4` … `lib0`) and narrow ones after the pixel shuffle.
    pub enc: [usize; 5],
    pub dec: [usize; 5],
    pub prior_blocks: usize,
    pub swin: SwinConfig,
    /// PAIR: window attention in the wide blocks and a plain product as the gate; MuGI: sigmoid gates throughout.
    pub pair: bool,
}

impl DirsConfig {
    pub(crate) fn mugi() -> Self {
        Self { dim: 48, enc: [12, 8, 4, 2, 2], dec: [2, 2, 2, 2, 0], prior_blocks: 2, swin: SwinConfig::large(), pair: false }
    }

    pub(crate) fn pair() -> Self {
        Self { dim: 48, enc: [12, 8, 4, 2, 2], dec: [2, 2, 2, 2, 2], prior_blocks: 2, swin: SwinConfig::large(), pair: true }
    }
}

pub struct DirsNet {
    vb: VarBuilder<'static>,
    swin: Swin,
    device: Device,
    cfg: DirsConfig,
}

impl DirsNet {
    /// Reads `dirs_mugi_lors.ckpt` (or `dirs_pair_lors.ckpt` when `pair`) and `swin_large_o365.pth` from `dir`.
    pub fn load(dir: &Path, cpu: bool, pair: bool) -> Result<Self, String> {
        let device = device(cpu);
        let mut tensors = HashMap::new();
        let (checkpoint, cfg) = if pair { (PAIR_CHECKPOINT, DirsConfig::pair()) } else { (MUGI_CHECKPOINT, DirsConfig::mugi()) };
        read_pickle(&dir.join(checkpoint), "state_dict", &device, &mut tensors, |name| name.strip_prefix(CHECKPOINT_PREFIX).map(str::to_string))?;
        read_pickle(&dir.join(SWIN_PRIOR), "model", &device, &mut tensors, |name| {
            (!name.ends_with("relative_position_index")).then(|| format!("swin.{name}"))
        })?;
        let label = if pair { "DIRS-PAIR" } else { "DIRS-MuGI" };
        Self::from_tensors(tensors, cfg, device).map_err(|e| format!("unsupported {label} files: {e}"))
    }

    pub(crate) fn from_tensors(tensors: HashMap<String, Tensor>, cfg: DirsConfig, device: Device) -> R<Self> {
        Self::from_builder(VarBuilder::from_tensors(tensors, DType::F32, &device), cfg, device)
    }

    pub(crate) fn from_builder(vb: VarBuilder<'static>, cfg: DirsConfig, device: Device) -> R<Self> {
        let swin = Swin::new(vb.pp("swin"), cfg.swin.clone(), device.clone());
        let net = Self { vb, swin, device, cfg };
        net.check_weights()?;
        Ok(net)
    }

    /// Every tensor the forward pass reads is present with the right shape, before the first run.
    fn check_weights(&self) -> R<()> {
        let tiny = Tensor::zeros((1, 3, 32, 32), DType::F32, &self.device)?;
        self.forward(&tiny).map(|_| ())
    }

    pub fn infer(&self, rgb: &[u8], w: usize, h: usize) -> Result<Vec<[f32; 3]>, String> {
        if w == 0 || h == 0 || w > MAX_EDGE || h > MAX_EDGE || rgb.len() != w * h * 3 {
            return Err("invalid reflection input dimensions".into());
        }
        self.run(rgb, w, h).map_err(|e| e.to_string())
    }

    fn run(&self, rgb: &[u8], w: usize, h: usize) -> R<Vec<[f32; 3]>> {
        let data: Vec<f32> = rgb.iter().map(|x| *x as f32 / 255.0).collect();
        let mut input = Tensor::from_vec(data, (1, h, w, 3), &self.device)?.permute((0, 3, 1, 2))?.contiguous()?;
        if !w.is_multiple_of(32) {
            let n = 32 - w % 32;
            input = Tensor::cat(&[&input, &input.narrow(3, w - 1, 1)?.repeat((1, 1, 1, n))?], 3)?;
        }
        if !h.is_multiple_of(32) {
            let n = 32 - h % 32;
            input = Tensor::cat(&[&input, &input.narrow(2, h - 1, 1)?.repeat((1, 1, n, 1))?], 2)?;
        }
        let out = self.forward(&input)?.narrow(2, 0, h)?.narrow(3, 0, w)?.permute((0, 2, 3, 1))?;
        let flat = to_f32_vec(&out)?;
        let mut pixels = Vec::with_capacity(w * h);
        for p in flat.as_chunks::<3>().0 {
            if !p.iter().all(|x| x.is_finite()) {
                candle_core::bail!("non-finite model output");
            }
            pixels.push(p.map(|x| x.clamp(0.0, 1.0)));
        }
        Ok(pixels)
    }

    /// `input` is `[1, 3, h, w]` in [0, 1] with h and w multiples of 32; returns the transmission `[1, 3, h, w]`.
    pub(crate) fn forward(&self, input: &Tensor) -> R<Tensor> {
        let d = self.cfg.dim;
        let mut sp = self.swin.forward(input)?.into_iter();
        let (sp2, sp3, sp4, sp5) = match (sp.next(), sp.next(), sp.next(), sp.next()) {
            (Some(a), Some(b), Some(c), Some(e)) => (a, b, c, e),
            _ => candle_core::bail!("the Swin prior returned fewer than four scales"),
        };
        let mut cp = self.extractor(input)?.into_iter();
        let (cp0, cp1, cp2, cp3, cp4, cp5) = match (cp.next(), cp.next(), cp.next(), cp.next(), cp.next(), cp.next()) {
            (Some(a), Some(b), Some(c), Some(e), Some(f), Some(g)) => (a, b, c, e, f, g),
            _ => candle_core::bail!("the feature extractor returned fewer than six scales"),
        };
        // Scale 5 and 4 fuse the Swin prior with the extractor, per stream.
        let s5l = self.aib5(&sp5, &cp5.0)?;
        let s5r = self.aib5(&sp5, &cp5.1)?;
        drop((sp5, cp5));
        let s4l = self.wide(&sp4, &cp4.0, "aib4.0", 16 * d)?;
        let s4r = self.wide(&sp4, &cp4.1, "aib4.0", 16 * d)?;
        drop((sp4, cp4));
        let (f4l, f4r) = self.lib((&sum4(&s5l, &s4l)?, &sum4(&s5r, &s4r)?), "lib4", (16 * d, self.cfg.enc[0], self.cfg.dec[0]))?;
        drop((s5l, s5r, s4l, s4r));
        let s3l = self.wide(&sp3, &cp3.0, "aib3.0", 8 * d)?;
        let s3r = self.wide(&sp3, &cp3.1, "aib3.0", 8 * d)?;
        drop((sp3, cp3));
        let (f3l, f3r) =
            self.lib((&((&f4l + &s3l.0)? + &s3l.1)?, &((&f4r + &s3r.0)? + &s3r.1)?), "lib3", (8 * d, self.cfg.enc[1], self.cfg.dec[1]))?;
        drop((f4l, f4r, s3l, s3r));
        let s2l = self.wide(&sp2, &cp2.0, "aib2.0", 4 * d)?;
        let s2r = self.wide(&sp2, &cp2.1, "aib2.0", 4 * d)?;
        drop((sp2, cp2));
        let (f2l, f2r) =
            self.lib((&((&f3l + &s2l.0)? + &s2l.1)?, &((&f3r + &s2r.0)? + &s2r.1)?), "lib2", (4 * d, self.cfg.enc[2], self.cfg.dec[2]))?;
        drop((f3l, f3r, s2l, s2r));
        let (g1l, g1r) = ((&f2l + &cp1.0)?, (&f2r + &cp1.1)?);
        drop((f2l, f2r, cp1));
        let (f1l, f1r) = self.lib((&g1l, &g1r), "lib1", (2 * d, self.cfg.enc[3], self.cfg.dec[3]))?;
        let (mut xl, mut xr) = ((&f1l + &cp0.0)?, (&f1r + &cp0.1)?);
        drop((f1l, f1r, cp0));
        for i in 0..self.cfg.enc[4] {
            (xl, xr) = self.wide(&xl, &xr, &format!("lib0.{i}"), d)?;
        }
        for j in 0..self.cfg.dec[4] {
            (xl, xr) = self.mugi(&xl, &xr, &format!("lib0.{}", self.cfg.enc[4] + j), d)?;
        }
        conv(&self.vb.pp("out.seq.0"), &xl, (d, 3, 3), 1, 1)
    }

    fn aib5(&self, sp5: &Tensor, cp5: &Tensor) -> R<(Tensor, Tensor)> {
        let d = self.cfg.dim;
        let (a, b) = (candle_nn::ops::pixel_shuffle(sp5, 2)?, candle_nn::ops::pixel_shuffle(cp5, 2)?);
        let (a, b) = self.wide(&a, &b, "aib5.1", 8 * d)?;
        let up = self.vb.pp("aib5.2.seq.0");
        Ok((conv1x1(&up, &a, 8 * d, 16 * d)?, conv1x1(&up, &b, 8 * d, 16 * d)?))
    }

    /// One decoder stage: `enc` wide blocks, a pixel shuffle, `dec` narrow blocks and a 1×1 convolution.
    fn lib(&self, x: (&Tensor, &Tensor), name: &str, spec: (usize, usize, usize)) -> R<(Tensor, Tensor)> {
        let (wide, enc, dec) = spec;
        let (mut a, mut b) = (x.0.clone(), x.1.clone());
        for i in 0..enc {
            (a, b) = self.wide(&a, &b, &format!("{name}.{i}"), wide)?;
        }
        let narrow = wide / 4;
        a = candle_nn::ops::pixel_shuffle(&a, 2)?;
        b = candle_nn::ops::pixel_shuffle(&b, 2)?;
        for j in 0..dec {
            (a, b) = self.mugi(&a, &b, &format!("{name}.{}", enc + 1 + j), narrow)?;
        }
        let out = self.vb.pp(format!("{name}.{}.seq.0", enc + 1 + dec));
        Ok((conv1x1(&out, &a, narrow, narrow * 2)?, conv1x1(&out, &b, narrow, narrow * 2)?))
    }

    /// `LocalFeatureExtractor`: the stem and five stages of MuGI blocks, each ending in a 2×2 stride-2 convolution.
    fn extractor(&self, input: &Tensor) -> R<Vec<(Tensor, Tensor)>> {
        let mut c = self.cfg.dim;
        let stem = conv(&self.vb.pp("conv_prior.stem.seq.0"), input, (3, c, 3), 1, 1)?;
        let mut outs = vec![(stem.clone(), stem.clone())];
        let (mut x, mut y) = (stem.clone(), stem);
        for stage in 1..=5 {
            for i in 0..self.cfg.prior_blocks {
                (x, y) = self.mugi(&x, &y, &format!("conv_prior.block{stage}.{i}"), c)?;
            }
            let down = self.vb.pp(format!("conv_prior.block{stage}.{}.seq.0", self.cfg.prior_blocks));
            x = conv(&down, &x, (c, c * 2, 2), 2, 0)?;
            y = conv(&down, &y, (c, c * 2, 2), 2, 0)?;
            c *= 2;
            outs.push((x.clone(), y.clone()));
        }
        Ok(outs)
    }

    /// A wide block: MuGI's interaction block, or PAIR's window-attention block.
    fn wide(&self, a: &Tensor, b: &Tensor, p: &str, c: usize) -> R<(Tensor, Tensor)> {
        if self.cfg.pair { self.attention_block(a, b, p, c) } else { self.mugi(a, b, p, c) }
    }

    /// The first half of an interaction block: norm, expand, depthwise, gate, channel attention, project.
    fn block1(&self, vb: &VarBuilder<'static>, a: &Tensor, b: &Tensor, c: usize) -> R<(Tensor, Tensor)> {
        let first = |t: &Tensor| -> R<Tensor> {
            let t = norm2d(&vb.pp("0.seq.0"), t, c)?;
            depthwise3x3(&vb.pp("0.seq.2"), &conv1x1(&vb.pp("0.seq.1"), &t, c, c * 2)?, c * 2)
        };
        let (x, y) = gate(&first(a)?, &first(b)?, c, !self.cfg.pair)?;
        let attend = |t: &Tensor| -> R<Tensor> {
            let pooled = t.mean_keepdim((2, 3))?;
            let t = t.broadcast_mul(&conv1x1(&vb.pp("2.seq.0.ca.1"), &pooled, c, c)?)?;
            conv1x1(&vb.pp("3.seq.0"), &t, c, c)
        };
        Ok((attend(&x)?, attend(&y)?))
    }

    /// `MuGIInteractionBlock` (or PAIR's `DIRSLocalInteractionBlock`, which only differs in the gate): the exchange
    /// between the streams and a shared `b`.
    fn mugi(&self, a: &Tensor, b: &Tensor, p: &str, c: usize) -> R<(Tensor, Tensor)> {
        let vb = self.vb.pp(p);
        let (x, y) = self.block1(&vb.pp("block1"), a, b, c)?;
        let a = (a + x.broadcast_mul(&vb.get((1, c, 1, 1), "a_l")?)?)?;
        let b = (b + y.broadcast_mul(&vb.get((1, c, 1, 1), "a_r")?)?)?;
        let b2 = vb.pp("block2");
        let second = |t: &Tensor| -> R<Tensor> { conv1x1(&b2.pp("0.seq.1"), &norm2d(&b2.pp("0.seq.0"), t, c)?, c, c * 2) };
        let (x, y) = gate(&second(&a)?, &second(&b)?, c, !self.cfg.pair)?;
        let x = conv1x1(&b2.pp("2.seq.0"), &x, c, c)?;
        let y = conv1x1(&b2.pp("2.seq.0"), &y, c, c)?;
        let gain = vb.get((1, c, 1, 1), "b")?;
        Ok(((a + x.broadcast_mul(&gain)?)?, (b + y.broadcast_mul(&gain)?)?))
    }
}

fn sum4(a: &(Tensor, Tensor), b: &(Tensor, Tensor)) -> R<Tensor> {
    ((&a.0 + &a.1)? + (&b.0 + &b.1)?)?.contiguous()
}

/// `DualStreamGLU` (MuGI, `sigmoid`) or `DualStreamGate` (PAIR): each stream's first half times the other
/// stream's second half, passed through a sigmoid for MuGI.
fn gate(x: &Tensor, y: &Tensor, c: usize, sigmoid: bool) -> R<(Tensor, Tensor)> {
    let other = |t: &Tensor| -> R<Tensor> {
        let half = t.narrow(1, c, c)?;
        if sigmoid { candle_nn::ops::sigmoid(&half) } else { Ok(half) }
    };
    Ok((x.narrow(1, 0, c)?.mul(&other(y)?)?, y.narrow(1, 0, c)?.mul(&other(x)?)?))
}

/// Reads the tensors `keep` selects from a PyTorch zip pickle under top-level `key`, as f32 on `device`.
/// candle's reader builds tensors from the file's data and never runs code from it; a damaged or
/// hostile file is an error, never a panic.
fn read_pickle(
    path: &Path,
    key: &str,
    device: &Device,
    out: &mut HashMap<String, Tensor>,
    keep: impl Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let size = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?.len();
    if !(64..=MAX_CHECKPOINT_BYTES).contains(&size) {
        return Err(format!("{name}: invalid file size"));
    }
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
        let pth = candle_core::pickle::PthTensors::new(path, Some(key)).map_err(|e| format!("{name}: {e}"))?;
        if pth.tensor_infos().len() > MAX_TENSORS {
            return Err(format!("{name}: too many tensors"));
        }
        for (original, info) in pth.tensor_infos() {
            let Some(target) = keep(original) else { continue };
            if info.layout.shape().elem_count() > MAX_TENSOR_ELEMENTS {
                return Err(format!("{name}: tensor {original} is too large"));
            }
            let tensor = pth
                .get(original)
                .map_err(|e| format!("{name}: {original}: {e}"))?
                .ok_or_else(|| format!("{name}: {original} missing"))?
                .to_dtype(DType::F32)
                .and_then(|t| t.to_device(device))
                .map_err(|e| format!("{name}: {original}: {e}"))?;
            out.insert(target, tensor);
        }
        Ok(())
    }));
    match read {
        Ok(result) => result,
        Err(_) => Err(format!("{name}: not a readable PyTorch checkpoint")),
    }
}

mod pair;

#[cfg(test)]
mod tests;
