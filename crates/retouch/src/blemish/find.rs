//! Finding the spots on a face's skin, once per photo (the provider calls it on the picture its masks come from).
//!
//! On the Facial Skin mask's box, reduced so the face is at most [`WORK`] pixels:
//!
//! 1. **Skin-only blurs** of an "absorbing" channel ([`ABSORB`]: mostly green, which blood and melanin both absorb)
//!    and of red, at the sizes of [`SCALES`] (each the picture weighted by the mask, divided by the blurred mask, so
//!    hair, eyes and lips never count as a spot's surroundings).
//! 2. **Response** per size: how much darker the pixel's blur is than the skin around it (the blur [`SURROUND`]
//!    sizes up), in natural log. A spot of radius r answers most at the size r/√2.
//! 3. **Spots** are the places darker than their surroundings by more than [`FIND`] at their best size and darker
//!    than every neighbour; no larger structure behind them (the last size, past 3 % of the face, answering more
//!    means a nostril or a shadow, not a spot); round (not a line or a wrinkle: the response's curvature is at most
//!    [`MAX_ELONGATION`] times longer one way); with skin all around. Overlapping finds keep the strongest.
//! 4. **Kinds**: a spot whose red darkens nearly as much as the rest is brown (melanin), one whose red stays is red
//!    (blood). Brown, dark ([`MOLE`]) and round is a mole; small and brown among many like it is a freckle when faint
//!    and a mole when dark; anything else is a blemish.
//! 5. **Donor**: a clean patch of skin next to each spot, whose texture the repair borrows.

use lightcraft_raster::blur::{gauss_taps, gaussian, gaussian_taps};
use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};

use crate::input::{Spot, SpotKind, Spots};
use crate::{Part, Scene};

/// The face's size, in pixels, detection works at: a bigger face is reduced to it (spots under about 2 of these
/// pixels are not looked for), a smaller one is worked on as it is.
pub const WORK: f64 = 640.0;

/// The sizes looked at (Gaussian σ, fractions of the face's size), smallest first. A spot of radius r answers most
/// at σ = r/√2, so all but the last cover spots of 0.3–3 % of the face across; the last only tells a larger
/// structure (it must answer less than the spot's own size).
pub const SCALES: [f64; 8] = [0.0011, 0.0016, 0.00234, 0.00342, 0.005, 0.0073, 0.0107, 0.0156];

/// The surroundings a size is compared with: the blur this many sizes up ([`SCALES`] continue by the same ratio).
const SURROUND: usize = 2;
/// The ratio between two neighbouring sizes.
const STEP: f64 = 1.46;

/// The absorbing channel's weights (R, G, B): spots darken green most (blood and melanin), blue next.
pub const ABSORB: [f32; 3] = [0.15, 0.65, 0.2];

/// A spot is darker than the skin around it by at least this (natural log: about 6 %).
pub const FIND: f32 = 0.06;
/// The two smallest sizes need this much more (pores and grain are small and faint): ×1.6 and ×1.3.
const SMALL_EXTRA: [f32; 2] = [0.6, 0.3];
/// A spot this much darker than the skin around it counts fully ([`Spot::strength`] 1).
pub const STRONG: f32 = 0.3;
/// The response's curvature may be at most this many times stronger one way than the other (a line or a wrinkle
/// is much longer than wide), per size: the smallest (a hair, a pore's edge) and the largest (a crease, the
/// corner of a nostril) must be rounder.
pub const MAX_ELONGATION: [f32; 7] = [2.0, 2.2, 2.5, 2.5, 2.2, 1.8, 1.8];
/// Red darkening as much as the rest, or more, is a shadow or a hair, not a spot: blood leaves red as it is and
/// melanin darkens it less than green. A spot's red share is at most this.
pub const NEUTRAL: f32 = 0.92;
/// The mask must be at least this all around a spot (1.5 radii out): a spot at the skin's edge is a lash, a lip
/// line or hair.
const SKIN_ALL_AROUND: f32 = 0.9;

