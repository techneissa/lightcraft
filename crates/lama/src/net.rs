//! The big-lama generator (`FFCResNetGenerator`) and the inpainting wrapper around it.

use std::collections::BTreeMap;
use std::path::Path;

use candle_core::{Device, Tensor};

use crate::cl::{GeoCache, Head, ResnetBlock, Up, to_rows};
use crate::layers::{Down, Weights};
use crate::{Error, Result, checkpoint};

/// The picture sides must be multiples of this (three stride-2 stages).
pub const MODULO: usize = 8;
/// Smallest side the model is run on (smaller pictures are mirrored up to it).
const MIN_SIDE: usize = 32;
/// Largest picture side the model is run on (memory grows with the area).
pub const MAX_SIDE: usize = 2048;
/// Most residual blocks a checkpoint may list.
const MAX_BLOCKS: usize = 64;

/// A loaded LaMa generator, on one device.
pub struct Lama {
    device: Device,
    geos: GeoCache,
    stem: Down,
    downs: Vec<Down>,
    blocks: Vec<ResnetBlock>,
    ups: Vec<Up>,
    head: Head,
}

impl Lama {
    /// Loads `big-lama.pt` (or a Lightning checkpoint) from `path` onto `device`.
    pub fn load(path: &Path, device: &Device) -> Result<Self> {
        Self::from_tensors(checkpoint::read_generator(path, device)?, device)
    }

    /// Builds the generator from its named tensors (`<layer index>.<module>.<param>`).
    pub fn from_tensors(tensors: BTreeMap<String, Tensor>, device: &Device) -> Result<Self> {
        let mut w = Weights::new(tensors, device.clone());
        // layer 1 is a 7×7 convolution over (masked image, mask); its width sets the others
        let shape = w.shape("1.ffc.convl2l.weight")?;
        let &[ngf, 4, 7, 7] = shape.as_slice() else {
            return Err(Error::Model(format!("the first layer has shape {shape:?}, expected [ngf, 4, 7, 7]")));
        };
        if ngf == 0 || !ngf.is_multiple_of(8) || ngf > 256 {
            return Err(Error::Model(format!("unexpected base width {ngf}")));
        }
        // (reflection pad of 3 + a valid 7×7 convolution is a reflect-padded convolution)
        let stem = Down::load(&mut w, "1", 4, (ngf, 0), 7, 1, 3)?;
        // downsampling, layers 2..4: the last one makes the global branch (three quarters of the channels)
        let widths = [ngf, ngf * 2, ngf * 4, ngf * 8];
        let global = widths[3] * 3 / 4;
        let local = widths[3] - global;
        let mut downs = Vec::new();
        for i in 0..3 {
            let out = widths[i + 1];
            let out_g = if i == 2 { global } else { 0 };
            downs.push(Down::load(&mut w, &(i + 2).to_string(), widths[i], (out - out_g, out_g), 3, 2, 1)?);
        }
        // residual blocks from layer 5 on, for as long as they exist
        let mut blocks = Vec::new();
        let mut index = 5;
        while w.has(&format!("{index}.conv1.ffc.convl2l.weight")) {
            if blocks.len() >= MAX_BLOCKS {
                return Err(Error::Model("too many residual blocks".into()));
            }
            blocks.push(ResnetBlock::load(&mut w, &index.to_string(), local, global)?);
            index += 1;
        }
        if blocks.is_empty() {
            return Err(Error::Model("the checkpoint has no residual blocks".into()));
        }
        // layer `index` joins the branches; then three (ConvTranspose, BatchNorm, ReLU) triples
        let mut ups = Vec::new();
        let mut at = index + 1;
        for i in 0..3 {
            ups.push(Up::load(&mut w, at, at + 1, widths[3 - i], widths[2 - i])?);
            at += 3;
        }
        // the reflection pad is layer `at`; the output convolution is `at + 1`
        let head = Head::load(&mut w, &(at + 1).to_string(), ngf, 7)?;
        let left = w.leftover();
        if !left.is_empty() {
            log::warn!("LaMa: {} weights were not used, e.g. {}", left.len(), left.first().copied().unwrap_or_default());
        }
        Ok(Self { device: device.clone(), geos: GeoCache::new(device.clone()), stem, downs, blocks, ups, head })
    }

