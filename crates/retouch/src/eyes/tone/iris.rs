//! Iris: the iris's own pattern brought out, its pupil, limbal ring and catchlight kept.
//!
//! All in proportion to the slider, on the Iris mask cut to the iris's circle and the eye's opening:
//! - **Pattern**: the fibres' local contrast (a fine scale) and a little clarity (a coarser one) are raised, each
//!   against the iris's own surroundings only, so the pupil, the sclera and a catchlight don't halo into it.
//! - **Light**: the iris gets brighter between the pupil and the limbal ring, more in its lower part, where light
//!   enters the eye, less under the upper lid's shadow; a dark iris more than a light one.
//! - **Colour**: a little more saturation of its own hue.
//!
//! The pupil (its radius read from the iris's profile, out from the centre) and the dark limbal ring at the rim stay as
//! they are, and so does a catchlight (much brighter than the iris).

use lightcraft_raster::{Image, Rgb32f};

use super::super::geom::{Eye, smoothstep};
use super::super::grid::{blur, luma, sample};
use super::{Patch, opening, quantile, response, weighted_blur};
use crate::{Part, Scene};

/// The polar grid (radii, angles) the iris's profile is read on.
const PROFILE: (usize, usize) = (24, 48);
/// The pupil's radius, as a share of the iris's, when the profile shows none (a dark iris), and its bounds.
const PUPIL_DEFAULT: f64 = 0.36;
const PUPIL_RANGE: (f64, f64) = (0.2, 0.62);
/// A pupil shows when the iris is this much brighter than the darkest of its centre.
const PUPIL_SHOWS: f32 = 1.6;
/// Past the pupil's edge (in iris radii), where the brightening (and the added colour) starts and where it is whole;
/// where it fades out toward the limbal ring, well inside it (the mesh's circle can be a little larger than the iris).
const LIGHT_IN: (f64, f64) = (0.03, 0.2);
const LIGHT_OUT: (f64, f64) = (0.6, 0.86);
/// The same for the pattern's contrast: from the pupil's edge to the limbal ring.
const DETAIL_IN: (f64, f64) = (-0.02, 0.06);
const DETAIL_OUT: (f64, f64) = (0.82, 0.98);
/// The iris's circle, softened (in its radii): the mask is cut to it.
const DISC: (f64, f64) = (0.92, 1.1);
/// A catchlight: brighter than this many times the iris's level (wholly from the upper one).
const CATCHLIGHT: (f32, f32) = (2.2, 3.5);
/// The opening's edge for the iris ([`opening::Fade`]): only inside it, so the lids' rims and lashes over the iris
/// are left alone.
const FADE: opening::Fade = (0.0, 0.06);
/// At the lower lid, whose wet rim lies just inside the mesh's outline, the change starts further in.
const FADE_LOWER: opening::Fade = (0.0, 0.12);
/// The scales (in iris radii) of the iris's bands: the fibres lie between the first two, what clarity raises between
/// the second and the third. Grain finer than the first is left as it is.
const SIGMAS: (f64, f64, f64) = (0.02, 0.05, 0.2);
/// How much the fibres' and the clarity's contrast is raised at 100 (1 doubles it).
const FINE: f32 = 2.4;
const CLARITY: f32 = 1.0;
/// Detail under this share of its surroundings is noise: left as it is; for an iris whose detail is small (a dark
/// one), under `CORE_SHARE` of its own spread instead.
const CORE: f32 = 0.015;
const CORE_SHARE: f32 = 0.35;
/// Each iris's pattern is raised toward this spread of detail (the mean size of its detail against its surroundings),
/// fibres then clarity, by up to `MAX_BOOST` times the usual raise: a dark iris, whose pattern is small against its
/// level, shows it too. (A grainy dark iris's fine band is mostly grain, so its clarity band carries the lift.)
const SPREAD: (f32, f32) = (0.08, 0.2);
const MAX_BOOST: f32 = 3.0;
/// Near the lids (within this many half eye widths of the opening's outline, fading out at the second), a pixel
/// much brighter than the iris and of another colour is the lid's wet rim, not iris: left alone.
const RIM_ZONE: (f64, f64) = (0.06, 0.16);
const RIM_BRIGHT: (f64, f64) = (1.15, 1.8);
const RIM_COLOUR: (f64, f64) = (0.1, 0.3);
/// How far down the iris (in its radii) the lower lid's rim is looked for everywhere, not only by the outline.
const LOW_RIM: (f64, f64) = (0.3, 0.6);
/// Outside the pupil, a pixel this much darker than the iris (wholly at the first share) is a lid's shadow, a lash or
/// the limbal ring: left as it is, and out of the blurs.
const LID_SHADOW: (f64, f64) = (0.3, 0.55);
/// There, a pixel this much brighter than the iris is not iris whatever its colour (a rim of the iris's own hue).
const RIM_BRIGHTER: (f64, f64) = (1.8, 2.6);
/// Most a detail darkens or brightens, as a share of its surroundings, before raising.
const DETAIL: (f32, f32) = (-0.3, 0.35);
/// The most the raised pattern darkens or brightens a pixel (a factor), whatever its detail: an edge the masks let in
/// (a lid's, a reflection's) is never blown up.
const DETAIL_GAIN: (f32, f32) = (0.7, 1.8);
/// The brightening at 100, in stops: for a dark iris and a light one (by the iris's level, linear).
const LIGHT: (f32, f32) = (1.2, 0.85);
/// An iris with no colour to speak of (a near-black or grey one, its chroma as a share of its level under the first
/// value) gets this share of the brightening: lifting it only greys it.
const GREY_IRIS: (f64, f64, f32) = (0.1, 0.35, 0.5);
const LIGHT_LEVELS: (f64, f64) = (0.04, 0.15);
/// The share of the brightening the iris's upper part (under the lid's shadow) gets; the lower part gets it whole.
const UPPER: f32 = 0.4;
/// More saturation at 100 (0.3: chroma × 1.3).
const SATURATION: f32 = 0.2;