/// A spot reaches as far as the largest size that still answers this share of its best size.
const EXTENT: f32 = 0.75;

/// The texture around a spot is measured over this share of the face's size …
const BUSY: f64 = 0.02;
/// … and the spot must be this many times darker than the texture's mean response.
pub const STAND_OUT: f32 = 4.0;

/// Skin around a place more textured than this is a beard, stubble or brows, not clean skin: nothing there is a
/// spot.
pub const TOO_BUSY: f32 = 0.06;

/// The blur the skin's brightness around a spot is measured with, a fraction of the face's size …
const BASE: f64 = 0.03;
/// … and the least share of the face's median skin brightness it may have.
pub const DARKEST_SKIN: f32 = 0.6;

/// Skin whose mean colour has less saturation than this is grey (a black-and-white photo).
const GREY: f64 = 0.04;

/// Brown: red darkens at least this share of how much the absorbing channel does (blood leaves red as it is).
pub const BROWN: f32 = 0.45;
/// A mole: brown and darker than the skin around it by at least this (natural log: about 25 %).
pub const MOLE: f32 = 0.22;
/// A mole is round: at most this elongation.
const MOLE_ROUND: f32 = 1.8;
/// A brown spot at most [`FRECKLE_RADIUS`] across (a fraction of the face's size) with at least [`FRECKLE_GROUP`]
/// others like it within [`FRECKLE_NEAR`] is a freckle when fainter than this, else a mole.
pub const FRECKLE_FAINT: f32 = 0.2;
const FRECKLE_RADIUS: f64 = 0.006;
pub const FRECKLE_GROUP: usize = 5;
const FRECKLE_NEAR: f64 = 0.12;

/// Most spots kept per face (the strongest).
pub const MAX_SPOTS: usize = 400;
/// Most pixels of the mask's box looked at (the box is reduced to the face's [`WORK`] size first).
const MAX_BOX: usize = 1 << 26;
/// Most candidate places looked at further (the strongest).
const MAX_CANDIDATES: usize = 20_000;

/// The skin of one face, reduced: what detection works on.
struct Work {
    /// The box's corner in the image's pixels and the reduction (image pixels per work pixel).
    x0: usize,
    y0: usize,
    factor: usize,
    /// The face's size in work pixels.
    size: f64,
    w: usize,
    h: usize,
    /// Per pixel: absorbing channel × mask, red × mask, mask.
    weighted: Image<[f32; 3]>,
    /// The face's skin area in image pixels.
    skin_px: f64,
    /// The skin has no colour (a black-and-white photo): red can't tell a spot from a shadow.
    grey: bool,
}

/// One place that answered, with what it is measured by.
#[derive(Clone, Copy, Debug)]
struct Found {
    x: usize,
    y: usize,
    scale: usize,
    contrast: f32,
    /// Red's darkening as a share of the absorbing channel's.
    red_share: f32,
    elongation: f32,
    /// Radius in work pixels.
    radius: f64,
}

/// The spots on face `face` of `scene` (on `img`, the picture the face's masks come from). Empty without a Facial
/// Skin mask, or when the face has no skin in the picture.
pub fn find(img: &Rgb32f, scene: &Scene<'_>, face: usize) -> Spots {
    let Some(work) = work(img, scene, face) else { return Spots::default() };
    let size_px = scene.faces.get(face).map_or(1.0, |f| f.size).max(1.0);
    let skin = work.skin_px / (size_px * size_px);
    let (resp, red) = responses(&work);
    let busy = busy(&work, &resp);
    let base = base(&work);
    let mut found = candidates(&work, &resp, &red, &busy);
    found.retain(|f| is_skin_bright(&base, f));
    found.sort_by(|a, b| b.contrast.total_cmp(&a.contrast));
    let kept = suppress(found);
    let kinds = kinds(&kept, work.size);
    let list = kept
        .iter()
        .zip(kinds)
        .filter_map(|(f, kind)| {
            let at = to_norm(scene, &work, f.x as f64 + 0.5, f.y as f64 + 0.5)?;
            let donor = donor(&work, &resp, &kept, f).and_then(|(dx, dy)| to_norm(scene, &work, dx, dy));
            let strength = ((f.contrast - FIND) / (STRONG - FIND)).clamp(0.0, 1.0);
            Some(Spot { at, radius: f.radius * work.factor as f64 / size_px, donor, kind, strength })
        })
        .collect();
    Spots { list, skin }
}

