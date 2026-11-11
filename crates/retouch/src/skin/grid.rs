//! The blurs Skin works with: mask-weighted Gaussian blurs on the skin reduced for their size ([`Reduced`]), and
//! the edge-aware smoothing of Smoothness ([`Grid`]).

use lightcraft_raster::blur::{gauss_taps, gaussian, gaussian_taps};
use lightcraft_raster::{Image, par_rows};

/// A bilateral grid (Chen, Paris and Durand, "Real-time edge-aware image processing with the bilateral grid",
/// 2007): the pore-free skin gathered into cells of `factor` pixels a side and bins of `bin` in log luminance,
/// blurred in space and in light, then read back at each pixel's own place and light. A pixel is averaged only
/// with skin of about its own light: a bump, a mark or a blotch (a little lighter or darker than the skin around
/// it) is evened, while across an edge (a nostril, a lash line, a fold, a shadow) the two sides stay apart, right
/// up to the edge. The grid's cells are a fixed fraction of the face, so its size, and its cost, are about the same
/// for a draft and a full-size export.
pub(super) struct Grid {
    /// Cells across, down, and bins of light (with [`PAD`] empty bins past each end).
    w: usize,
    h: usize,
    bins: usize,
    factor: usize,
    /// The light of bin 0, and a bin's width (natural log).
    lo: f32,
    bin: f32,
    /// `[log luminance, red, blue, 1]` weighted by the mask and summed, `[bin][y][x]`: a plane per bin.
    cells: Vec<[f32; 4]>,
}

/// Empty bins past each end of the light's range, so the blur in light has room to spread.
const PAD: usize = 2;
/// Most bins of light (more only on skin spanning an absurd range of light: its bins are then made wider).
const MOST_BINS: usize = 96;
/// Most cells of a grid (a mask's box far bigger than its face: its cells are then made bigger).
const MOST_CELLS: usize = 1 << 22;

impl Grid {
    /// The grid of `base` (the pore-free skin, `[log luminance, red, blue, mask]` per pixel, unweighted but for the
    /// mask) in cells for blurs of `sigma` pixels or more and bins `range` wide; `None` without skin.
    pub(super) fn new(base: &Image<[f32; 4]>, sigma: f32, range: f32) -> Option<Grid> {
        let (fw, fh) = (base.width, base.height);
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in base.data.iter().filter(|p| p[3] > 0.0 && p[0].is_finite()) {
            (lo, hi) = (lo.min(p[0]), hi.max(p[0]));
        }
        if !lo.is_finite() || !hi.is_finite() || lo > hi || !range.is_finite() || range <= 0.0 {
            return None;
        }
        let span = ((hi - lo) / range).ceil() as usize + 1;
        let bin = if span + 2 * PAD > MOST_BINS { (hi - lo) / (MOST_BINS - 2 * PAD - 1) as f32 } else { range };
        let bins = (((hi - lo) / bin.max(1e-6)).ceil() as usize + 1 + 2 * PAD).min(MOST_BINS);
        let mut factor = ((sigma / PER_PIXEL).floor() as usize).clamp(1, fw.max(fh).max(1));
        while fw.div_ceil(factor).saturating_mul(fh.div_ceil(factor)).saturating_mul(bins) > MOST_CELLS {
            factor += factor.div_ceil(4);
        }
        let (w, h) = (fw.div_ceil(factor), fh.div_ceil(factor));
        // gathered a row of cells at a time (`[y][bin][x]`, in parallel), then laid out a plane per bin
        let mut rows = vec![[0.0f32; 4]; w * h * bins];
        par_rows(&mut rows, (w * bins).max(1), |gy, row| {
            for y in gy * factor..((gy + 1) * factor).min(fh) {
                for (x, p) in base.data.get(y * fw..(y + 1) * fw).unwrap_or(&[]).iter().enumerate() {
                    let mv = p[3];
                    if mv <= 0.0 || !p[0].is_finite() {
                        continue;
                    }
                    let z = ((p[0] - lo) / bin).clamp(0.0, (bins - 2 * PAD - 1) as f32) + PAD as f32;
                    let (z0, t) = (z as usize, z.fract());
                    let v = [p[0] * mv, p[1] * mv, p[2] * mv, mv];
                    for (i, wt) in [(z0 * w + x / factor, 1.0 - t), ((z0 + 1) * w + x / factor, t)] {
                        if let Some(c) = row.get_mut(i) {
                            *c = std::array::from_fn(|k| c[k] + v[k] * wt);
                        }
                    }
                }
            }
        });
        let plane = w * h;
        let mut cells = vec![[0.0f32; 4]; plane * bins];
        par_rows(&mut cells, plane.max(1), |z, out| {
            for (gy, line) in out.chunks_mut(w.max(1)).enumerate() {
                if let Some(src) = rows.get((gy * bins + z) * w..(gy * bins + z + 1) * w) {
                    line.copy_from_slice(src);
                }
            }
        });
        Some(Grid { w, h, bins, factor, lo: lo - PAD as f32 * bin, bin, cells })
    }

