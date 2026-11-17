//! The Rim light (issues #300, #319): a thin bright edge along the subject's outline.
//!
//! The cue is the depth edge, not the surface normals (which are noisy inside a face and made pale patches on a
//! forehead and a cheek). A pixel is lit by how far a *farther* surface lies from it, across a step in depth: the
//! far side of a step, and everything that is not the near side of one, gets nothing. That is a grey-scale dilation of
//! the log depth by a cone, `M(p) = max over q of (z(q) − slope · |q − p|)`, so a step must be taller the farther it is
//! from the pixel to count: the rim is the share of `M(p) − z(p)` between [`STEP_MIN`] and [`STEP_FULL`]. It fades
//! smoothly inward over [`Shape::Rim`](crate::kinds::Shape)'s reach (Rim Width), is whole at the outline and follows
//! the depth's edge, which is the hair's and the ear's. Two passes over the field make it, whatever the reach.
//!
//! The surface's turning away from the camera and the side the light comes from only weigh that: the turning-away
//! never brings a rim where there is no step, and the side (a direction in the photo's plane and how one-sided the
//! light is) takes the rim off the outline facing away from it.

use rayon::prelude::*;

use crate::field::Field;

/// The step in log depth (a farther surface than the pixel's) under which there is no rim, and above which it is whole
/// at the pixel itself (about 16 % and 57 % farther).
const STEP_MIN: f32 = 0.15;
const STEP_FULL: f32 = 0.45;
/// The largest step that is seen: a farther surface counts as no more than this behind the nearest thing round it, so a
/// deep backdrop makes a rim no wider than a near wall does.
const STEP_CAP: f32 = 0.5;
const SQRT2: f32 = std::f32::consts::SQRT_2;

/// How the Rim light is set up on a field (all numbers already in range).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Setup {
    /// How far the rim reaches inward, as a share of the field's longer side.
    pub reach: f32,
    /// The cosine to the camera under which a surface is wholly turned away (over it the normals weigh the rim down).
    pub turned: f32,
    /// The direction the light comes from in the photo's plane (x right, y down; a unit vector) and how one-sided it
    /// is, 0 (all round the outline) .. 1 (only the side that faces it).
    pub from: [f32; 2],
    pub one_sided: f32,
}

/// A Rim light on a field: its setup, and the rim's value at every pixel, made the first time a pixel is asked for (a rig
/// that is only set up, as every drag step of another light does, costs nothing; the lobe made from it is what is kept).
#[derive(Debug)]
pub(crate) struct RimShape {
    setup: Setup,
    map: std::sync::OnceLock<RimMap>,
}

impl RimShape {
    pub(crate) fn new(setup: Setup) -> Self {
        RimShape { setup, map: std::sync::OnceLock::new() }
    }

    /// The rim over `field` (the field it was set up on), made now if it was not yet.
    pub(crate) fn map(&self, field: &Field) -> &RimMap {
        self.map.get_or_init(|| build(field, &self.setup))
    }
}

/// The rim's value at every pixel of a field (0..1).
pub(crate) struct RimMap {
    width: usize,
    height: usize,
    v: Vec<f32>,
}

impl std::fmt::Debug for RimMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RimMap({} × {})", self.width, self.height)
    }
}

impl RimMap {
    /// The rim at normalized photo coordinates `(u, v)` (bilinear).
    #[inline]
    pub(crate) fn at(&self, u: f32, v: f32) -> f32 {
        crate::filter::bilinear(&self.v, self.width, self.height, u * self.width as f32, v * self.height as f32)
    }
}