/// Work pixel (`x`, `y`) → normalised photo coordinates.
fn to_norm(scene: &Scene<'_>, w: &Work, x: f64, y: f64) -> Option<(f64, f64)> {
    let f = w.factor as f64;
    let p = scene.view().to_norm(w.x0 as f64 + x * f, w.y0 as f64 + y * f);
    (p.0.is_finite() && p.1.is_finite()).then_some(p)
}

fn work(img: &Rgb32f, scene: &Scene<'_>, face: usize) -> Option<Work> {
    if img.data.len() != img.width.saturating_mul(img.height) {
        return None;
    }
    let size = scene.faces.get(face)?.size;
    let mask = scene.faces.get(face)?.face.mask(Part::FacialSkin)?;
    let [x0, y0, x1, y1] = scene.mask_box(face, Part::FacialSkin)?;
    let (bw, bh) = (x1 - x0, y1 - y0);
    if bw.saturating_mul(bh) > MAX_BOX || !size.is_finite() {
        return None;
    }
    let factor = ((size / WORK).floor() as usize).clamp(1, bw.max(bh).max(1));
    let (w, h) = (bw.div_ceil(factor), bh.div_ceil(factor));
    let view = scene.view();
    let mut weighted = Image::<[f32; 3]>::new(w, h);
    let mut skin_rows = vec![0.0f64; h];
    par_rows(&mut weighted.data, w.max(1), |sy, row| {
        let ys = y0 + sy * factor..(y0 + (sy + 1) * factor).min(y1);
        for (sx, o) in row.iter_mut().enumerate() {
            let xs = x0 + sx * factor..(x0 + (sx + 1) * factor).min(x1);
            let mut acc = [0.0f32; 3];
            for y in ys.clone() {
                for x in xs.clone() {
                    let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
                    let m = mask.sample(nx, ny).clamp(0.0, 1.0);
                    let p = img.data.get(y * img.width + x).copied().unwrap_or_default();
                    let a = (p[0] * ABSORB[0] + p[1] * ABSORB[1] + p[2] * ABSORB[2]).max(0.0);
                    acc = [acc[0] + a * m, acc[1] + p[0].max(0.0) * m, acc[2] + m];
                }
            }
            let n = (ys.len() * xs.len()).max(1) as f32;
            *o = acc.map(|v| if v.is_finite() { v / n } else { 0.0 });
        }
    });
    for (s, row) in skin_rows.iter_mut().zip(weighted.data.chunks(w.max(1))) {
        *s = row.iter().map(|p| f64::from(p[2])).sum::<f64>() * (factor * factor) as f64;
    }
    let skin_px: f64 = skin_rows.iter().sum();
    // the skin's mean colour, from up to 128 × 128 samples
    let step = (bw.max(bh) / 128).max(1);
    let mut mean = [0.0f64; 3];
    for y in (y0..y1).step_by(step) {
        for x in (x0..x1).step_by(step) {
            let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
            let m = f64::from(mask.sample(nx, ny));
            let p = img.data.get(y * img.width + x).copied().unwrap_or_default();
            mean = std::array::from_fn(|c| mean[c] + f64::from(p[c].max(0.0)) * m);
        }
    }
    let (hi, lo) = (mean[0].max(mean[1]).max(mean[2]), mean[0].min(mean[1]).min(mean[2]));
    let grey = hi <= 0.0 || (hi - lo) / hi < GREY;
    (skin_px > 0.0).then_some(Work { x0, y0, factor, size: size / factor as f64, w, h, weighted, skin_px, grey })
}