    /// The device the weights live on.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Runs the generator on `input` `(1, 4, h, w)` (the masked picture, then the mask) and returns the
    /// predicted picture as rows `(h·w, 3)` in `0..1`. `cancel` is asked between layers.
    pub fn generate(&self, input: &Tensor, cancel: &dyn Fn() -> bool) -> Result<Tensor> {
        let check = || if cancel() { Err(Error::Cancelled) } else { Ok(()) };
        let prof = std::env::var_os("LIGHTCRAFT_LAMA_PROFILE").is_some();
        let mut t0 = std::time::Instant::now();
        let mut lap = |what: &str| {
            if prof {
                let _ = self.device.synchronize();
                eprintln!("lama {what}: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
                t0 = std::time::Instant::now();
            }
        };
        let (mut x, _) = self.stem.forward(input)?;
        lap("stem");
        let mut xg = None;
        for d in &self.downs {
            check()?;
            let (l, g) = d.forward(&x)?;
            x = l;
            xg = g;
            lap("down");
        }
        let Some(g) = xg else { return Err(Error::Model("the downsampling made no global branch".into())) };
        let (_, _, h8, w8) = x.dims4()?;
        let (mut l, mut g) = (to_rows(&x)?, to_rows(&g)?);
        let geo = self.geos.get(h8, w8, 1)?;
        lap("layout");
        for b in &self.blocks {
            check()?;
            (l, g) = b.forward(&l, &g, &geo)?;
            lap("block");
        }
        let mut y = Tensor::cat(&[&l, &g], 1)?.contiguous()?;
        let (mut h, mut w) = (h8, w8);
        for u in &self.ups {
            check()?;
            y = u.forward(&y, h, w)?;
            (h, w) = (h * 2, w * 2);
            lap("up");
        }
        check()?;
        let geo = self.geos.get(h, w, 3)?;
        let out = candle_nn::ops::sigmoid(&self.head.forward(&y, &geo)?)?;
        lap("head");
        Ok(out)
    }

    /// Fills the hole of a picture: `rgb` is `w·h` pixels of three values in `0..1` (gamma encoded),
    /// `hole` is `w·h` values, 1 where the picture is to be filled and 0 where it is kept. The result is
    /// the picture with the hole replaced, same layout. Sides are padded to a multiple of [`MODULO`]
    /// by mirroring and cut back.
    pub fn inpaint(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, cancel: &dyn Fn() -> bool) -> Result<Vec<f32>> {
        check_input(rgb, hole, w, h)?;
        let (pw, ph) = (w.max(MIN_SIDE).div_ceil(MODULO) * MODULO, h.max(MIN_SIDE).div_ceil(MODULO) * MODULO);
        let input = Tensor::from_vec(planes(rgb, hole, (w, h), (pw, ph)), (1, 4, ph, pw), &self.device)?;
        // rows (h·w, 3)
        let out = self.generate(&input, cancel)?.to_device(&Device::Cpu)?.flatten_all()?.to_vec1::<f32>()?;
        Ok(put_back(rgb, hole, w, h, |x, y, c| out.get((y * pw + x) * 3 + c).copied().unwrap_or(0.0)))
    }
}

/// Whether `rgb` and `hole` are a picture of `w × h` pixels the model can fill: 1 to [`MAX_SIDE`] a side, the
/// buffers' lengths right, every value a number.
pub(crate) fn check_input(rgb: &[f32], hole: &[f32], w: usize, h: usize) -> Result<()> {
    let n = w.checked_mul(h).ok_or_else(|| Error::Input("picture too large".into()))?;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(Error::Input(format!("cannot fill a {w}×{h} picture (1 to {MAX_SIDE} pixels a side)")));
    }
    if rgb.len() != n * 3 || hole.len() != n {
        return Err(Error::Input(format!("buffers hold {} and {} values for {w}×{h} pixels", rgb.len(), hole.len())));
    }
    if rgb.iter().chain(hole).any(|v| !v.is_finite()) {
        return Err(Error::Input("the picture or hole has a value that is not a number".into()));
    }
    Ok(())
}

/// The model's input, channel-first `(4, ph, pw)`: the picture with the hole blanked, then the hole, mirrored at the
/// right and bottom edges from `w × h` up to `pw × ph` (`np.pad(.., "symmetric")`). Inputs checked by [`check_input`].
pub(crate) fn planes(rgb: &[f32], hole: &[f32], (w, h): (usize, usize), (pw, ph): (usize, usize)) -> Vec<f32> {
    let (px, py) = (mirror_index(pw, w), mirror_index(ph, h));
    let mut planes = vec![0f32; 4 * pw * ph];
    for (y, sy) in py.iter().enumerate() {
        for (x, sx) in px.iter().enumerate() {
            let src = sy * w + sx;
            let m = hole.get(src).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            for c in 0..3 {
                // the model sees the picture with the hole blanked
                planes[c * pw * ph + y * pw + x] = rgb.get(src * 3 + c).copied().unwrap_or(0.0).clamp(0.0, 1.0) * (1.0 - m);
            }
            planes[3 * pw * ph + y * pw + x] = m;
        }
    }
    planes
}

/// `rgb` (`w × h`) with the hole's pixels taken from the model's prediction `pred(x, y, channel)`, blended by the hole's
/// value.
pub(crate) fn put_back(rgb: &[f32], hole: &[f32], w: usize, h: usize, pred: impl Fn(usize, usize, usize) -> f32) -> Vec<f32> {
    let mut result = rgb.to_vec();
    for y in 0..h {
        for x in 0..w {
            let m = hole.get(y * w + x).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            if m > 0.0 {
                for c in 0..3 {
                    if let Some(r) = result.get_mut((y * w + x) * 3 + c) {
                        *r = m * pred(x, y, c) + (1.0 - m) * *r;
                    }
                }
            }
        }
    }
    result
}

/// For each of `padded` positions, the source index in `0..n`, mirrored at the end (`np.pad(.., "symmetric")`).
fn mirror_index(padded: usize, n: usize) -> Vec<usize> {
    (0..padded)
        .map(|i| {
            if i < n {
                return i;
            }
            let period = 2 * n;
            let j = i % period;
            if j < n { j } else { period - 1 - j }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_index_reflects_at_the_end_including_the_edge_pixel() {
        assert_eq!(mirror_index(8, 5), vec![0, 1, 2, 3, 4, 4, 3, 2]);
        assert_eq!(mirror_index(4, 4), vec![0, 1, 2, 3]);
        assert_eq!(mirror_index(8, 1), vec![0; 8]);
    }
}
