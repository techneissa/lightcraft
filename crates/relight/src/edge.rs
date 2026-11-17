//! The sun past an edge (issue #332, `fork/relight.md`): a light just behind a building's corner, a wall, a trunk or a
//! shoulder. Light spills round the edge and glows there, beams fan out from that point through the haze (the beam code
//! does that, [`crate::beam`]) and the edge's own shadow cuts them (the shadow code does that, [`crate::shadow`]); this
//! module finds the edge and draws the glow.
//!
//! - **Snap** ([`Field::snap_to_edge`]): the nearest depth step to a point the user dragged the light to, and the place just
//!   behind it: on the step, at the depth of its far side. The step test is the Rim light's idea ([`crate::rim`]): a pixel
//!   with something clearly farther a few pixels away. A [`Field::sun_towards`] gives the direction a Sun must shine from to
//!   come from that point, so its shafts fan out from there.
//! - **Glow** ([`Field::edge_glow`]): light added on top, in the light's own colour, brightest at the light's place and
//!   falling off round it. The open side of the edge (the far side) gets all of it, like the glare of a bright thing seen
//!   past the edge; the near side (what hides the light) only what wraps over the edge, a few pixels deep. An optional
//!   sunburst adds a few faint rays on the open side. It is a plane the finish stage adds beside the beam's
//!   ([`crate::Field::beam_plane`]), so it shows over black and takes no colour from the pixel, on the CPU and the GPU alike.

use rayon::prelude::*;

use crate::effects::Effects;
use crate::field::Field;
use crate::light::{Light, Lobe};

/// How much farther (log depth) something must lie than a pixel, over what the depth was already doing on the way to the pixel
/// (a slope is no edge), for the pixel to be on an edge (about 6 % farther: a depth model puts a building against the sky only
/// that far behind it).
const STEP: f32 = 0.06;
/// How far from the dragged point an edge is looked for, as a share of the field's longer side.
const SEARCH: f32 = 0.15;
/// How far behind the far side's own depth the light is put (a share of the light's depth span).
const BEHIND: f64 = 0.02;
/// How far from the pixel the far side is looked for, as a share of the field's longer side.
const SPAN: f32 = 0.006;

/// Where a light dragged to an edge goes: its place (normalized photo coordinates) and depth (0..1 of a light's span).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSnap {
    pub x: f64,
    pub y: f64,
    pub depth: f64,
}

/// The step across the depth at pixel `(px, py)`, the way an edge is told from a slope: the farthest-reaching of the four ways `k`
/// pixels on, with how much farther that is than the pixel (less the slope the depth had coming in from the other side, so a
/// ramp has none), which way it lies and its depth. `None` when no way is a step of [`STEP`] or more. `z` reads the depth.
fn step_at(z: &dyn Fn(isize, isize) -> Option<f32>, px: isize, py: isize, k: isize) -> Option<(f32, isize, isize, f32)> {
    let z0 = z(px, py)?;
    let mut top: Option<(f32, isize, isize, f32)> = None;
    for (dx, dy) in [(k, 0), (-k, 0), (0, k), (0, -k)] {
        let slope = z(px - dx, py - dy).map_or(0.0, |zb| z0 - zb).max(0.0);
        if let Some(z1) = z(px + dx, py + dy)
            && z1 - z0 >= STEP
            && z1 - z0 - slope >= STEP
            && top.is_none_or(|t| z1 - z0 > t.0)
        {
            top = Some((z1 - z0, dx, dy, z1));
        }
    }
    top
}