/// Blurs under this σ (pixels) use exact Gaussian taps (the three-box blur snaps small sizes to whole pixels).
const EXACT_BELOW: f32 = 2.0;

fn blur(img: &Image<[f32; 3]>, sigma: f32) -> Image<[f32; 3]> {
    if sigma < EXACT_BELOW && sigma.is_finite() && sigma > 0.0 { gaussian_taps(img, &gauss_taps(sigma)) } else { gaussian(img, sigma) }
}

/// The blur at size `i` ([`SCALES`], continued by [`STEP`] past its end), in work pixels.
fn sigma(work: &Work, i: usize) -> f32 {
    let last = SCALES.len() - 1;
    let f = SCALES.get(i).copied().unwrap_or_else(|| SCALES[last] * STEP.powi((i - last) as i32));
    (f * work.size) as f32
}

/// Per size of [`SCALES`]: the response (how much darker than the surroundings, natural log; 0 off the skin) and
/// red's darkening at the same size.
fn responses(work: &Work) -> (Vec<Plane>, Vec<Plane>) {
    let (w, h) = (work.w, work.h);
    let n = SCALES.len();
    let mut levels: Vec<Image<[f32; 3]>> = (0..SURROUND).map(|i| blur(&work.weighted, sigma(work, i))).collect();
    let (mut resp, mut red) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let outer = blur(&work.weighted, sigma(work, i + SURROUND));
        let inner = levels.remove(0);
        let mut r = Plane::new(w, h);
        let mut d = Plane::new(w, h);
        let row_of = |img: &Image<[f32; 3]>, y: usize| img.data.get(y * w..(y + 1) * w).unwrap_or(&[]).to_vec();
        par_rows(&mut r.data, w.max(1), |y, row| {
            let (a, b) = (row_of(&inner, y), row_of(&outer, y));
            for ((o, p), q) in row.iter_mut().zip(&a).zip(&b) {
                *o = if p[2] >= 0.5 && q[2] > 1e-3 { ln_ratio(q[0] / q[2], p[0] / p[2]) } else { 0.0 };
            }
        });
        par_rows(&mut d.data, w.max(1), |y, row| {
            let (a, b) = (row_of(&inner, y), row_of(&outer, y));
            for ((o, p), q) in row.iter_mut().zip(&a).zip(&b) {
                *o = if p[2] >= 0.5 && q[2] > 1e-3 { ln_ratio(q[1] / q[2], p[1] / p[2]) } else { 0.0 };
            }
        });
        resp.push(r);
        red.push(d);
        levels.push(outer);
    }
    (resp, red)
}

/// ln(a / b), 0 when either is not a positive number.
fn ln_ratio(a: f32, b: f32) -> f32 {
    const DARK: f32 = 1e-5;
    if a > DARK && b > DARK && a.is_finite() && b.is_finite() { (a / b).ln() } else { 0.0 }
}

fn at(p: &Plane, x: isize, y: isize) -> f32 {
    if x < 0 || y < 0 || x as usize >= p.width || y as usize >= p.height {
        return 0.0;
    }
    p.data.get(y as usize * p.width + x as usize).copied().unwrap_or(0.0)
}

/// The threshold of size `i`.
fn threshold(i: usize) -> f32 {
    FIND * (1.0 + SMALL_EXTRA.get(i).copied().unwrap_or(0.0))
}

