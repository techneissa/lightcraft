//! What goes where a strand was: the background behind it, on the search's picture (the head crop at most
//! [`super::MAX_WORK`] pixels across): a weight per pixel (how much the strand's pixel is replaced) and the replacement
//! colour, in the natural log of linear light per channel. (A value, not a ratio to the old pixel: a render that has
//! other pixels than the search's, a draft or an export, still puts the background there.)
//!
//! Two bands, as Wrinkles v2's replacement:
//! - **low**: the light and colour of the background without its grain: LaMa's fill of the strand mask (the engine's
//!   model runs behind [`super::Remover::fill`]), blurred a little, so its own texture is never used;
//! - **fine**: the background's grain and texture (finer than [`FINE`] of the face, at least [`FINE_MIN_PX`] pixels),
//!   put back from nearby clean pixels of the same tone (the first of [`SHIFTS`] whose source is not near a strand and
//!   whose low band is about the same), held to [`FINE_CLAMP`] times the typical fine band so a speck is not copied
//!   whole. Where no source is found the fill stays flat.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::morph::max_filter;
use lightcraft_raster::{Plane, Rgb32f};

/// The fine band's blur (sigma), a fraction of the face, and its floor in pixels.
const FINE: f64 = 0.002;
const FINE_MIN_PX: f64 = 2.0;
/// The fill is blurred to this many times the fine blur before it is used as the low band.
const LOW_BLUR: f32 = 1.5;
/// A donor's fine band is held to this many times the typical one (its median, as a Gaussian's sigma).
const FINE_CLAMP: f32 = 3.0;
/// A donor must have this much clean ground around it (0…1) and lie where the mask is under [`DONOR_MASK`].
const DONOR_CLEAN: f32 = 0.8;
const DONOR_MASK: f32 = 0.15;
/// A donor's low band may differ from the target's by this much (mean over the channels, log light).
const SIMILAR: f32 = 0.12;
/// The most a pixel is changed, either way (log light).
const MOST: f32 = 3.0;
/// How far the replacement's light may stray from the background's (log light), the background's blur (a fraction of the
/// face) and the least share of the blur's window that must be background to count.
const STRAY: f32 = 0.2;
const BACKGROUND_BLUR: f64 = 0.04;
const MIN_BACKGROUND: f32 = 0.02;
/// The replacement's range (log light): from black to far above white; a value outside it is a fill gone wrong.
const LOWEST: f32 = -9.3;
const HIGHEST: f32 = 6.0;
/// Where, as fractions of the face, a donor is looked for, in order (across, down).
const SHIFTS: [(f64, f64); 14] = [
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

fn fine_sigma(size: f64) -> f32 {
    (FINE * size).max(FINE_MIN_PX) as f32
}

/// The natural log of linear light per channel (floored, so black stays finite).
fn log_image(img: &Rgb32f) -> Rgb32f {
    img.map(|p| p.map(|v| if v.is_finite() { v.max(1e-4).ln() } else { 1e-4f32.ln() }))
}

/// The strand mask (0/1) grown by a pixel: what LaMa is asked to fill.
pub(super) fn hole(mask: &Plane) -> Plane {
    max_filter(mask, 1)
}

/// The strand mask as the weight of the replacement: grown by a pixel and softened, so a strand's anti-aliased edge goes
/// too, and a render that reads it between pixels still has the strand's core at full weight.
pub(super) fn weight(mask: &Plane) -> Plane {
    gaussian(&hole(mask), 0.8).map(|v| (v * 1.6).clamp(0.0, 1.0))
}

/// The replacement at each pixel of `work` (linear light) for the strand `mask`, given LaMa's `fill` of `hole(mask)`
/// (linear light, the same size): its log light per channel, only meaningful where [`weight`] is above 0 (elsewhere
/// 0); a face `size` pixels across.
pub(super) fn colour(work: &Rgb32f, mask: &Plane, background: &Plane, fill: &Rgb32f, size: f64) -> Rgb32f {
    let (w, h) = (work.width, work.height);
    let weight = weight(mask);
    let lg = log_image(work);
    let sigma = fine_sigma(size);
    // the low band of the replacement, and of the picture as it is (a donor's, compared with the replacement's)
    let low = gaussian(&log_image(fill), LOW_BLUR * sigma);
    // (a fill that strays from the background around it, as a learned fill can in a tangle of hair, is held to it)
    let low = held_to_background(&low, &lg, background, mask, size);
    let smooth = gaussian(&lg, LOW_BLUR * sigma);
    // the fine band: the picture less its slight blur, held to a few times what is typical
    let blurred = gaussian(&lg, sigma);
    let mut fine = lg.zip_map(&blurred, |a, b| std::array::from_fn(|k| a[k] - b[k]));
    let wide = max_filter(mask, ((0.004 * size).round() as usize).max(2));
    let clean = gaussian(&wide.map(|v| 1.0 - v), (0.006 * size).max(2.0) as f32);
    if let Some(typical) = typical_fine(&fine, &wide, &clean) {
        let limit = FINE_CLAMP * typical;
        fine.map_in_place(|p| p.map(|v| v.clamp(-limit, limit)));
    }
    let shifts: Vec<(isize, isize)> = SHIFTS.iter().map(|(sx, sy)| ((sx * size).round() as isize, (sy * size).round() as isize)).collect();
    let (iw, ih) = (w as isize, h as isize);
    Rgb32f::from_fn(w, h, |x, y| {
        if weight.get(x, y) <= 0.0 {
            return [0.0; 3];
        }
        // the first donor that is clean ground of the same tone
        let here = low.get(x, y);
        let donor = shifts.iter().find_map(|(sx, sy)| {
            let (nx, ny) = (x as isize + sx, y as isize + sy);
            if nx < 0 || ny < 0 || nx >= iw || ny >= ih {
                return None;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            let there = smooth.get(nx, ny);
            let apart = (0..3).map(|c| (there[c] - here[c]).abs()).sum::<f32>() / 3.0;
            (wide.get(nx, ny) < DONOR_MASK && clean.get(nx, ny) > DONOR_CLEAN && apart < SIMILAR).then(|| fine.get(nx, ny))
        });
        let f = donor.unwrap_or([0.0; 3]);
        std::array::from_fn(|c| {
            let v = here[c] + f[c];
            if v.is_finite() { v.clamp(LOWEST, HIGHEST) } else { 0.0 }
        })
    })
}

/// The change at each pixel of `work` (linear light) that puts `colour` (from [`colour`], log light) there where `weight`
/// is above 0: the log of new over old per channel, 0 elsewhere. A change, not the colour: the render the strands are
/// taken out of is the photo as edited (exposure, tone, white balance), the search's picture is not, and a ratio carries
/// across the two where a colour does not.
pub(super) fn log_ratio(work: &Rgb32f, colour: &Rgb32f, weight: &Plane) -> Rgb32f {
    let lg = log_image(work);
    Rgb32f::from_fn(work.width, work.height, |x, y| {
        if weight.get(x, y) <= 0.0 {
            return [0.0; 3];
        }
        let (c, l) = (colour.get(x, y), lg.get(x, y));
        std::array::from_fn(|k| {
            let d = c[k] - l[k];
            if d.is_finite() { d.clamp(-MOST, MOST) } else { 0.0 }
        })
    })
}

/// `low` with each channel held within [`STRAY`] (log light) of the background around: the picture's log light `lg` over
/// the pixels `background` marks (1 where it is plain background) and away from the strands, averaged by normalised
/// convolution at [`BACKGROUND_BLUR`] of the face. Where there is no background near, `low` is kept.
fn held_to_background(low: &Rgb32f, lg: &Rgb32f, background: &Plane, mask: &Plane, size: f64) -> Rgb32f {
    let away = max_filter(mask, ((0.004 * size).round() as usize).max(2));
    let support = background.zip_map(&away, |b, m| if b > 0.5 && m < 0.5 { 1.0 } else { 0.0 });
    let sigma = (BACKGROUND_BLUR * size).max(8.0) as f32;
    let weighted = Rgb32f::from_fn(lg.width, lg.height, |x, y| lg.get(x, y).map(|v| v * support.get(x, y)));
    let (num, den) = (gaussian(&weighted, sigma), gaussian(&support, sigma));
    Rgb32f::from_fn(low.width, low.height, |x, y| {
        let (l, d) = (low.get(x, y), den.get(x, y));
        if d < MIN_BACKGROUND {
            return l;
        }
        let bg = num.get(x, y).map(|v| v / d);
        std::array::from_fn(|c| l[c].clamp(bg[c] - STRAY, bg[c] + STRAY))
    })
}

/// The background's typical fine band: the median of the absolute fine band over clean ground away from the strands,
/// as a Gaussian's sigma; `None` without such ground.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dark_line_over_a_flat_background_is_replaced_by_the_background() {
        let work = Rgb32f::from_fn(96, 64, |x, _| if (47..=49).contains(&x) { [0.2; 3] } else { [0.5; 3] });
        let mask = Plane::from_fn(96, 64, |x, _| if (47..=49).contains(&x) { 1.0 } else { 0.0 });
        let fill = Rgb32f::filled(96, 64, [0.5; 3]);
        let (w, c) = (weight(&mask), colour(&work, &mask, &Plane::new(96, 64), &fill, 60.0));
        assert!(w.get(48, 30) > 0.99, "the strand's core is fully replaced");
        assert_eq!(w.get(10, 30), 0.0, "far from the strand nothing is");
        let v = c.get(48, 30)[0].exp();
        assert!((v - 0.5).abs() < 0.01, "{v}");
        assert_eq!(c.get(10, 30), [0.0; 3]);
    }

    #[test]
    fn the_backgrounds_grain_is_put_back_inside_the_strand() {
        // a checkered grain, ±5 %, with a bright line across it
        let grain = |x: usize, y: usize| if (x + y).is_multiple_of(2) { 0.52 } else { 0.48 };
        let work = Rgb32f::from_fn(120, 80, |x, y| if (59..=61).contains(&x) { [0.9; 3] } else { [grain(x, y); 3] });
        let mask = Plane::from_fn(120, 80, |x, _| if (59..=61).contains(&x) { 1.0 } else { 0.0 });
        let fill = Rgb32f::filled(120, 80, [0.5; 3]);
        let c = colour(&work, &mask, &Plane::new(120, 80), &fill, 80.0);
        // the new pixels vary like the grain (not one flat value), and stay near the background
        let vals: Vec<f32> = (0..8).map(|y| c.get(60, 20 + y)[0].exp()).collect();
        assert!(vals.iter().all(|v| (v - 0.5).abs() < 0.08), "{vals:?}");
        assert!(vals.windows(2).any(|p| (p[0] - p[1]).abs() > 0.005), "grain came back: {vals:?}");
    }

    #[test]
    fn the_change_is_the_log_of_new_over_old_where_there_is_weight() {
        let work = Rgb32f::from_fn(8, 1, |x, _| if x == 3 { [0.1; 3] } else { [0.5; 3] });
        let colour = Rgb32f::filled(8, 1, [0.5f32.ln(); 3]);
        let weight = Plane::from_fn(8, 1, |x, _| if x == 3 { 1.0 } else { 0.0 });
        let d = log_ratio(&work, &colour, &weight);
        assert!((d.get(3, 0)[0] - 5f32.ln()).abs() < 1e-4);
        assert_eq!(d.get(2, 0), [0.0; 3]);
    }

    #[test]
    fn a_fill_that_strays_from_the_background_is_held_to_it() {
        // a bright wrong fill (0.9) where the background all round is 0.5
        let work = Rgb32f::from_fn(120, 80, |x, _| if (59..=61).contains(&x) { [0.1; 3] } else { [0.5; 3] });
        let mask = Plane::from_fn(120, 80, |x, _| if (59..=61).contains(&x) { 1.0 } else { 0.0 });
        let fill = Rgb32f::filled(120, 80, [0.9; 3]);
        let background = Plane::filled(120, 80, 1.0);
        let c = colour(&work, &mask, &background, &fill, 80.0);
        let v = c.get(60, 40)[0].exp();
        assert!(v < 0.5 * (STRAY).exp() * 1.01, "held within {STRAY} of the background: {v}");
        // with no background marked the fill is used as it is
        let c = colour(&work, &mask, &Plane::new(120, 80), &fill, 80.0);
        assert!((c.get(60, 40)[0].exp() - 0.9).abs() < 0.02);
    }

    #[test]
    fn nothing_to_remove_gives_nothing() {
        let work = Rgb32f::filled(40, 40, [0.3; 3]);
        let none = Plane::new(40, 40);
        assert!(weight(&none).data.iter().all(|v| *v == 0.0));
        assert!(colour(&work, &none, &none, &work, 30.0).data.iter().all(|p| *p == [0.0; 3]));
    }
}