/// One iris: centre (pixels), radius (pixels), the pupil's radius (a share of it), the iris's level, the eye's down.
struct Circle {
    c: (f64, f64),
    r: f64,
    pupil: f64,
    level: f32,
    /// The iris's colour: each channel over the luminance (the median of its samples).
    colour: [f32; 3],
    down: (f64, f64),
}

impl Circle {
    /// Pixel (`x`, `y`) as distance from the centre (in radii) and how far down (in radii).
    fn polar(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.c.0, y - self.c.1);
        (dx.hypot(dy) / self.r, (dx * self.down.0 + dy * self.down.1) / self.r)
    }
}

/// Iris, `amount` 0…1.
pub fn iris(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, eyes: &[Eye], amount: f32) {
    let Some(ctx) = scene.faces.get(face) else { return };
    let Some(mask) = ctx.face.mask(Part::Iris) else { return };
    let Some(bx) = scene.mask_box(face, Part::Iris) else { return };
    let openings = opening::of(ctx, eyes);
    let view = scene.view();
    let mask_at = |x: f64, y: f64| {
        let (nx, ny) = view.to_norm(x, y);
        mask.sample(nx, ny)
    };
    let circles: Vec<Circle> = eyes.iter().filter_map(|e| circle(img, e, &mask_at, &openings)).collect();
    if circles.is_empty() {
        return;
    }
    let k = response(amount);
    let r_mean = circles.iter().map(|c| c.r).sum::<f64>() / circles.len() as f64;
    let pad = (3.0 * SIGMAS.2 * r_mean).ceil() as usize + 1;
    let Some(patch) = Patch::grown(bx, pad, img.width, img.height) else { return };
    let n = patch.w * patch.h;
    let rgb = Image::<[f32; 3]>::from_fn(patch.w, patch.h, |x, y| {
        img.data.get(patch.image_index(y * patch.w + x, img.width)).copied().filter(|p| p.iter().all(|v| v.is_finite())).unwrap_or([0.0; 3])
    });
    let lum: Vec<f32> = rgb.data.iter().map(|p| luma(*p)).collect();
    // the pixels' colour at the finest band's scale (grain out), for telling the lid's rim from the iris
    let smooth = blur(&rgb, (SIGMAS.0 * r_mean).max(0.7)).data;
    // per cell: the iris's weight (for the blurs, catchlights and rims out), the change's weight, where it is
    let (mut region, mut change, mut at) = (vec![0.0f32; n], vec![0.0f32; n], vec![(0.0f64, 0.0f64, 0usize); n]);
    for j in 0..n {
        let (x, y) = patch.centre(j);
        let Some((ci, c)) = circles.iter().enumerate().min_by(|(_, a), (_, b)| a.polar(x, y).0.total_cmp(&b.polar(x, y).0)) else { continue };
        let (rho, down) = c.polar(x, y);
        let m = if rho > DISC.1 { 0.0 } else { mask_at(x, y) };
        let (Some(&l), Some(&s)) = (lum.get(j), smooth.get(j)) else { continue };
        if m <= 0.0 {
            continue;
        }
        let catch = smoothstep((CATCHLIGHT.0 * c.level) as f64, (CATCHLIGHT.1 * c.level) as f64, l as f64);
        let depth = opening::depth(&openings, x, y);
        let iris = (1.0 - catch) * (1.0 - lid(c, s, depth.map(|d| d.d), rho, down));
        let w = m * (1.0 - smoothstep(DISC.0, DISC.1, rho)) as f32 * opening::fade(depth, FADE, FADE_LOWER) * iris as f32;
        let inside = smoothstep(c.pupil + DETAIL_IN.0, c.pupil + DETAIL_IN.1, rho) * (1.0 - smoothstep(DETAIL_OUT.0, DETAIL_OUT.1, rho));
        if let (Some(a), Some(b), Some(t)) = (region.get_mut(j), change.get_mut(j), at.get_mut(j)) {
            *a = w;
            *b = w * inside as f32;
            *t = (rho, down, ci);
        }
    }
    let bands = [SIGMAS.0, SIGMAS.1, SIGMAS.2].map(|s| weighted_blur(&lum, &region, patch.w, patch.h, s * r_mean));
    // each band's detail, as a share of its surroundings
    let detail = |j: usize| -> (f32, f32) {
        let (Some(&b0), Some(&b1), Some(&b2)) = (bands[0].get(j), bands[1].get(j), bands[2].get(j)) else { return (0.0, 0.0) };
        (if b1 > 1e-6 { (b0 - b1) / b1 } else { 0.0 }, if b2 > 1e-6 { (b1 - b2) / b2 } else { 0.0 })
    };
    // each iris's own spread of detail, over the pixels the change reaches: a dark iris's fibres are as real as a
    // light one's but smaller, so its pattern is raised toward the same spread
    let mut sums = vec![[0.0f64; 3]; circles.len()];
    for j in 0..n {
        let (Some(&w), Some(&(_, _, ci))) = (change.get(j), at.get(j)) else { continue };
        let (f, c) = detail(j);
        if let Some(t) = sums.get_mut(ci)
            && w > 0.0
            && f.is_finite()
            && c.is_finite()
        {
            *t = [t[0] + f64::from(w), t[1] + f64::from(w * f.abs()), t[2] + f64::from(w * c.abs())];
        }
    }
    let lifts: Vec<(f32, f32, f32, f32)> = sums
        .iter()
        .map(|t| {
            let spread = |v: f64| if t[0] > 0.0 { (v / t[0]) as f32 } else { 0.0 };
            let (sf, sc) = (spread(t[1]), spread(t[2]));
            let boost = |sp: f32, target: f32| if sp > 1e-6 { (target / sp).clamp(1.0, MAX_BOOST) } else { 1.0 };
            (boost(sf, SPREAD.0), boost(sc, SPREAD.1), (CORE_SHARE * sf).min(CORE), (CORE_SHARE * sc).min(CORE))
        })
        .collect();
    let sat = 1.0 + SATURATION * k;
    for j in 0..n {
        let (Some(&w), Some(&l), Some(&(rho, down, ci))) = (change.get(j), lum.get(j), at.get(j)) else { continue };
        if w <= 0.0 || l <= 0.0 {
            continue;
        }
        let Some(c) = circles.get(ci) else { continue };
        let Some(&(boost_f, boost_c, core_f, core_c)) = lifts.get(ci) else { continue };
        let core = |d: f32, floor: f32, boost: f32| (d.signum() * (d.abs() - floor).max(0.0) * boost).clamp(DETAIL.0, DETAIL.1);
        let (f, cl) = detail(j);
        let (fine_d, clar_d) = (core(f, core_f, boost_f), core(cl, core_c, boost_c));
        let lower = UPPER + (1.0 - UPPER) * smoothstep(-0.5, 0.45, down) as f32;
        let ring = (smoothstep(c.pupil + LIGHT_IN.0, c.pupil + LIGHT_IN.1, rho) * (1.0 - smoothstep(LIGHT_OUT.0, LIGHT_OUT.1, rho))) as f32;
        let pattern = limit((1.0 + FINE * k * fine_d) * (1.0 + CLARITY * k * clar_d));
        let gain = (k * light(c) * lower * ring).exp2() * pattern;
        if !(gain.is_finite() && gain > 0.0) {
            continue;
        }
        let Some(p) = img.data.get_mut(patch.image_index(j, img.width)) else { continue };
        let lit = p.map(|v| v * gain);
        let ly = luma(lit);
        let sat = 1.0 + (sat - 1.0) * ring;
        let out: [f32; 3] = std::array::from_fn(|i| {
            let col = (ly + (lit[i] - ly) * sat).max(0.0);
            p[i] + w * (col - p[i])
        });
        if out.iter().all(|v| v.is_finite()) {
            *p = out;
        }
    }
}