/// Every place darker than its neighbours at its best size, by more than that size's threshold, with no larger
/// structure behind it, round and with skin all around.
fn candidates(work: &Work, resp: &[Plane], red: &[Plane], busy: &Plane) -> Vec<Found> {
    let (w, h) = (work.w, work.h);
    let detect = SCALES.len() - 1;
    // the best size per pixel
    let mut best = Plane::new(w, h);
    let mut scale = vec![0u8; w * h];
    for (i, r) in resp.iter().enumerate().take(detect) {
        for (j, v) in r.data.iter().enumerate() {
            if let (Some(b), Some(s)) = (best.data.get_mut(j), scale.get_mut(j))
                && *v > *b
            {
                *b = *v;
                *s = i as u8;
            }
        }
    }
    let mut out = Vec::new();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let j = y * w + x;
            let (v, i) = (best.data.get(j).copied().unwrap_or(0.0), scale.get(j).copied().unwrap_or(0) as usize);
            if v < threshold(i) {
                continue;
            }
            let (xi, yi) = (x as isize, y as isize);
            // a local maximum (ties go to the first in reading order)
            let n = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
            let peak = n.iter().enumerate().all(|(k, (dx, dy))| {
                let o = at(&best, xi + dx, yi + dy);
                if k < 4 { v > o } else { v >= o }
            });
            if !peak {
                continue;
            }
            // no larger structure behind it
            if resp.get(detect).is_some_and(|r| at(r, xi, yi) >= v) {
                continue;
            }
            let Some(f) = measure(work, resp, red, busy, x, y, i, v) else { continue };
            out.push(f);
            if out.len() >= MAX_CANDIDATES {
                return out;
            }
        }
    }
    out
}

/// The place's shape and colour; `None` when it is a line or lies at the skin's edge.
#[allow(clippy::too_many_arguments)]
fn measure(work: &Work, resp: &[Plane], red: &[Plane], busy: &Plane, x: usize, y: usize, i: usize, v: f32) -> Option<Found> {
    let r = resp.get(i)?;
    let s = sigma(work, i);
    let radius = f64::from(s) * std::f64::consts::SQRT_2;
    let (xi, yi) = (x as isize, y as isize);
    let step = (s.round() as isize).max(1);
    let at_r = |dx: isize, dy: isize| at(r, xi + dx * step, yi + dy * step);
    let st = (step * step) as f32;
    // the curvature of the response (negated: a peak curves down)
    let dxx = -(at_r(1, 0) + at_r(-1, 0) - 2.0 * v) / st;
    let dyy = -(at_r(0, 1) + at_r(0, -1) - 2.0 * v) / st;
    let dxy = -(at_r(1, 1) - at_r(1, -1) - at_r(-1, 1) + at_r(-1, -1)) / (4.0 * st);
    let (tr, det) = (dxx + dyy, dxx * dyy - dxy * dxy);
    if det <= 0.0 || tr <= 0.0 {
        return None;
    }
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (big, small) = (tr / 2.0 + disc, tr / 2.0 - disc);
    let elongation = if small > 0.0 { big / small } else { f32::INFINITY };
    if elongation > MAX_ELONGATION.get(i).copied().unwrap_or(0.0) {
        return None;
    }
    if !skin_all_around(work, x as f64 + 0.5, y as f64 + 0.5, radius * 1.5 + 1.0) {
        return None;
    }
    let around: f32 = (0..8)
        .map(|k| {
            let a = f64::from(k) * std::f64::consts::TAU / 8.0;
            let d = radius * 3.0 + 1.0;
            at(busy, (x as f64 + d * a.cos()) as isize, (y as f64 + d * a.sin()) as isize)
        })
        .sum::<f32>()
        / 8.0;
    if v < STAND_OUT * around || around > TOO_BUSY {
        return None;
    }
    // (without colour every spot reads as brown: shape and contrast alone tell them apart)
    let red_share = if work.grey { BROWN + 0.2 } else { red.get(i).map_or(0.0, |d| at(d, xi, yi)) / v.max(1e-6) };
    if red_share > NEUTRAL {
        return None;
    }
    // a mark with a faint wide rim answers almost as much at the next sizes up: it reaches as far as they do
    let reach = (i + 1..SCALES.len() - 1).take_while(|k| resp.get(*k).is_some_and(|p| at(p, xi, yi) >= EXTENT * v)).last().unwrap_or(i);
    let radius = f64::from(sigma(work, reach)) * std::f64::consts::SQRT_2;
    Some(Found { x, y, scale: i, contrast: v, red_share, elongation, radius })
}