impl Field {
    /// The nearest depth edge to `(x, y)` (normalized photo coordinates) within [`SEARCH`] of it that is a real outline (not a
    /// small step beside a big one), and the place just behind it:
    /// on the step, at its far side's depth. `None` when no edge is near (the light stays where it was put).
    pub fn snap_to_edge(&self, x: f64, y: f64) -> Option<EdgeSnap> {
        let (w, h) = (self.width, self.height);
        if !(x.is_finite() && y.is_finite()) || w < 8 || h < 8 {
            return None;
        }
        let long = w.max(h) as f32;
        let k = ((SPAN * long).round() as isize).max(2);
        let reach = ((SEARCH * long) as isize).max(k);
        let (wi, hi) = (w as isize, h as isize);
        let (cx, cy) = (((x.clamp(0.0, 1.0) * w as f64) as isize).clamp(0, wi - 1), ((y.clamp(0.0, 1.0) * h as f64) as isize).clamp(0, hi - 1));
        let z = |px: isize, py: isize| -> Option<f32> {
            if px < 0 || py < 0 || px >= wi || py >= hi {
                return None;
            }
            self.ln_z.get(py as usize * w + px as usize).copied().filter(|v| v.is_finite())
        };
        // (the pixels with a clearly farther one `k` pixels away: where, how far from the point, how big the step, which way the far
        // side lies, its depth)
        let mut found: Vec<(isize, isize, isize, f32, isize, isize, f32)> = Vec::new();
        for py in (cy - reach).max(0)..=(cy + reach).min(hi - 1) {
            for px in (cx - reach).max(0)..=(cx + reach).min(wi - 1) {
                let d2 = (px - cx) * (px - cx) + (py - cy) * (py - cy);
                if d2 > reach * reach {
                    continue;
                }
                if let Some((step, dx, dy, far)) = step_at(&z, px, py, k) {
                    found.push((d2, px, py, step, dx, dy, far));
                }
            }
        }
        // (the nearest of the real outlines: a step a quarter as big as the biggest round the point is no outline, an arm's edge
        // on a shirt is not what the light goes behind)
        let biggest = found.iter().map(|f| f.3).fold(0f32, f32::max);
        let (_, px, py, _, dx, dy, far) =
            found.into_iter().filter(|f| f.3 >= 0.25 * biggest).min_by(|a, b| a.0.cmp(&b.0).then(b.3.total_cmp(&a.3)))?;
        // (walk from the pixel towards the far side to where the depth is half way across: the step itself)
        let z0 = z(px, py)?;
        let (sx, sy) = (dx.signum(), dy.signum());
        let t = (0..=k).find(|t| z(px + sx * t, py + sy * t).is_some_and(|v| v >= z0 + 0.5 * (far - z0))).unwrap_or(k / 2);
        let (ex, ey) = ((px + sx * t) as f64 + 0.5, (py + sy * t) as f64 + 0.5);
        let (depth, _) = self.light_depth_of(far);
        Some(EdgeSnap { x: ex / w as f64, y: ey / h as f64, depth: (depth + BEHIND).clamp(0.0, 1.0) })
    }

    /// The azimuth and elevation (degrees, as [`crate::Sun`] takes them) a sun must shine from to come from the photo's point
    /// `(x, y)`, behind the scene along the camera's line of sight through it. The elevation is 0 for a point under the
    /// middle of the photo (a sun does not shine from below the horizon).
    pub fn sun_towards(&self, x: f64, y: f64) -> (f64, f64) {
        let (kx, ky) = self.lens();
        let (u, v) = (if x.is_finite() { x as f32 } else { 0.5 }, if y.is_finite() { y as f32 } else { 0.5 });
        let d = [(u - 0.5) * kx, (v - 0.5) * ky, 1.0];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
        let (dx, dy, dz) = (d[0] / len, d[1] / len, d[2] / len);
        let elevation = f64::from((-dy).clamp(-1.0, 1.0).asin().to_degrees()).clamp(0.0, 90.0);
        let azimuth = f64::from(dx.atan2(-dz).to_degrees());
        (azimuth, elevation)
    }
}

/// The glow's brightest, in units of 18 % grey per unit of the light's strength, at full Edge Glow.
const GLOW_PEAK: f32 = 5.5;
/// The sunburst's brightest, in the same units, at full Sunburst.
const BURST_PEAK: f32 = 2.2;
/// The core's radius at Softness 0 and what Softness adds, as shares of the field's longer side (a small, very bright core).
const CORE_MIN: f32 = 0.005;
const CORE_SOFT: f32 = 0.03;
/// The halo's size at Softness 0 and what Softness adds (a soft round glow in the air round the core).
const HALO_MIN: f32 = 0.06;
const HALO_SOFT: f32 = 0.08;
/// How much of the core and halo the halo keeps (the rest is the core).
const HALO: f32 = 0.22;
/// How much of the glow the near side of the edge (what hides the light) keeps away from the outline: it is backlit.
const NEAR_SIDE: f32 = 0.2;
/// How bright the thin rim along the outline is, against the core: it follows the outline near the light and fades along it.
const RIM: f32 = 1.1;
/// How far the rim follows the outline from the light, in core radii (it fades smoothly along it, to a tenth in about seven).
const RIM_REACH: f32 = 3.0;
/// A pixel is nearer than the edge by most of its step, and so in front of the light, when it lies within this share of the step
/// of the nearest thing round the light.
const COVER: f32 = 0.35;
/// How far a ray reaches, as a share of the field's longer side.
const RAY_REACH: f32 = 0.2;
/// How far from the light anything of the glow reaches (a share of the longer side): the halo is gone by then.
const REACH: f32 = 0.45;

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn unit(v: f64) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { 0.0 }
}

