//! The per-pixel part of Background, all a slider drag redoes: each pixel's background is the kept blur at its
//! radius, cleaned and swapped by the sliders' amounts, and the person is drawn over it with their edge's colours.

use lightcraft_raster::{Rgb32f, par_rows};

use super::grid::Grid;
use super::matte::EDGE;
use super::prepare::{Fitted, Prepared, luma};
use super::{Background, Swap};
use crate::Scene;

/// The sliders as 0…1 (a value that is not a number is 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Strength {
    pub blur: f32,
    pub swap: f32,
    pub clean: f32,
}

impl Strength {
    pub fn of(b: &Background) -> Strength {
        let k = |v: f64| if v.is_finite() { (v / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 };
        Strength { blur: k(b.blur), swap: k(b.swap), clean: k(b.clean) }
    }

    pub fn is_off(&self) -> bool {
        self.blur <= 0.0 && self.swap <= 0.0 && self.clean <= 0.0
    }
}

/// Where Clean Backdrop fades out: a pixel that differs from the smoothed backdrop by this share of its brightness
/// is still a crease or a scuff (cleaned fully)…
const CLEAN_KEEP: (f32, f32) = (0.2, 0.45);
/// …and the dither on the cleaned backdrop, at most this share of its brightness (about an 8-bit step on a light
/// backdrop), never more than twice what was smoothed away (a flat backdrop stays flat).
const DITHER: f32 = 0.004;
/// The least share of background near a pixel a blur reads (else the next one, or the pixel's own).
const MIN_WEIGHT: f32 = 1e-3;

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// sRGB (0…1, as a colour picker shows it) → linear Rec.2020, the render's working space.
pub(crate) fn colour_linear(rgb: [f64; 3]) -> [f32; 3] {
    let lin = rgb.map(|v| {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    });
    const M: [[f64; 3]; 3] = [[0.627_404, 0.329_283, 0.043_313], [0.069_097, 0.919_540, 0.011_362], [0.016_391, 0.088_013, 0.895_595]];
    M.map(|r| (r[0] * lin[0] + r[1] * lin[1] + r[2] * lin[2]) as f32)
}

/// A triangular dither in −1…1 for render pixel (`gx`, `gy`) (the same in a window of the render).
fn dither(gx: i64, gy: i64) -> f32 {
    let mut h = (gx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (gy as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    h ^= h >> 31;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 29;
    let (a, b) = ((h & 0xffff) as f32 / 65535.0, ((h >> 16) & 0xffff) as f32 / 65535.0);
    a + b - 1.0
}

/// `levels` read at radius `rho` (pixels) at render pixel (`gx`, `gy`): between the two levels around it, on a log
/// scale; between `sharp` and the first below it. A level with no background near falls back to the next.
fn blurred(levels: &[(f64, Grid)], rho: f64, sharp: [f32; 3], gx: i64, gy: i64) -> [f32; 3] {
    let read = |k: usize| levels.get(k).and_then(|(_, g)| g.read(gx, gy, MIN_WEIGHT));
    let Some(r1) = levels.first().map(|(r, _)| *r) else { return sharp };
    if rho <= r1 || r1 <= 0.0 {
        let t = if r1 > 0.0 { (rho / r1).clamp(0.0, 1.0) as f32 } else { 1.0 };
        return lerp(sharp, read(0).unwrap_or(sharp), t);
    }
    let t = (rho / r1).log2().clamp(0.0, (levels.len() - 1) as f64);
    let k = (t.floor() as usize).min(levels.len().saturating_sub(2));
    let f = (t - k as f64).clamp(0.0, 1.0) as f32;
    match (read(k), read(k + 1)) {
        (Some(a), Some(b)) => lerp(a, b, f),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => sharp,
    }
}

/// Redraw `img` with the background of `k`, from the kept work `prep` (made from these very pixels).
pub(crate) fn draw(img: &mut Rgb32f, scene: &Scene<'_>, prep: &Prepared, k: &Strength, with: &Swap) {
    let view = scene.view();
    // (everything kept is made from the pixels before they change)
    let shares = (k.blur > 0.0).then(|| prep.shares(scene));
    let levels = (k.blur > 0.0).then(|| prep.levels(img));
    let clean = (k.clean > 0.0).then(|| prep.clean(img));
    let grain = (k.clean > 0.0).then(|| prep.grain(img));
    let colour = match with {
        Swap::Colour { rgb } => Some(colour_linear(*rgb)),
        _ => None,
    };
    let fitted: Option<std::sync::Arc<Fitted>> = if k.swap > 0.0 && colour.is_none() { view.backdrop().and_then(|b| prep.fitted(b)) } else { None };
    let picture_levels = fitted.as_deref().filter(|_| k.blur > 0.0).map(|f| prep.picture_levels(f));
    let swap = if colour.is_some() || fitted.is_some() { k.swap } else { 0.0 };
    if k.blur <= 0.0 && k.clean <= 0.0 && swap <= 0.0 {
        return;
    }
    let (w, place) = (img.width, prep.place);
    par_rows(&mut img.data, w, |y, row| {
        let edge = prep.edge.row(y);
        let mut e = 0usize;
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let a = prep.alpha.data.get(i).copied().unwrap_or(0.0);
            if a >= EDGE.1 {
                continue;
            }
            let here = *px;
            let (a, person, behind) = if a <= EDGE.0 {
                (0.0, here, here)
            } else {
                while edge.get(e).is_some_and(|p| p.i < i) {
                    e += 1;
                }
                match edge.get(e).filter(|p| p.i == i) {
                    Some(p) => (a, p.person, p.behind),
                    None => (a, here, here),
                }
            };
            let (gx, gy) = (place.x + x as i64, place.y + y as i64);
            let rho = match (shares, k.blur > 0.0) {
                (Some(s), true) => f64::from(k.blur) * prep.radius * f64::from(s.data.get(i).copied().unwrap_or(1.0)),
                _ => 0.0,
            };
            let mut bg = match levels {
                Some(l) if rho > 0.0 => blurred(l, rho, behind, gx, gy),
                _ => behind,
            };
            if let Some(c) = clean.and_then(|g| g.read(gx, gy, MIN_WEIGHT)) {
                let (lb, lc) = (luma(behind), luma(c).max(1e-4));
                let keep = smoothstep(((lb - lc).abs() / lc - CLEAN_KEEP.0) / (CLEAN_KEEP.1 - CLEAN_KEEP.0));
                let d = dither(gx, gy) * (DITHER * lc).min(2.0 * (lb - lc).abs());
                let g = grain.and_then(|p| p.data.get(i).copied()).unwrap_or(1.0);
                let target = c.map(|v| (v * g + d).max(0.0));
                bg = lerp(bg, target, k.clean * (1.0 - keep));
            }
            if swap > 0.0 {
                let s = match (colour, fitted.as_deref()) {
                    (Some(c), _) => c,
                    (None, Some(f)) => {
                        let sharp = f.at(&place, gx as f64 + 0.5, gy as f64 + 0.5);
                        match picture_levels {
                            Some(l) if rho > 0.0 => blurred(l, rho, sharp, gx, gy),
                            _ => sharp,
                        }
                    }
                    (None, None) => bg,
                };
                bg = lerp(bg, s, swap);
            }
            *px = std::array::from_fn(|c| a * person[c] + (1.0 - a) * bg[c]);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dither_is_centred_and_bounded() {
        let v: Vec<f32> = (0..64).flat_map(|y| (0..64).map(move |x| dither(x, y))).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        assert!(mean.abs() < 0.05 && v.iter().all(|d| d.abs() <= 1.0), "{mean}");
        assert_eq!(dither(-3, 9), dither(-3, 9));
    }

    #[test]
    fn white_stays_white_and_grey_stays_grey() {
        let w = colour_linear([1.0; 3]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-3), "{w:?}");
        let g = colour_linear([0.5, 0.5, 0.5]);
        assert!((g[0] - g[1]).abs() < 1e-4 && (g[1] - 0.214).abs() < 1e-3, "{g:?}");
        assert_eq!(colour_linear([f64::NAN, -1.0, 0.0]), [0.0; 3]);
    }
}