/// The pattern's factor rolled off softly toward [`DETAIL_GAIN`] (so a stronger slider still does a little more,
/// never past it).
fn limit(x: f32) -> f32 {
    let (lo, hi) = DETAIL_GAIN;
    if x >= 1.0 { 1.0 + (hi - 1.0) * ((x - 1.0) / (hi - 1.0)).tanh() } else { 1.0 - (1.0 - lo) * ((1.0 - x) / (1.0 - lo)).tanh() }
}

/// The brightening at 100 for iris `c`, in stops: less for a light iris, and for one with no colour.
fn light(c: &Circle) -> f32 {
    let base = LIGHT.0 + (LIGHT.1 - LIGHT.0) * smoothstep(LIGHT_LEVELS.0, LIGHT_LEVELS.1, c.level as f64) as f32;
    let chroma = c.colour.iter().fold(f32::MIN, |a, v| a.max(*v)) - c.colour.iter().fold(f32::MAX, |a, v| a.min(*v));
    base * (GREY_IRIS.2 + (1.0 - GREY_IRIS.2) * smoothstep(GREY_IRIS.0, GREY_IRIS.1, chroma as f64) as f32)
}

/// How surely a pixel of colour `p` (smoothed) is a lid rather than iris `c`, at `depth` half eye widths inside the
/// opening and (`rho`, `down`) from the iris's centre (in its radii). A lid's wet rim: near the outline or low in the
/// iris (a squinting eye's lower lid can lie well inside the mesh's outline), much brighter than the iris and of
/// another colour, or brighter still. A lid's shadow (or a lash, or the limbal ring): outside the pupil, much darker
/// than the iris.
fn lid(c: &Circle, p: [f32; 3], depth: Option<f64>, rho: f64, down: f64) -> f64 {
    let low = smoothstep(LOW_RIM.0, LOW_RIM.1, down);
    let near = depth.map_or(0.0, |d| 1.0 - smoothstep(RIM_ZONE.0, RIM_ZONE.1, d)).max(low);
    let l = luma(p);
    if l <= 1e-6 {
        return 0.0;
    }
    let ratio = (l / c.level) as f64;
    let off = (0..3).map(|i| (p[i] / l - c.colour[i]).powi(2)).sum::<f32>().sqrt();
    let wet = (smoothstep(RIM_BRIGHT.0, RIM_BRIGHT.1, ratio) * smoothstep(RIM_COLOUR.0, RIM_COLOUR.1, off as f64)).max(smoothstep(
        RIM_BRIGHTER.0,
        RIM_BRIGHTER.1,
        ratio,
    ));
    let shadow = (1.0 - smoothstep(LID_SHADOW.0, LID_SHADOW.1, ratio)) * smoothstep(c.pupil, c.pupil + 0.1, rho);
    (near * wet).max(shadow)
}