/// How textured the skin is around each place: the mean size of the small responses (the second size, either
/// way) over [`BUSY`] of the face. A beard, stubble or rough skin is busy; a spot must stand out from it.
fn busy(work: &Work, resp: &[Plane]) -> Plane {
    let Some(r) = resp.get(1) else { return Plane::new(work.w, work.h) };
    let abs = Plane { width: r.width, height: r.height, data: r.data.iter().map(|v| v.abs()).collect() };
    gaussian(&abs, (BUSY * work.size) as f32)
}

/// The skin's brightness around each place ([`BASE`] blur), as a ratio to the face's median skin brightness.
fn base(work: &Work) -> Plane {
    let blurred = blur(&work.weighted, (BASE * work.size) as f32);
    let mut ys: Vec<f32> = blurred.data.iter().filter(|p| p[2] >= 0.9).map(|p| p[0] / p[2]).filter(|v| v.is_finite()).collect();
    let median = if ys.is_empty() {
        0.0
    } else {
        let mid = ys.len() / 2;
        *ys.select_nth_unstable_by(mid, f32::total_cmp).1
    };
    Plane {
        width: work.w,
        height: work.h,
        data: blurred.data.iter().map(|p| if p[2] > 1e-3 && median > 1e-6 { p[0] / p[2] / median } else { 0.0 }).collect(),
    }
}

/// A spot's surroundings are lit skin: at least [`DARKEST_SKIN`] of the face's median. Darker is a beard, stubble,
/// the hairline or deep shadow, where there is nothing to retouch.
fn is_skin_bright(base: &Plane, f: &Found) -> bool {
    at(base, f.x as isize, f.y as isize) >= DARKEST_SKIN
}

/// The mask (reduced, not blurred) at least [`SKIN_ALL_AROUND`] at the centre and at 12 points `r` out.
fn skin_all_around(work: &Work, cx: f64, cy: f64, r: f64) -> bool {
    let m = |x: f64, y: f64| {
        if x < 0.0 || y < 0.0 {
            return 0.0;
        }
        let (xi, yi) = (x as usize, y as usize);
        if xi >= work.w || yi >= work.h {
            return 0.0;
        }
        work.weighted.data.get(yi * work.w + xi).map_or(0.0, |p| p[2])
    };
    m(cx, cy) >= SKIN_ALL_AROUND
        && (0..12).all(|k| {
            let a = k as f64 * std::f64::consts::TAU / 12.0;
            m(cx + r * a.cos(), cy + r * a.sin()) >= SKIN_ALL_AROUND
        })
}

/// The strongest first: a find overlapping a stronger one is the same spot.
fn suppress(found: Vec<Found>) -> Vec<Found> {
    let mut kept: Vec<Found> = Vec::new();
    for f in found {
        let near = kept.iter().any(|k| {
            let d = ((k.x as f64 - f.x as f64).powi(2) + (k.y as f64 - f.y as f64).powi(2)).sqrt();
            d < (k.radius + f.radius) * 0.75
        });
        if !near {
            kept.push(f);
            if kept.len() >= MAX_SPOTS {
                break;
            }
        }
    }
    kept
}

fn is_brown(f: &Found) -> bool {
    f.red_share >= BROWN
}

