//! The grids the under-eye effects are worked out on, in an eye's own frame, and the blurs and samples they use.

use lightcraft_raster::blur::{gauss_taps, gaussian, gaussian_taps};
use lightcraft_raster::{Image, Rgb32f};

use super::geom::Eye;

/// Sigmas under this many pixels (or cells) use an exact kernel (the three-box blur can't make them).
const EXACT_BELOW: f32 = 2.5;

/// Luminance of linear Rec.2020 (the app's working space).
pub fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]
}

/// Blur `img` with a Gaussian of `sigma` pixels.
pub fn blur<T: lightcraft_raster::resample::Pixel>(img: &Image<T>, sigma: f64) -> Image<T> {
    let s = sigma as f32;
    if !(s.is_finite() && s > 0.0) {
        return img.clone();
    }
    if s < EXACT_BELOW { gaussian_taps(img, &gauss_taps(s)) } else { gaussian(img, s) }
}

/// Blur a grid's values (`w × h`, `N` channels) with a Gaussian of `sigma` cells: an exact kernel, on this
/// thread (a grid is a few thousand cells; handing it to the thread pool costs more than the blur).
pub fn grid_blur<const N: usize>(v: &[[f32; N]], w: usize, h: usize, sigma: f64) -> Vec<[f32; N]> {
    let s = sigma as f32;
    if !(s.is_finite() && s > 0.1) || v.len() != w.saturating_mul(h) || w == 0 || h == 0 {
        return v.to_vec();
    }
    if s >= EXACT_BELOW {
        // three box passes a side (running sums: the cost does not grow with the sigma)
        let mut out = v.to_vec();
        for r in lightcraft_raster::blur::box_radii(s) {
            out = box_pass(&out, w, h, r, true);
            out = box_pass(&out, w, h, r, false);
        }
        return out;
    }
    let taps = gauss_taps(s);
    let r = (taps.len() / 2) as isize;
    let pass = |src: &[[f32; N]], horizontal: bool| -> Vec<[f32; N]> {
        let mut out = vec![[0.0f32; N]; src.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; N];
                for (k, t) in taps.iter().enumerate() {
                    let d = k as isize - r;
                    let (sx, sy) = if horizontal {
                        ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                    } else {
                        (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                    };
                    if let Some(p) = src.get(sy * w + sx) {
                        for c in 0..N {
                            acc[c] += p[c] * t;
                        }
                    }
                }
                if let Some(o) = out.get_mut(y * w + x) {
                    *o = acc;
                }
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

/// A box of radius `r` along rows (`horizontal`) or columns of a `w × h` grid, edges held.
fn box_pass<const N: usize>(v: &[[f32; N]], w: usize, h: usize, r: usize, horizontal: bool) -> Vec<[f32; N]> {
    if r == 0 {
        return v.to_vec();
    }
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, k: usize| -> [f32; N] {
        let i = if horizontal { line * w + k } else { k * w + line };
        v.get(i).copied().unwrap_or([0.0; N])
    };
    let mut out = vec![[0.0f32; N]; v.len()];
    let n = (2 * r + 1) as f32;
    for line in 0..lines {
        let held = |k: isize| at(line, k.clamp(0, len as isize - 1) as usize);
        let mut sum = [0.0f32; N];
        for k in -(r as isize)..=(r as isize) {
            let p = held(k);
            sum = std::array::from_fn(|c| sum[c] + p[c]);
        }
        for k in 0..len {
            let i = if horizontal { line * w + k } else { k * w + line };
            if let Some(o) = out.get_mut(i) {
                *o = sum.map(|c| c / n);
            }
            let (add, sub) = (held(k as isize + r as isize + 1), held(k as isize - r as isize));
            sum = std::array::from_fn(|c| sum[c] + add[c] - sub[c]);
        }
    }
    out
}

/// The image at (`x`, `y`), bilinear between pixel centres; `None` outside it.
pub fn sample(img: &Rgb32f, x: f64, y: f64) -> Option<[f32; 3]> {
    let (w, h) = (img.width, img.height);
    if !(x >= 0.0 && y >= 0.0 && x <= w as f64 && y <= h as f64) || w == 0 || h == 0 {
        return None;
    }
    let (fx, fy) = ((x - 0.5).clamp(0.0, (w - 1) as f64), (y - 0.5).clamp(0.0, (h - 1) as f64));
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let at = |x: usize, y: usize| img.data.get(y * w + x).copied();
    let (a, b, c, d) = (at(x0, y0)?, at(x1, y0)?, at(x0, y1)?, at(x1, y1)?);
    Some(std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    }))
}

/// A grid in the eye's frame: cell (`i`, `j`) is centred on eye-local (`a0 + i·step`, `b0 + j·step`).
pub struct Grid {
    a0: f64,
    b0: f64,
    pub step: f64,
    pub w: usize,
    pub h: usize,
}

impl Grid {
    /// The grid of `per_half` cells per half eye width over eye-local box `[a0, a1, t0, t1]` (`t` below the lid: the
    /// lid's drop, at most `lid_max`, is added below), grown by `pad` half widths: the same cells, in the eye's frame,
    /// at every size and zoom; at most `max_side` a side.
    pub fn new(eye: &Eye, area: [f64; 4], pad: f64, per_half: f64, lid_max: f64, max_side: usize) -> Option<Grid> {
        let step = 1.0 / per_half;
        let [a0, a1, t0, t1] = area;
        let (a0, a1, b0, b1) = (a0 - pad, a1 + pad, t0 - pad, t1 + eye.lid_drop().min(lid_max) + pad);
        let side = |span: f64| {
            let n = (span / step).ceil();
            (n.is_finite() && n >= 0.0 && n < max_side as f64).then(|| (n as usize).checked_add(1)).flatten()
        };
        Some(Grid { a0, b0, step, w: side(a1 - a0)?, h: side(b1 - b0)? })
    }

    pub fn local(&self, i: usize, j: usize) -> (f64, f64) {
        (self.a0 + i as f64 * self.step, self.b0 + j as f64 * self.step)
    }

    /// `v` sampled at eye-local (`a`, `b`), bilinear between cell centres (held at the edge).
    pub fn at<T: Copy + Interp>(&self, v: &[T], a: f64, b: f64, none: T) -> T {
        let (fx, fy) = (((a - self.a0) / self.step).clamp(0.0, (self.w - 1) as f64), ((b - self.b0) / self.step).clamp(0.0, (self.h - 1) as f64));
        if !(fx.is_finite() && fy.is_finite()) {
            return none;
        }
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let g = |x: usize, y: usize| v.get(y * self.w + x).copied().unwrap_or(none);
        T::lerp(T::lerp(g(x0, y0), g(x1, y0), tx), T::lerp(g(x0, y1), g(x1, y1), tx), ty)
    }
}

pub trait Interp {
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl Interp for [f32; 3] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
    }
}

impl Interp for [f32; 1] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        [a[0] + (b[0] - a[0]) * t]
    }
}

/// The cells within `r` of cell `k` (itself included).
pub fn around(grid: &Grid, k: usize, r: isize) -> impl Iterator<Item = usize> + '_ {
    let (i, j) = ((k % grid.w.max(1)) as isize, (k / grid.w.max(1)) as isize);
    (-r..=r)
        .flat_map(move |dj| (-r..=r).map(move |di| (i + di, j + dj)))
        .filter(|&(x, y)| x >= 0 && y >= 0 && (x as usize) < grid.w && (y as usize) < grid.h)
        .map(|(x, y)| y as usize * grid.w + x as usize)
}
