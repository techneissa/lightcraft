//! Wrinkles v2, what lies on the skin and is not skin: a hair across the forehead. A wrinkle's mask and its fill must not
//! cut a hair in two (half a strand would be left) nor copy one as pores, so these pixels are left out of the mask, out of
//! the skin the fill and the donors read, and stay as they are.
//!
//! A pixel is hair-like when it is a thin bright line whose colour is greyer than the skin's own (a grey or white hair
//! against the red of skin). Chroma is measured against the face's median, so a black-and-white photo, where everything
//! is neutral, flags nothing. A skin-coloured thin bright line (the lit ridge beside a deep crease) and a grey patch with
//! no line in it (a pale ridge between creases) are skin.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::morph::max_filter;
use lightcraft_raster::{Plane, Rgb32f};

use super::heal::smoothstep;
use super::ridge::valley_response;

/// A thin bright line (a hair; width a fraction of the face) counts from this response (log light) up to [`LINE_FULL`].
const LINE_FROM: f32 = 0.015;
const LINE_FULL: f32 = 0.05;
const LINE_SIGMA: f64 = 0.003;
/// ...and only where the colour is greyer than the skin's median by this much (log of red over green) from
/// [`GREY_FROM`] to [`GREY_FULL`]: the lit ridge beside a deep crease is a thin bright line too, but skin-coloured.
const GREY_FROM: f32 = 0.12;
const GREY_FULL: f32 = 0.25;

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
    let line = valley_response(&lum, ((LINE_SIGMA * size) as f32).max(1.5));
    let raw = Plane::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let thin = smoothstep(LINE_FROM, LINE_FULL, line.data.get(i).copied().unwrap_or(0.0));
        let grey = if median.is_finite() {
            let d = median - chroma.data.get(i).copied().unwrap_or(median);
            smoothstep(GREY_FROM, GREY_FULL, d)
        } else {
            0.0
        };
        thin * grey
    });
    gaussian(&max_filter(&raw, 1), 0.7)
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
