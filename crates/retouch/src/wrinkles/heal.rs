//! Wrinkles v2, the replacement: what goes where the wrinkle mask is. Everything here works on a face crop at the
//! search's resolution, in the natural log of linear light per channel, and gives the change as a log difference.
//!
//! Three bands, as the step-1 experiment found them best (`fork/retouch-wrinkles.md`):
//! - **low**: the skin's light and colour without the pores. Rebuilt from the skin around the mask by a normalised
//!   convolution at growing widths, the finest one with enough skin winning ([`rebuild_low`], the banded heal that runs
//!   when LaMa isn't there), or taken from LaMa's fill, blurred to [`LAMA_LOW`] ([`low_of_fill`]). LaMa's own texture is
//!   never used.
//! - **mid**: the crease and its shaded flanks. Not put back at all: this is what a wrinkle is.
//! - **fine**: the pores, under [`FINE`] of the face (at least [`FINE_MIN_PX`] pixels). Put back from nearby skin: the donor is the first of [`SHIFTS`]
//!   whose source is clean skin (no mask, and skin all around it, so no hair streak or crease edge is copied).

use lightcraft_raster::Plane;
use lightcraft_raster::Rgb32f;
use lightcraft_raster::blur::gaussian;

/// The fine band's blur (sigma), a fraction of the face: what is finer than this is pores and is put back. On a small
/// face it is held to [`FINE_MIN_PX`] pixels, or the pores (a pixel or two across) would count as the low band.
pub(super) const FINE: f64 = 0.002;
const FINE_MIN_PX: f64 = 2.0;
/// LaMa's fill is blurred to twice the fine blur before it is used as the low band (its own texture is never used).
const LAMA_LOW: f64 = 2.0;

/// The fine band's blur in pixels for a face `size` pixels across.
pub(super) fn fine_sigma(size: f64) -> f32 {
    (FINE * size).max(FINE_MIN_PX) as f32
}
/// The widths (sigma, fractions of the face) the banded heal reads the skin around the mask at, widest first.
const WIDTHS: [f64; 6] = [0.20, 0.10, 0.05, 0.025, 0.0125, 0.006];
/// A donor's cleanliness (skin all around it, outside the mask) must reach this, and its own mask stay under
/// [`DONOR_MASK`].
pub(super) const DONOR_CLEAN: f32 = 0.8;
const DONOR_MASK: f32 = 0.15;

/// Where, as fractions of the face, the fine band is looked for when it can't be taken from the pixel itself, in
/// order (across, down).
pub(super) const SHIFTS: [(f64, f64); 14] = [
    (0.07, 0.0),
    (-0.07, 0.0),
    (0.0, 0.07),
    (0.0, -0.07),
    (0.05, 0.05),
    (-0.05, -0.05),
    (0.05, -0.05),
    (-0.05, 0.05),
    (0.14, 0.0),
    (-0.14, 0.0),
    (0.0, 0.14),
    (0.0, -0.14),
    (0.21, 0.0),
    (-0.21, 0.0),
];

pub(super) fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The natural log of linear light per channel (floored, so black stays finite).
pub(super) fn log_image(img: &Rgb32f) -> Rgb32f {
    img.map(|p| p.map(|v| if v.is_finite() { v.max(1e-4).ln() } else { 1e-4f32.ln() }))
}

/// The skin the low band may be read from: skin, and not in the mask.
fn support(mask: &Plane, skin: &Plane) -> Plane {
    mask.zip_map(skin, |m, s| (1.0 - m).powi(2) * smoothstep(0.5, 0.9, s))
}

/// How clean the skin around each pixel is (0…1): the share of the neighbourhood (sigma `sigma` pixels) that is skin
/// outside the mask. A hair streak, a crease or the skin's edge nearby lowers it.
pub(super) fn cleanliness(mask: &Plane, skin: &Plane, sigma: f32) -> Plane {
    gaussian(&support(mask, skin), sigma)
}

/// The low band inside the mask, rebuilt from the skin around it: each channel's log light, blurred a little (so no
/// pores), then filled by normalised convolution at growing widths, the finest one with enough skin there winning.
pub(super) fn rebuild_low(lg: &Rgb32f, mask: &Plane, skin: &Plane, size: f64) -> Rgb32f {
    let smooth = gaussian(lg, fine_sigma(size));
    let support = support(mask, skin);
    let weighted = Rgb32f::from_fn(lg.width, lg.height, |x, y| smooth.get(x, y).map(|v| v * support.get(x, y)));
    let mut out: Option<Rgb32f> = None;
    for width in WIDTHS {
        let sigma = ((width * size) as f32).max(2.0 * fine_sigma(size));
        let (num, den) = (gaussian(&weighted, sigma), gaussian(&support, sigma));
        let level = Rgb32f::from_fn(lg.width, lg.height, |x, y| {
            let d = den.get(x, y).max(1e-6);
            num.get(x, y).map(|v| v / d)
        });
        out = Some(match out {
            None => level,
            Some(coarse) => Rgb32f::from_fn(lg.width, lg.height, |x, y| {
                let c = smoothstep(0.02, 0.35, den.get(x, y));
                let (a, b) = (level.get(x, y), coarse.get(x, y));
                std::array::from_fn(|k| a[k] * c + b[k] * (1.0 - c))
            }),
        });
    }
    out.unwrap_or(smooth)
}

