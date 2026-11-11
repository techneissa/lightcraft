//! Wrinkles v2, what lies on the skin and is not skin: a hair across the forehead. A wrinkle's mask and its fill must not
//! cut a hair in two (half a strand would be left) nor copy one as pores, so these pixels are left out of the mask, out of
//! the skin the fill and the donors read, and stay as they are.
//!
//! A pixel is hair-like when it is a thin bright line (thinner than a lit ridge of skin, [`THIN`]) whose colour is greyer than the skin's own (a grey or white hair
//! against the red of skin). Chroma is measured against the face's median, so a black-and-white photo, where everything
//! is neutral, flags nothing. A skin-coloured thin bright line (the lit ridge beside a deep crease) and a grey patch with
//! no line in it (a pale ridge between creases) are skin. So is a short pale streak: on a deeply lined face the lit
//! ridges between the creases are thin, bright and paler than the skin, but short; a hair is one long strand, so only
//! stretches at least [`MIN_LEN`] long count.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::morph::max_filter;
use lightcraft_raster::{Plane, Rgb32f, par_rows};

use super::heal::smoothstep;

/// A thin bright line (a hair; width a fraction of the face) counts from this response (log light) up to [`LINE_FULL`].
const LINE_FROM: f32 = 0.015;
const LINE_FULL: f32 = 0.05;
const LINE_SIGMA: f64 = 0.0015;
/// A hair is thin: its line answers at [`LINE_SIGMA`] more than this share of its answer at [`WIDE`] times that width.
/// A lit ridge of skin, a few times wider, answers more at the wider width.
const THIN: f32 = 1.2;
const WIDE: f32 = 3.0;
/// ...and only where the colour is greyer than the skin's median by this much (log of red over green) from
/// [`GREY_FROM`] to [`GREY_FULL`]: the lit ridge beside a deep crease is a thin bright line too, but skin-coloured.
const GREY_FROM: f32 = 0.12;
const GREY_FULL: f32 = 0.25;
/// The shortest hair, a fraction of the face (a stretch of hair-like pixels of fewer than this length × 2 pixels is not
/// one).
const MIN_LEN: f64 = 0.06;

/// 0…1 per pixel: how hair-like each pixel of the crop is (grown by a pixel and softened, so a hair's edge goes too).
/// `lg` is the crop's log light per channel, `skin` its skin (0…1) and `size` the face's size in pixels.
pub(super) fn hair_like(lg: &Rgb32f, skin: &Plane, size: f64) -> Plane {
    let (w, h) = (lg.width, lg.height);
    if skin.data.len() != lg.data.len() || w == 0 || h == 0 {
        return Plane::new(w, h);
    }
    // the skin's median chroma
    let chroma = lg.map(|p| p[0] - p[1]);
    let mut c: Vec<f32> = chroma.data.iter().zip(&skin.data).filter(|(v, s)| **s > 0.9 && v.is_finite()).map(|(v, _)| *v).collect();
    let median = if c.len() >= 64 {
        let mid = c.len() / 2;
        *c.select_nth_unstable_by(mid, f32::total_cmp).1
    } else {
        f32::NEG_INFINITY
    };
    // thin bright lines: valleys of the negated brightness
    let lum = lg.map(|p| -(0.2126 * p[0].exp() + 0.7152 * p[1].exp() + 0.0722 * p[2].exp()).max(1e-4).ln());
    let sigma = ((LINE_SIGMA * size) as f32).max(1.5);
    let grey_at = |i: usize| {
        if median.is_finite() {
            let d = median - chroma.data.get(i).copied().unwrap_or(median);
            smoothstep(GREY_FROM, GREY_FULL, d)
        } else {
            0.0
        }
    };
    // (the valleys are measured only where a hair could be: a pixel that is not greyer than the skin, or whose thin
    // line is too faint, is no hair whatever the wider width says; issue #253)
    let line = gaussian(&lum, sigma);
    let mut raw = Plane::new(w, h);
    par_rows(&mut raw.data, w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let grey = grey_at(y * w + x);
            if grey > 0.0 || !super::skip_idle() {
                // (the line's response for now; the wider one decides below whether it is thin)
                *o = valley_at(&line, x, y, sigma);
            }
        }
    });
    let wide_wanted: Vec<bool> = raw.data.iter().map(|r| *r > LINE_FROM || !super::skip_idle()).collect();
    if wide_wanted.iter().any(|v| *v) {
        let wide = gaussian(&lum, WIDE * sigma);
        par_rows(&mut raw.data, w, |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let i = y * w + x;
                if !wide_wanted.get(i).copied().unwrap_or(false) {
                    *o = 0.0;
                    continue;
                }
                let r = *o;
                let narrow = r > THIN * valley_at(&wide, x, y, WIDE * sigma);
                let thin = if narrow { smoothstep(LINE_FROM, LINE_FULL, r) } else { 0.0 };
                *o = thin * grey_at(i);
            }
        });
    } else {
        raw.data.fill(0.0);
    }
    // one long strand, not a short pale streak
    let strands = super::ridge::hysteresis(&raw, 0.3, 0.6, |_| ((2.0 * MIN_LEN * size) as usize).max(8));
    gaussian(&max_filter(&raw.zip_map(&strands, |r, s| r * s), 1), 0.7)
}