    /// This grid blurred by `sigma` pixels in space and by `range` in light (nothing past its edges).
    pub(super) fn blurred(&self, sigma: f32, range: f32) -> Grid {
        let (w, h, plane) = (self.w, self.h, self.w * self.h);
        // in space: each bin's plane on its own
        let s = sigma / self.factor as f32;
        let mut spread = self.cells.clone();
        par_rows(&mut spread, plane.max(1), |_, cells| {
            if cells.iter().all(|c| c[3] == 0.0) {
                return;
            }
            let img = Image { width: w, height: h, data: cells.to_vec() };
            cells.copy_from_slice(&blur(&img, s).data);
        });
        // in light: each plane a weighted sum of its neighbours'
        let light = gauss_taps(range / self.bin);
        let r = light.len() / 2;
        let mut cells = vec![[0.0f32; 4]; spread.len()];
        par_rows(&mut cells, plane.max(1), |z, out| {
            for (k, wt) in light.iter().enumerate() {
                let Some(j) = (z + k).checked_sub(r) else { continue };
                let Some(src) = spread.get(j * plane..(j + 1) * plane) else { continue };
                for (o, v) in out.iter_mut().zip(src) {
                    *o = std::array::from_fn(|c| o[c] + v[c] * wt);
                }
            }
        });
        Grid { cells, ..*self }
    }

    /// This grid with `t` (0…1) of `other` (the same grid blurred another way) mixed in, cell by cell.
    pub(super) fn mix(self, other: &Grid, t: f32) -> Grid {
        let cells = self.cells.iter().zip(&other.cells).map(|(a, b)| std::array::from_fn(|c| a[c] + t * (b[c] - a[c]))).collect();
        Grid { cells, ..self }
    }

    /// The smoothed skin at pixel (`x`, `y`) whose own pore-free value is `own`: `[log luminance, red, blue]`;
    /// `own` where the grid has no skin of its light near it.
    pub(super) fn read(&self, x: usize, y: usize, own: [f32; 3]) -> [f32; 3] {
        let (w, h, bins) = (self.w, self.h, self.bins);
        if w == 0 || h == 0 || bins < 2 {
            return own;
        }
        let f = self.factor as f32;
        let fx = ((x as f32 + 0.5) / f - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = ((y as f32 + 0.5) / f - 0.5).clamp(0.0, (h - 1) as f32);
        let fz = ((own[0] - self.lo) / self.bin).clamp(0.0, (bins - 1) as f32);
        let (x0, y0, z0) = (fx as usize, fy as usize, (fz as usize).min(bins - 2));
        let (tx, ty, tz) = (fx - x0 as f32, fy - y0 as f32, fz - z0 as f32);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let plane = w * h;
        let mut acc = [0.0f32; 4];
        for (z, wz) in [(z0, 1.0 - tz), (z0 + 1, tz)] {
            for (yy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                for (xx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                    if let Some(v) = self.cells.get(z * plane + yy * w + xx) {
                        let wt = wz * wy * wx;
                        acc = std::array::from_fn(|c| acc[c] + v[c] * wt);
                    }
                }
            }
        }
        unweight(acc, own)
    }
}

/// A mask-weighted blur back to colour; `fallback` where the mask's blur is 0.
pub(super) fn unweight(p: [f32; 4], fallback: [f32; 3]) -> [f32; 3] {
    if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { fallback }
}

/// The mask-weighted region reduced by `factor` (whole pixels averaged), for blurs of at least about
/// [`PER_PIXEL`] times the factor; a blur of it is read back bilinearly.
#[derive(Clone)]
pub(super) struct Reduced {
    pub(super) img: Image<[f32; 4]>,
    pub(super) factor: usize,
}

/// The smallest blur, in reduced pixels, a reduction is made for (a blur of 2.5 px or more read back bilinearly
/// differs from the full-size blur by well under a percent of the band).
pub(super) const PER_PIXEL: f32 = 2.5;

impl Reduced {
    /// Reduce `weighted` for blurs of `smallest` full-size pixels or more.
    pub(super) fn new(weighted: &Image<[f32; 4]>, smallest: f32) -> Reduced {
        Reduced::through(weighted, smallest, |p| p)
    }