/// The rays' brightness at angle `a` round the light: a few soft lobes, unevenly spread (never the regular star of a hard
/// filter), 0..1.
fn rays(a: f32) -> f32 {
    let lobe = |n: f32, phase: f32| (0.5 + 0.5 * (n * a + phase).cos()).powi(4);
    0.6 * lobe(5.0, 0.4) + 0.4 * lobe(3.0, 2.1)
}

impl Field {
    /// The glow past an edge of `light` with effects `e` (Edge Glow and Sunburst), at the field's size, in units of 18 % grey
    /// per unit of the light's strength ([`crate::Rig::gain`]): a small very bright core at the light's place, a soft round halo
    /// round it that falls off the same every way (the glow is the glare of the light, not shaped by the photo's depth), a thin
    /// bright rim along the near side of the outline that fades along it away from the light, and with Sunburst a few faint rays.
    /// What hides the light is each pixel's own depth: a pixel nearer than the edge the light sits behind keeps only [`NEAR_SIDE`] of
    /// the halo (it is backlit) and none of the core or rays (the light peeks past the edge), and the open air beside and below
    /// it, outside the thing's pixels, keeps all of it. Without a depth step right at the light nothing is hidden. `None` for a light with no place
    /// or with neither number on. It depends on the light's place, depth and Softness, and on `e`'s two numbers; not on its
    /// Amount, Warmth or anything of the haze.
    pub fn edge_glow(&self, light: &Light, e: &Effects) -> Option<Lobe> {
        let (glow, burst) = (unit(e.edge_glow), unit(e.sunburst));
        if !light.kind.has_place() || (glow <= 0.0 && burst <= 0.0) || !(light.x.is_finite() && light.y.is_finite()) {
            return None;
        }
        let (w, h) = (self.width, self.height);
        let n = w.checked_mul(h)?;
        if n == 0 {
            return None;
        }
        let long = w.max(h) as f32;
        let soft = unit(light.softness);
        let (r0, halo) = (CORE_MIN + CORE_SOFT * soft, HALO_MIN + HALO_SOFT * soft);
        let (sx, sy) = (light.x.clamp(-0.5, 1.5) as f32 * w as f32, light.y.clamp(-0.5, 1.5) as f32 * h as f32);
        let (wi, hi) = (w as isize, h as isize);
        let k = ((SPAN * long).round() as isize).max(2);
        let z = |px: isize, py: isize| -> Option<f32> {
            if px < 0 || py < 0 || px >= wi || py >= hi {
                return None;
            }
            self.ln_z.get(py as usize * w + px as usize).copied().filter(|v| v.is_finite())
        };
        // (the biggest step right round the light: the edge it sits behind, if any)
        let (cx, cy) = ((sx as isize).clamp(0, wi - 1), (sy as isize).clamp(0, hi - 1));
        let mut biggest = 0f32;
        for y in (cy - 3 * k).max(0)..=(cy + 3 * k).min(hi - 1) {
            for x in (cx - 3 * k).max(0)..=(cx + 3 * k).min(wi - 1) {
                if let Some((step, ..)) = step_at(&z, x, y, k) {
                    biggest = biggest.max(step);
                }
            }
        }
        let has_edge = biggest > 0.0;
        // (what hides the light: the nearest thing right round it, and how far behind it lies the far side)
        let mut near = f32::MAX;
        for y in (cy - 3 * k).max(0)..=(cy + 3 * k).min(hi - 1) {
            for x in (cx - 3 * k).max(0)..=(cx + 3 * k).min(wi - 1) {
                near = near.min(z(x, y).unwrap_or(f32::MAX));
            }
        }
        let reach = (REACH * long) as isize;
        // (the thing in front of the light: the pixels nearer than the edge by most of its step that join, without a jump in depth, the
        // ones right round the light. A far side whose depth runs on from the thing's own, away from the edge, to farther depth is
        // behind the light, not in front of it, and is not part of it; each pixel is marked once, when it is pushed, and the fill
        // stays inside the glow's reach)
        let limit = near + COVER * biggest;
        let mut cover = vec![false; n];
        if has_edge {
            let jump = 0.25 * biggest;
            let mut stack: Vec<(isize, isize)> = Vec::new();
            for y in (cy - 3 * k).max(0)..=(cy + 3 * k).min(hi - 1) {
                for x in (cx - 3 * k).max(0)..=(cx + 3 * k).min(wi - 1) {
                    if let Some(c) = cover.get_mut(y as usize * w + x as usize)
                        && z(x, y).is_some_and(|v| v <= limit)
                    {
                        *c = true;
                        stack.push((x, y));
                    }
                }
            }
            while let Some((x, y)) = stack.pop() {
                let Some(here) = z(x, y) else { continue };
                for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                    if (nx - cx).abs() > reach || (ny - cy).abs() > reach {
                        continue;
                    }
                    if let Some(c) = cover.get_mut(ny.max(0) as usize * w + nx.max(0) as usize)
                        && !*c
                        && z(nx, ny).is_some_and(|v| v <= limit && (v - here).abs() <= jump)
                    {
                        *c = true;
                        stack.push((nx, ny));
                    }
                }
            }
        }
        let mut m = vec![0f32; n];
        m.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            let dy = (y as f32 + 0.5 - sy) / long;
            if dy.abs() * long > reach as f32 {
                return;
            }
            for (x, out) in row.iter_mut().enumerate() {
                let dx = (x as f32 + 0.5 - sx) / long;
                let d = (dx * dx + dy * dy).sqrt();
                if d * long > reach as f32 {
                    continue;
                }
                // (what hides the light is each pixel's own depth: a pixel nearer than the edge, by most of its step, is the thing in front
                // of the light, however it lies round the light; the sky beside and below a roof's apex is open air, not a line's far side)
                let seen = if cover.get(y * w + x).copied().unwrap_or(false) {
                    z(x as isize, y as isize).map_or(1.0, |zp| smooth(near + 0.1 * biggest, limit, zp))
                } else {
                    1.0
                };
                // (the halo, soft and round, a share of itself where something covers it: backlit)
                let halo_here = HALO * (-d / halo).exp() * (NEAR_SIDE + (1.0 - NEAR_SIDE) * seen);
                // (the core and the rays only where nothing nearer than the edge covers them: they peek past it)
                let core = 1.0 / (1.0 + (d / r0) * (d / r0));
                let mut v = glow * GLOW_PEAK * (core * seen + halo_here);
                // (the thin rim: the near side of a step as big as the one the light sits behind, a few pixels deep, only where the outline
                // faces the light, and strongest by it, fading within a short way along the outline; the smaller steps of a surface get none)
                if has_edge && let Some((step, sdx, sdy, far)) = step_at(&z, x as isize, y as isize, k) {
                    let (xi, yi) = (x as isize, y as isize);
                    let grad = |a: Option<f32>, b: Option<f32>, fallback: isize| match (a, b) {
                        (Some(a), Some(b)) => b - a,
                        _ => fallback as f32,
                    };
                    let (gx, gy) = (grad(z(xi - k, yi), z(xi + k, yi), sdx), grad(z(xi, yi - k), z(xi, yi + k), sdy));
                    let gl = (gx * gx + gy * gy).sqrt();
                    let facing = if gl > 1e-6 && d > 1e-6 { smooth(0.2, 0.8, -(gx * dx + gy * dy) / (gl * d)) } else { 0.0 };
                    // (the depth half way across the step: the pixels from there in, down to the outline itself, are the rim, brightest at it)
                    let z0 = z(xi, yi).unwrap_or(0.0);
                    let (ux, uy) = (sdx.signum(), sdy.signum());
                    let reached = (1..=k).find(|j| z(xi + ux * j, yi + uy * j).is_some_and(|v| v >= z0 + 0.5 * (far - z0)));
                    let band = reached.map_or(0.15, |j| (1.0 - (j - 1) as f32 / k as f32).powi(2));
                    let along = (-d / (RIM_REACH * r0)).exp();
                    v += glow * GLOW_PEAK * RIM * along * facing * band * smooth(0.6 * biggest, 0.95 * biggest, step);
                }
                if burst > 0.0 {
                    let fade = r0 / (d + r0) * (-d / RAY_REACH).exp();
                    v += burst * BURST_PEAK * rays(dy.atan2(dx)) * fade * seen;
                }
                *out = if v.is_finite() { v.max(0.0) } else { 0.0 };
            }
        });
        Some(Lobe { m })
    }
}