/// Each spot's kind: moles, then freckles (brown faint small spots in a group), then blemishes.
fn kinds(spots: &[Found], size: f64) -> Vec<SpotKind> {
    let small_brown = |f: &Found| is_brown(f) && f.radius <= FRECKLE_RADIUS * size;
    let near = FRECKLE_NEAR * size;
    spots
        .iter()
        .map(|f| {
            if is_brown(f) && f.contrast >= MOLE && f.elongation <= MOLE_ROUND {
                return SpotKind::Mole;
            }
            if small_brown(f) {
                let group = spots
                    .iter()
                    .filter(|o| !std::ptr::eq(*o, f) && small_brown(o))
                    .filter(|o| ((o.x as f64 - f.x as f64).powi(2) + (o.y as f64 - f.y as f64).powi(2)).sqrt() <= near)
                    .count();
                // one of many small brown spots: a freckle when faint, a mole when dark
                if group >= FRECKLE_GROUP {
                    return if f.contrast < FRECKLE_FAINT { SpotKind::Freckle } else { SpotKind::Mole };
                }
            }
            SpotKind::Blemish
        })
        .collect()
}

/// A clean patch of skin next to spot `f` (work pixels): of 16 places 2.6 and 3.4 radii away, the one with skin all
/// around, clear of every spot, answering least at the spot's size and closest to the skin around the spot in
/// brightness. `None` when none has skin all around.
fn donor(work: &Work, resp: &[Plane], spots: &[Found], f: &Found) -> Option<(f64, f64)> {
    let (cx, cy) = (f.x as f64 + 0.5, f.y as f64 + 0.5);
    let r = f.radius;
    let resp = resp.get(f.scale)?;
    let bright = |x: f64, y: f64| {
        let (xi, yi) = (x.max(0.0) as usize, y.max(0.0) as usize);
        work.weighted.data.get(yi.min(work.h.saturating_sub(1)) * work.w + xi.min(work.w.saturating_sub(1))).map_or(0.0, |p| p[0] / p[2].max(1e-3))
    };
    let ring: f32 =
        (0..8).map(|k| f64::from(k) * std::f64::consts::TAU / 8.0).map(|a| bright(cx + 1.8 * r * a.cos(), cy + 1.8 * r * a.sin())).sum::<f32>() / 8.0;
    let mut best: Option<(f32, (f64, f64))> = None;
    for dist in [2.6, 3.4] {
        for k in 0..8 {
            let a = (f64::from(k) + if dist > 3.0 { 0.5 } else { 0.0 }) * std::f64::consts::TAU / 8.0;
            let (dx, dy) = (cx + dist * r * a.cos(), cy + dist * r * a.sin());
            if !skin_all_around(work, dx, dy, r * 1.6 + 1.0) {
                continue;
            }
            let clear = spots.iter().all(|o| {
                let d = ((o.x as f64 + 0.5 - dx).powi(2) + (o.y as f64 + 0.5 - dy).powi(2)).sqrt();
                std::ptr::eq(o, f) || d > (o.radius + r) * 1.3
            });
            if !clear {
                continue;
            }
            let pts = (0..9).map(|j| {
                let b = f64::from(j) * std::f64::consts::TAU / 8.0;
                let rr = if j == 8 { 0.0 } else { r * 0.7 };
                (dx + rr * b.cos(), dy + rr * b.sin())
            });
            let (mut busy, mut light) = (0.0f32, 0.0f32);
            for (px, py) in pts {
                busy += at(resp, px as isize, py as isize).abs();
                light += bright(px, py);
            }
            let score = busy / 9.0 + ln_ratio(light / 9.0, ring).abs();
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, (dx, dy)));
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sizes_cover_spots_of_three_tenths_to_three_percent_of_the_face() {
        let across = |s: f64| 2.0 * std::f64::consts::SQRT_2 * s;
        assert!((across(SCALES[0]) - 0.003).abs() < 0.0003);
        assert!((across(SCALES[SCALES.len() - 2]) - 0.03).abs() < 0.002);
        for w in SCALES.windows(2) {
            assert!((w[1] / w[0] - STEP).abs() < 0.02, "{w:?}");
        }
    }
}
