//! A face too big to retouch at full size (a box past a feature's cap on the pixels it works on, such as a face
//! filling a very large export): the feature runs on the box reduced by a whole factor, as on a draft, and the
//! change it makes there is carried back to the full-size pixels as a gain per channel. So the export makes the
//! change the draft shows instead of skipping the face, and the work's memory stays within the cap.

use lightcraft_raster::{Rgb32f, par_rows};

use crate::feature::{Scene, View};
use crate::input::KeyMap;

/// The darkest value (scene-linear) a gain is taken against; a darker pixel keeps its value.
const DARK: f32 = 1e-4;
/// The largest gain carried back (a change past it is a feature's own clamp, never a retouch).
const MOST: f32 = 16.0;

/// The smallest whole factor that brings the box `b` (`[x0, y0, x1, y1)`) within `max` pixels; `None` when it
/// already is.
pub(crate) fn factor(b: [usize; 4], max: usize) -> Option<usize> {
    let (w, h) = (b[2].saturating_sub(b[0]), b[3].saturating_sub(b[1]));
    if w.saturating_mul(h) <= max {
        return None;
    }
    let max = max.max(1);
    let mut f = ((w as f64 * h as f64 / max as f64).sqrt().ceil() as usize).max(2);
    // (at most a few steps: the estimate is within one of the answer, and at w.max(h) the box is one pixel)
    while f < w.max(h) && w.div_ceil(f).saturating_mul(h.div_ceil(f)) > max {
        f += 1;
    }
    Some(f)
}

/// Retouch the box `b` of `img` for face `face` of `scene` at 1/`factor` of its size. `run` retouches the reduced
/// box: an image of its own, the faces of `scene` mapped into it, and the face's place in that scene. Its change
/// is carried back as a gain per channel, read bilinearly, to the pixels of `b` where `keep` (`x`, `y` in `img`)
/// is true; every other pixel keeps its bits.
pub(crate) fn retouch(
    img: &mut Rgb32f,
    scene: &Scene<'_>,
    face: usize,
    b: [usize; 4],
    factor: usize,
    keep: impl Fn(usize, usize) -> bool + Sync,
    run: impl FnOnce(&mut Rgb32f, &Scene<'_>, usize),
) {
    let [x0, y0] = [b[0], b[1]];
    let [x1, y1] = [b[2].min(img.width), b[3].min(img.height)];
    if x0 >= x1 || y0 >= y1 || img.data.len() != img.width.saturating_mul(img.height) {
        return;
    }
    let f = factor.max(1);
    let (sw, sh) = ((x1 - x0).div_ceil(f), (y1 - y0).div_ceil(f));
    let iw = img.width;
    // the mean of the pixels reduced pixel (`sx`, `sy`) covers
    let mean = |img: &Rgb32f, sx: usize, sy: usize| -> [f32; 3] {
        let (xs, ys) = (x0 + sx * f..(x0 + (sx + 1) * f).min(x1), y0 + sy * f..(y0 + (sy + 1) * f).min(y1));
        let mut acc = [0.0f32; 3];
        for y in ys.clone() {
            for p in img.data.get(y * iw + xs.start..y * iw + xs.end).unwrap_or(&[]) {
                acc = std::array::from_fn(|c| acc[c] + p[c]);
            }
        }
        let n = (xs.len() * ys.len()).max(1) as f32;
        acc.map(|v| v / n)
    };
    let mut small = Rgb32f::new(sw, sh);
    par_rows(&mut small.data, sw, |sy, row| {
        for (sx, o) in row.iter_mut().enumerate() {
            *o = mean(img, sx, sy);
        }
    });
    let Some(index) = scene.faces.get(face).map(|c| c.index) else { return };
    let view = BoxView { inner: scene.view(), origin: [x0 as f64, y0 as f64], factor: f as f64 };
    let sub = scene.through(&view, sw, sh);
    let Some(at) = sub.faces.iter().position(|c| c.index == index) else { return };
    run(&mut small, &sub, at);
    // the change, as a gain per channel (`img` still holds the pixels it was made from)
    par_rows(&mut small.data, sw, |sy, row| {
        for (sx, o) in row.iter_mut().enumerate() {
            let before = mean(img, sx, sy);
            *o = std::array::from_fn(|c| if before[c] > DARK && o[c].is_finite() { (o[c] / before[c]).clamp(0.0, MOST) } else { 1.0 });
        }
    });
    let gain = |sx: usize, sy: usize| small.data.get(sy * sw + sx).copied().unwrap_or([1.0; 3]);
    par_rows(&mut img.data, iw, |y, row| {
        if !(y0..y1).contains(&y) {
            return;
        }
        let fy = (((y - y0) as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (ty0, ty) = (fy as usize, fy - fy.floor());
        let ty1 = (ty0 + 1).min(sh - 1);
        for (x, p) in row.iter_mut().enumerate().take(x1).skip(x0) {
            let fx = (((x - x0) as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (tx0, tx) = (fx as usize, fx - fx.floor());
            let tx1 = (tx0 + 1).min(sw - 1);
            let g = [gain(tx0, ty0), gain(tx1, ty0), gain(tx0, ty1), gain(tx1, ty1)];
            // (where the reduced run changed nothing around, the pixel keeps its bits)
            if g.iter().all(|v| *v == [1.0; 3]) || !keep(x, y) {
                continue;
            }
            let k: [f32; 3] = std::array::from_fn(|c| (g[0][c] * (1.0 - tx) + g[1][c] * tx) * (1.0 - ty) + (g[2][c] * (1.0 - tx) + g[3][c] * tx) * ty);
            let new: [f32; 3] = std::array::from_fn(|c| p[c] * k[c]);
            if new.iter().all(|v| v.is_finite()) {
                *p = new;
            }
        }
    });
}

/// A box of a bigger image, reduced by `factor`: its pixel (`x`, `y`) covers the bigger image's pixels from
/// (`origin` + (`x`, `y`) × `factor`), `factor` on a side.
struct BoxView<'a> {
    inner: &'a dyn View,
    origin: [f64; 2],
    factor: f64,
}

impl View for BoxView<'_> {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        self.inner.to_norm(self.origin[0] + x * self.factor, self.origin[1] + y * self.factor)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        let (x, y) = self.inner.to_px(nx, ny)?;
        let p = ((x - self.origin[0]) / self.factor, (y - self.origin[1]) / self.factor);
        (p.0.is_finite() && p.1.is_finite()).then_some(p)
    }

    fn key_map(&self) -> Option<&dyn KeyMap> {
        self.inner.key_map()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factor_brings_the_box_within_the_cap() {
        assert_eq!(factor([0, 0, 100, 100], 10_000), None);
        for (b, max) in [([0, 0, 101, 100], 10_000), ([5, 7, 9000, 8000], 1 << 20), ([0, 0, 3, 1_000_000], 7), ([0, 0, 10, 10], 0)] {
            let Some(f) = factor(b, max) else { panic!("{b:?} is past {max}") };
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            assert!(w.div_ceil(f) * h.div_ceil(f) <= max.max(1) || f >= w.max(h), "{b:?}, {max}: {f}");
            assert!(f == 2 || (w.div_ceil(f - 1) * h.div_ceil(f - 1) > max.max(1)), "{b:?}, {max}: {f} is the smallest");
        }
    }
}