/// How much of a valley (dark line) of width `sigma` there is at pixel (`x`, `y`) of `g` (the picture blurred by
/// `sigma`), in log-light depth: the larger Hessian eigenvalue times `sigma²`, damped where the second one is as large
/// (a dark spot).
fn valley_at(g: &Plane, x: usize, y: usize, sigma: f32) -> f32 {
    let at = |dx: isize, dy: isize| g.get_clamped(x as isize + dx, y as isize + dy);
    let c = at(0, 0);
    let (a, d) = (at(1, 0) - 2.0 * c + at(-1, 0), at(0, 1) - 2.0 * c + at(0, -1));
    let b = (at(1, 1) - at(-1, 1) - at(1, -1) + at(-1, -1)) / 4.0;
    let (mean, spread) = ((a + d) / 2.0, (((a - d) / 2.0).powi(2) + b * b).sqrt());
    let (l1, l2) = (mean + spread, mean - spread);
    if l1 <= 0.0 || !l1.is_finite() {
        return 0.0;
    }
    let ratio = l2 / l1;
    sigma * sigma * l1 * (-(ratio * ratio) / (2.0 * 0.5 * 0.5)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Orange skin with a pixel-wide white hair across it (row 50) and a grey patch.
    fn picture() -> (Rgb32f, Plane) {
        let lg = Rgb32f::from_fn(120, 100, |x, y| {
            let p = if y == 50 && x > 10 {
                [0.8f32, 0.78, 0.74]
            } else if (90..100).contains(&x) && (60..70).contains(&y) {
                [0.3, 0.3, 0.3]
            } else {
                [0.45, 0.3, 0.24]
            };
            p.map(f32::ln)
        });
        (lg, Plane::filled(120, 100, 1.0))
    }

    #[test]
    fn a_white_hair_is_hair_like_and_a_grey_patch_and_skin_are_not() {
        let (lg, skin) = picture();
        let hair = hair_like(&lg, &skin, 100.0);
        let at = |x: usize, y: usize| hair.data.get(y * 120 + x).copied().unwrap_or(f32::NAN);
        assert!(at(60, 50) > 0.8, "the hair");
        assert!(at(95, 65) < 0.01, "a grey patch with no thin line in it is not a hair");
        assert!(at(60, 20) < 0.01 && at(30, 80) < 0.01, "skin");
    }

    #[test]
    fn a_neutral_photo_flags_nothing_by_colour() {
        let lg = Rgb32f::from_fn(80, 80, |x, _| [0.3 + 0.001 * x as f32; 3].map(f32::ln));
        let hair = hair_like(&lg, &Plane::filled(80, 80, 1.0), 80.0);
        assert!(hair.data.iter().all(|v| *v < 0.01));
    }

    #[test]
    fn a_mismatched_skin_gives_nothing() {
        let lg = Rgb32f::new(10, 10);
        assert!(hair_like(&lg, &Plane::new(3, 3), 10.0).data.iter().all(|v| *v == 0.0));
    }
}