/// 0 below `e0`, 1 above `e1`, smooth between (a step when they meet).
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 - e0 < 1e-6 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `m` (`w × h`) dilated by the cone `z − slope · distance`: every value is the highest of its neighbours' less the slope
/// times how far they are (a two-pass chamfer, a few per cent off round).
fn dilate(m: &mut [f32], w: usize, h: usize, slope: f32) {
    let (a, d) = (slope, slope * SQRT2);
    let take = |m: &mut [f32], i: usize, j: Option<usize>, cost: f32| {
        let Some(j) = j else { return };
        let (Some(&from), Some(&here)) = (m.get(j), m.get(i)) else { return };
        let v = from - cost;
        if v > here
            && let Some(slot) = m.get_mut(i)
        {
            *slot = v;
        }
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            take(m, i, x.checked_sub(1).map(|x| y * w + x), a);
            if let Some(yy) = y.checked_sub(1) {
                take(m, i, x.checked_sub(1).map(|x| yy * w + x), d);
                take(m, i, Some(yy * w + x), a);
                take(m, i, (x + 1 < w).then(|| yy * w + x + 1), d);
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            take(m, i, (x + 1 < w).then(|| y * w + x + 1), a);
            if y + 1 < h {
                let yy = y + 1;
                take(m, i, (x + 1 < w).then(|| yy * w + x + 1), d);
                take(m, i, Some(yy * w + x), a);
                take(m, i, x.checked_sub(1).map(|x| yy * w + x), d);
            }
        }
    }
}

/// The Rim light's value over `field` for `setup`.
pub(crate) fn build(field: &Field, setup: &Setup) -> RimMap {
    let (w, h) = (field.width, field.height);
    let n = w * h;
    let long = w.max(h).max(1) as f32;
    let reach_px = (setup.reach * long).max(1.0);
    // the cone is as steep as a whole step's rim (STEP_CAP down to STEP_MIN) fades out over the reach
    let slope = (STEP_CAP - STEP_MIN) / reach_px;
    let z: Vec<f32> = (0..n).map(|i| field.at_px(i).0).map(|lz| if lz.is_finite() { lz } else { 0.0 }).collect();
    // the nearest thing round each pixel (its depth plus the cone), the far side read no more than STEP_CAP behind it
    let mut near: Vec<f32> = z.iter().map(|v| -v).collect();
    dilate(&mut near, w, h, slope);
    let mut m: Vec<f32> = z.iter().zip(&near).map(|(v, n)| v.min(STEP_CAP - n)).collect();
    dilate(&mut m, w, h, slope);
    let [fx, fy] = setup.from;
    let mut v = vec![0f32; n];
    v.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        let at = |x: usize, y: usize| m.get(y.min(h.saturating_sub(1)) * w + x.min(w.saturating_sub(1))).copied().unwrap_or(0.0);
        for (x, out) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (Some(&zi), Some(&mi)) = (z.get(i), m.get(i)) else { continue };
            let edge = smooth(STEP_MIN, STEP_FULL, mi - zi);
            if edge <= 0.0 {
                continue;
            }
            // the surface's turn from the camera only weighs the rim (1 face on .. 0 edge on)
            let (lz, nrm, _) = field.at_px(i);
            let p = field.point((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, lz);
            let pl = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-9);
            let c = -(nrm[0] * p[0] + nrm[1] * p[1] + nrm[2] * p[2]) / pl;
            let away = 1.0 - smooth(0.05, setup.turned, c);
            // (squared: a bright edge that falls away quickly, not a band; the turn from the camera takes up to half of it)
            let mut value = edge * edge * (0.5 + 0.5 * away);
            if setup.one_sided > 0.0 {
                // the outline's outward direction: where the dilation climbs (towards the far side of the step)
                let (gx, gy) = (at(x + 2, y) - at(x.saturating_sub(2), y), at(x, y + 2) - at(x, y.saturating_sub(2)));
                let gl = (gx * gx + gy * gy).sqrt();
                // (no direction to the far side, no side it faces: nothing)
                let toward = if gl > 1e-6 { (gx * fx + gy * fy) / gl } else { 0.0 };
                let side = smooth(0.0, 0.8, toward);
                value *= 1.0 - setup.one_sided * (1.0 - side);
            }
            *out = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        }
    });
    RimMap { width: w, height: h, v }
}