/// The low band of a LaMa fill (linear light, the crop's size): its log, blurred to [`LAMA_LOW`].
pub(super) fn low_of_fill(fill: &Rgb32f, size: f64) -> Rgb32f {
    gaussian(&log_image(fill), LAMA_LOW as f32 * fine_sigma(size))
}

/// A donor's fine band is held to this many times the clean skin's own typical fine band (its median, as a Gaussian's
/// sigma): a stray hair or a speck of dust on the skin is an outlier and is not copied whole.
const FINE_CLAMP: f32 = 3.0;

/// The skin's typical fine band: the median of the absolute fine band over the clean skin outside the mask, as a
/// Gaussian's sigma; `None` without such skin.
fn typical_fine(fine: &Rgb32f, mask: &Plane, clean: &Plane) -> Option<f32> {
    let mut v: Vec<f32> = fine
        .data
        .iter()
        .zip(mask.data.iter().zip(&clean.data))
        .filter(|(_, (m, c))| **m < DONOR_MASK && **c > DONOR_CLEAN)
        .map(|(f, _)| (f[0].abs() + f[1].abs() + f[2].abs()) / 3.0)
        .filter(|a| a.is_finite())
        .collect();
    if v.len() < 64 {
        return None;
    }
    let mid = v.len() / 2;
    let (_, median, _) = v.select_nth_unstable_by(mid, f32::total_cmp);
    Some(*median * 1.4826)
}

/// The fine band (log light less its slight blur) at each pixel, taken from the first of [`SHIFTS`] whose source is a
/// clean donor (`clean` ≥ [`DONOR_CLEAN`], outside the mask), held to [`FINE_CLAMP`] times the skin's typical fine
/// band; zero (flat) where none is.
pub(super) fn fine_from_neighbours(lg: &Rgb32f, mask: &Plane, clean: &Plane, size: f64) -> Rgb32f {
    let smooth = gaussian(lg, fine_sigma(size));
    let mut fine = lg.zip_map(&smooth, |a, b| std::array::from_fn(|k| a[k] - b[k]));
    if let Some(sigma) = typical_fine(&fine, mask, clean) {
        let limit = FINE_CLAMP * sigma;
        fine.map_in_place(|p| p.map(|v| v.clamp(-limit, limit)));
    }
    let (w, h) = (lg.width as isize, lg.height as isize);
    // (whole pixels: a fractional shift would blend neighbours and soften the borrowed pores)
    let shifts: Vec<(isize, isize)> = SHIFTS.iter().map(|(sx, sy)| ((sx * size).round() as isize, (sy * size).round() as isize)).collect();
    Rgb32f::from_fn(lg.width, lg.height, |x, y| {
        if mask.get(x, y) <= 0.0 {
            return [0.0; 3];
        }
        for (sx, sy) in &shifts {
            let (nx, ny) = (x as isize + sx, y as isize + sy);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if mask.get(nx, ny) < DONOR_MASK && clean.get(nx, ny) > DONOR_CLEAN {
                return fine.get(nx, ny);
            }
        }
        [0.0; 3]
    })
}

/// The change at each pixel, in log light per channel: where the mask `m` is 1 the new skin (`low` + `fine`) replaces
/// the old one (`lg`), fading in with the mask. Not a number becomes 0, and the change is held to a plausible range.
pub(super) fn delta(lg: &Rgb32f, m: &Plane, low: &Rgb32f, fine: &Rgb32f) -> Rgb32f {
    Rgb32f::from_fn(lg.width, lg.height, |x, y| {
        let k = m.get(x, y);
        if k <= 0.0 {
            return [0.0; 3];
        }
        let (l, lo, f) = (lg.get(x, y), low.get(x, y), fine.get(x, y));
        std::array::from_fn(|c| {
            let d = k * (lo[c] + f[c] - l[c]);
            if d.is_finite() { d.clamp(MOST_DARKER, MOST_LIGHTER) } else { 0.0 }
        })
    })
}

/// The most a pixel is darkened or lightened (log light): a crease is lifted, not deepened.
pub(super) const MOST_DARKER: f32 = -0.4;
pub(super) const MOST_LIGHTER: f32 = 1.5;