/// Eye `e`'s iris: from the face mesh, else from the Iris mask; with its pupil and level read from the picture.
fn circle(img: &Rgb32f, e: &Eye, mask_at: &dyn Fn(f64, f64) -> f32, openings: &[opening::Opening]) -> Option<Circle> {
    let (c, r) = match e.iris {
        Some(c) => c,
        None => from_mask(e, mask_at)?,
    };
    if !(r.is_finite() && r > 0.5 && c.0.is_finite() && c.1.is_finite()) {
        return None;
    }
    // the profile out from the centre, inside the opening and the mask: per radius, the median
    let mut rings: Vec<Vec<f32>> = vec![Vec::new(); PROFILE.0];
    let mut all = Vec::new();
    let mut colours: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (i, ring) in rings.iter_mut().enumerate() {
        let rho = (i as f64 + 0.5) / PROFILE.0 as f64;
        for t in 0..PROFILE.1 {
            let th = t as f64 / PROFILE.1 as f64 * std::f64::consts::TAU;
            let (x, y) = (c.0 + rho * r * th.cos(), c.1 + rho * r * th.sin());
            if mask_at(x, y) < 0.5 || opening::fade(opening::depth(openings, x, y), FADE, FADE_LOWER) < 0.5 {
                continue;
            }
            let Some(p) = sample(img, x, y).filter(|p| p.iter().all(|v| v.is_finite())) else { continue };
            let v = luma(p);
            ring.push(v);
            if (0.45..0.85).contains(&rho) {
                all.push(v);
                if v > 1e-6 {
                    for (c, list) in colours.iter_mut().enumerate() {
                        list.push(p[c] / v);
                    }
                }
            }
        }
    }
    let level = quantile(&mut all, 0.5).filter(|l| *l > 1e-6)?;
    let medians: Vec<Option<f32>> = rings.iter_mut().map(|v| quantile(v, 0.5)).collect();
    let inner = medians.iter().enumerate().filter(|(i, _)| ((*i as f64 + 0.5) / PROFILE.0 as f64) < 0.3).filter_map(|(_, m)| *m);
    let dark = inner.fold(f32::INFINITY, f32::min);
    let pupil = if dark.is_finite() && level > PUPIL_SHOWS * dark {
        let half_way = dark + 0.5 * (level - dark);
        medians
            .iter()
            .enumerate()
            .find(|(i, m)| ((*i as f64 + 0.5) / PROFILE.0 as f64) >= 0.12 && m.is_some_and(|m| m > half_way))
            .map_or(PUPIL_DEFAULT, |(i, _)| (i as f64 + 0.5) / PROFILE.0 as f64)
            .clamp(PUPIL_RANGE.0, PUPIL_RANGE.1)
    } else {
        PUPIL_DEFAULT
    };
    let colour = colours.map(|mut v| quantile(&mut v, 0.5).unwrap_or(1.0));
    Some(Circle { c, r, pupil, level, colour, down: e.down })
}

/// An iris circle from the Iris mask around eye `e` (no face mesh): its centre of mass and half its width across.
fn from_mask(e: &Eye, mask_at: &dyn Fn(f64, f64) -> f32) -> Option<((f64, f64), f64)> {
    const GRID: (usize, usize) = (48, 32);
    let (mut sw, mut sx, mut sy) = (0.0, 0.0, 0.0);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for j in 0..GRID.1 {
        for i in 0..GRID.0 {
            let a = -1.2 + 2.4 * (i as f64 + 0.5) / GRID.0 as f64;
            let b = -0.8 + 1.6 * (j as f64 + 0.5) / GRID.1 as f64;
            let (x, y) = e.to_px(a, b);
            let m = mask_at(x, y) as f64;
            if m < 0.5 {
                continue;
            }
            sw += m;
            sx += m * x;
            sy += m * y;
            lo = lo.min(a);
            hi = hi.max(a);
        }
    }
    (sw > 0.0 && hi > lo).then(|| ((sx / sw, sy / sw), (hi - lo) / 2.0 * e.half))
}