    /// Reduce `skin` (values and the mask, not yet weighted by it) for blurs of `smallest` full-size pixels or more.
    pub(super) fn of_skin(skin: &Image<[f32; 4]>, smallest: f32) -> Reduced {
        Reduced::through(skin, smallest, |p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]])
    }

    /// Reduce `src` through `f` (which gives each pixel's mask-weighted value) for blurs of `smallest` pixels or
    /// more.
    fn through(src: &Image<[f32; 4]>, smallest: f32, f: impl Fn([f32; 4]) -> [f32; 4] + Sync) -> Reduced {
        let (fw, fh) = (src.width, src.height);
        let factor = ((smallest / PER_PIXEL).floor() as usize).clamp(1, fw.max(fh).max(1));
        if factor == 1 {
            return Reduced { img: Image { width: fw, height: fh, data: src.data.iter().map(|p| f(*p)).collect() }, factor };
        }
        let (w, h) = (fw.div_ceil(factor), fh.div_ceil(factor));
        let mut img = Image::<[f32; 4]>::new(w, h);
        par_rows(&mut img.data, w.max(1), |sy, row| {
            let ys = sy * factor..((sy + 1) * factor).min(fh);
            for (sx, o) in row.iter_mut().enumerate() {
                let xs = sx * factor..((sx + 1) * factor).min(fw);
                let mut acc = [0.0f32; 4];
                for y in ys.clone() {
                    for p in src.data.get(y * fw + xs.start..y * fw + xs.end).unwrap_or(&[]) {
                        let v = f(*p);
                        acc = std::array::from_fn(|c| acc[c] + v[c]);
                    }
                }
                let n = (ys.len() * xs.len()).max(1) as f32;
                *o = acc.map(|v| v / n);
            }
        });
        Reduced { img, factor }
    }

    /// `weighted` blurred by `sigma` full-size pixels, reduced for it (not copied when it needs no reduction).
    pub(super) fn blurred(weighted: &Image<[f32; 4]>, sigma: f32) -> Reduced {
        if ((sigma / PER_PIXEL).floor() as usize) < 2 {
            return Reduced { img: blur(weighted, sigma), factor: 1 };
        }
        Reduced::new(weighted, sigma).blur(sigma)
    }

    /// Blurred by `sigma` full-size pixels.
    pub(super) fn blur(&self, sigma: f32) -> Reduced {
        Reduced { img: blur(&self.img, sigma / self.factor as f32), factor: self.factor }
    }

    /// Where full-size pixel (`x`, `y`) reads this reduction: four samples and their bilinear weights. Every blur
    /// of one reduction shares them.
    pub(super) fn taps(&self, x: usize, y: usize) -> Taps {
        let (w, h) = (self.img.width, self.img.height);
        if w == 0 || h == 0 {
            return Taps { at: [usize::MAX; 4], weight: [0.0; 4] };
        }
        if self.factor == 1 {
            let i = y * w + x;
            return Taps { at: [i; 4], weight: [1.0, 0.0, 0.0, 0.0] };
        }
        let f = self.factor as f32;
        let fx = ((x as f32 + 0.5) / f - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = ((y as f32 + 0.5) / f - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        Taps {
            at: [y0 * w + x0, y0 * w + x1, y1 * w + x0, y1 * w + x1],
            weight: [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty],
        }
    }

    /// The blurred mask at `taps` (0…1).
    pub(super) fn weight(&self, t: &Taps) -> f32 {
        t.at.iter().zip(t.weight).map(|(i, wt)| self.img.data.get(*i).map_or(0.0, |v| v[3] * wt)).sum()
    }

    /// This blur at `taps`, back to colour; `fallback` where the mask's blur is 0.
    pub(super) fn read(&self, t: &Taps, fallback: [f32; 3]) -> [f32; 3] {
        let mut p = [0.0f32; 4];
        for (i, wt) in t.at.iter().zip(t.weight) {
            if let Some(v) = self.img.data.get(*i) {
                p = std::array::from_fn(|c| p[c] + v[c] * wt);
            }
        }
        unweight(p, fallback)
    }
}

/// Blurs under this sigma (pixels) use exact Gaussian taps: the three-box blur snaps a small sigma to whole-pixel
/// boxes, and leaves the image as it is below about 0.58 px, so Texture did nothing on a face under about 72 px.
const EXACT_BELOW: f32 = 2.0;

/// `img` blurred by Gaussian `sigma` (pixels): exact taps under [`EXACT_BELOW`], three box passes above.
pub(super) fn blur(img: &Image<[f32; 4]>, sigma: f32) -> Image<[f32; 4]> {
    if sigma < EXACT_BELOW && sigma.is_finite() && sigma > 0.0 { gaussian_taps(img, &gauss_taps(sigma)) } else { gaussian(img, sigma) }
}

/// Four samples of a reduction and their weights ([`Reduced::taps`]).
pub(super) struct Taps {
    at: [usize; 4],
    weight: [f32; 4],
}
