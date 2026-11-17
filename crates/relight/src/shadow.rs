//! Shadows (issue #315, `fork/relight.md`): a light is blocked by what the photo shows. For each pixel's surface point
//! the light's rays are walked through the depth map towards the light (a ray towards the light's place for a point, a
//! spot and a softbox, parallel rays along the direction for a sun); a ray is blocked where it runs behind a nearer
//! surface of the photo, within the thickness a thing is taken to have ([`THICK`]), as in a screen-space shadow march.
//! The light's size is spread over several rays aimed across it (a disc for a point, a spot and a sun, the rectangle for
//! a softbox), so a small light gives a hard shadow and a big one a soft shadow whose penumbra grows with the distance
//! from the blocker, as it does in the world. A Rim and a Fill take no shadow.
//!
//! The march is made on a reduced copy of the field (about 400 pixels on the long side: [`Reduced`], made once per
//! field), as a [`Visibility`] (1 lit .. 0 blocked) that depends on the light's place, depth, size and type numbers and
//! not on its Amount or the shadow amount, and so is kept by the caller until the light moves. [`Field::shadowed`] brings it to
//! the field's size with the field's own edge-aware filter (a joint bilateral upsample guided by the photo's
//! brightness and its depth, so a shadow stops at a near thing's outline) and multiplies it into the light's lobe, before
//! the lights add up.
//!
//! **Drag quality:** a drag's draft render asks for [`Field::visibility`] with `draft`: a grid of about 200 pixels on the long side
//! (1/8 of a 1600-pixel field), the first four rays only (no penumbra pass) and, in [`Field::shadowed`], the nearest sample
//! instead of the filter. The full one is made at the release and for every export.

use rayon::prelude::*;

use crate::field::{Field, LENS};
use crate::kinds::{LightKind, Shape};
use crate::light::{Light, Lobe, Rig};

/// The long side of the reduced field the march runs on (the reduction is the whole number of field pixels that brings
/// it closest to this from above: 4 for the usual 1600-pixel field, 1 for a small one), for a drag's draft and for the full march.
const LOW_SIDE_DRAFT: usize = 200;
const LOW_SIDE: usize = 400;
/// Steps along each ray.
const STEPS: usize = 24;
/// Rays of the first pass (one at the light's middle and three round its edge); a pixel whose rays disagree (it is in
/// a penumbra) gets [`MORE`] more across the whole light.
const FIRST: usize = 4;
const MORE: usize = 12;
/// A ray point is behind a surface when its depth is more than this ratio of the surface's (a little over: the
/// surface it starts on, and the depth model's small ripples, don't shadow it) ...
const BIAS: f32 = 1.05;
/// ... and a thing blocks the light only so far behind its visible surface: this ratio of the surface's depth is the
/// thickness a thing is taken to have (about 0.47 in log depth), since the photo shows nothing behind a thing's front.
const THICK: f32 = 1.6;
/// How much of the light's own radius (the sphere its fall-off comes from) a point or a spot's shadow uses.
const POINT_SHARE: f32 = 0.5;
/// The sun's angular radius (radians) at Softness 0 and the most it adds at Softness 1.
const SUN_MIN: f32 = 0.012;
const SUN_SPREAD: f32 = 0.26;
/// Where the weights of the upsampling fall to 1/e: the photo's brightness (cube root of linear light) and log depth.
const SIGMA_LUM: f32 = 0.1;
/// (the depth's is narrow: a step of 5–10 % of depth, a near thing's outline, must not let its shadow leak across)
const SIGMA_DEPTH: f32 = 0.04;

impl LightKind {
    /// Whether a light of this type casts shadows: a point, a spot, a sun and a softbox do, a rim and a fill have no
    /// place the light comes from.
    pub fn takes_shadows(&self) -> bool {
        matches!(self, LightKind::Point | LightKind::Spot(_) | LightKind::Sun(_) | LightKind::Softbox(_))
    }
}

/// The field, reduced for the march: at the centre pixel of each `f × f` block its depth (linear and log) and the
/// photo's brightness. Made once per field.
#[derive(Clone, Debug)]
pub(crate) struct Reduced {
    f: usize,
    w: usize,
    h: usize,
    z: Vec<f32>,
    lz: Vec<f32>,
    gi: Vec<f32>,
}

impl Reduced {
    pub(crate) fn of(field: &Field, low_side: usize) -> Reduced {
        let (fw, fh) = (field.width.max(1), field.height.max(1));
        let f = fw.max(fh).div_ceil(low_side.max(1)).max(1);
        let (w, h) = (fw.div_ceil(f), fh.div_ceil(f));
        let (mut z, mut lz, mut gi) = (Vec::with_capacity(w * h), Vec::with_capacity(w * h), Vec::with_capacity(w * h));
        for y in 0..h {
            let cy = (y * f + f / 2).min(fh - 1);
            for x in 0..w {
                let i = cy * fw + (x * f + f / 2).min(fw - 1);
                let l = field.ln_z.get(i).copied().unwrap_or(0.0);
                lz.push(l);
                z.push(l.exp());
                gi.push(field.gi.get(i).copied().unwrap_or(0.0));
            }
        }
        Reduced { f, w, h, z, lz, gi }
    }

    /// Memory held (bytes).
    pub(crate) fn bytes(&self) -> usize {
        (self.z.len() + self.lz.len() + self.gi.len()) * 4
    }
}

/// How much of a light each pixel of the reduced field sees: 1 all of it, 0 none.
#[derive(Clone, Debug)]
pub struct Visibility {
    w: usize,
    h: usize,
    v: Vec<f32>,
    /// Made for a drag (coarse): [`Field::shadowed`] reads it by the nearest sample.
    draft: bool,
}

impl Visibility {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.v.len() * 4
    }
}

/// Where a light's rays come from, set up on the field.
enum Source {
    /// A round source at a point, `radius` wide (a point, and a spot through its cone).
    Point { pos: [f32; 3], radius: f32 },
    /// Parallel light from direction `to` (a unit vector towards the sun), with an angular radius.
    Sun { to: [f32; 3], spread: f32 },
    /// A rectangle: its middle, its two axes, and its half sides.
    Box { c: [f32; 3], tx: [f32; 3], ty: [f32; 3], hw: f32, hh: f32 },
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l.is_finite() && l > 1e-9 { [v[0] / l, v[1] / l, v[2] / l] } else { fallback }
}

/// Two unit vectors that, with `a` (a unit vector), make a frame.
fn frame(a: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let helper = if a[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let e1 = unit(cross(a, helper), [1.0, 0.0, 0.0]);
    (e1, cross(a, e1))
}

/// A number in 0..1 from a pixel's place and a salt (a fixed noise: the same pixel gets the same rays every time).
fn noise(x: usize, y: usize, salt: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ salt.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / 16_777_216.0
}

/// The golden angle (radians): successive points of a sunflower spread round a disc evenly.
const GOLDEN: f32 = 2.399_963;

/// The `k`th of the rays across the light as a point of the unit disc: the first pass's are the middle and three on
/// the edge's 0.8, the rest a sunflower (`turn`: this pixel's rotation of the pattern).
fn offset(k: usize, turn: f32) -> (f32, f32) {
    if k == 0 {
        return (0.0, 0.0);
    }
    if k < FIRST {
        let a = turn + (k as f32 - 1.0) * std::f32::consts::TAU / 3.0;
        return (0.8 * a.cos(), 0.8 * a.sin());
    }
    let j = (k - FIRST) as f32;
    let (r, a) = (((j + 0.5) / MORE as f32).sqrt(), turn + j * GOLDEN);
    (r * a.cos(), r * a.sin())
}

/// The march's numbers for one reduced field and one light.
struct March<'a> {
    r: &'a Reduced,
    src: Source,
    /// The camera's half-extent per unit depth, on each axis.
    kx: f32,
    ky: f32,
    /// Nothing is nearer than this (the light never sits in front of it, so the ray ends), and nothing farther than this.
    z_cut: f32,
    z_cap: f32,
    /// The sun's frame ⟂ its direction.
    sun_axes: ([f32; 3], [f32; 3]),
    /// The light's own depth (none for a sun, which has no place).
    light_z: Option<f32>,
}

impl March<'_> {
    /// How far along `d` from `p` the ray stays in the picture and between [`March::z_cut`] and [`March::z_cap`].
    fn exit(&self, p: [f32; 3], d: [f32; 3]) -> f32 {
        let mut t = f32::INFINITY;
        for (a, pa, da) in [(0.5 * self.kx, p[0], d[0]), (0.5 * self.ky, p[1], d[1])] {
            for s in [1.0f32, -1.0] {
                // pa + t·da = s·a·(pz + t·dz): the plane of the picture's side
                let den = da - s * a * d[2];
                let num = s * a * p[2] - pa;
                if den.abs() > 1e-9 {
                    let tt = num / den;
                    if tt > 0.0 {
                        t = t.min(tt);
                    }
                }
            }
        }
        if d[2] < -1e-9 {
            t = t.min((p[2] - self.z_cut) / -d[2]);
        } else if d[2] > 1e-9 {
            t = t.min((self.z_cap - p[2]) / d[2]);
        }
        if t.is_finite() { t.max(0.0) } else { 0.0 }
    }

    /// Whether the light itself lies in the band behind a surface of depth `zs` that would block (a light placed a little behind a
    /// near subject): such a surface blocks nothing for it, as the light is taken to be in front of it. Else a light
    /// over its subject's shoulder would shade the whole picture.
    fn light_inside(&self, zs: f32) -> bool {
        self.light_z.is_some_and(|z| z > zs * BIAS && z < zs * THICK)
    }

    /// Whether the ray from `p` along `d` (`d` spans the whole way to the light when `whole`, else it is a direction)
    /// reaches the light: `false` when something blocks it. `jit` (0..1) offsets the steps.
    fn clear(&self, p: [f32; 3], d: [f32; 3], whole: bool, jit: f32) -> bool {
        let r = self.r;
        let mut end = self.exit(p, d);
        if whole {
            end = end.min(1.0);
        }
        if end.is_nan() || end <= 1e-6 {
            return true;
        }
        let (lw, lh) = (r.w as f32, r.h as f32);
        for i in 0..STEPS {
            let t = end * (i as f32 + jit) / STEPS as f32;
            let q = [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
            if q[2].is_nan() || q[2] <= 1e-6 {
                break;
            }
            let iz = 1.0 / q[2];
            let (u, v) = (q[0] * iz / self.kx + 0.5, q[1] * iz / self.ky + 0.5);
            let (ix, iy) = (((u * lw).max(0.0) as usize).min(r.w - 1), ((v * lh).max(0.0) as usize).min(r.h - 1));
            let Some(zs) = r.z.get(iy * r.w + ix) else { continue };
            if q[2] > zs * BIAS && q[2] < zs * THICK && !self.light_inside(*zs) {
                return false;
            }
        }
        true
    }

    /// The ray `k` from `p` (a surface point) towards the light, turned by `turn`; whether it reaches the light.
    fn ray(&self, p: [f32; 3], k: usize, turn: f32, jit: f32) -> bool {
        let (a, b) = offset(k, turn);
        match &self.src {
            Source::Point { pos, radius } => {
                let axis = unit(sub(*pos, p), [0.0, 0.0, -1.0]);
                let (e1, e2) = frame(axis);
                let s = [0, 1, 2].map(|j| pos[j] + (e1[j] * a + e2[j] * b) * radius);
                self.clear(p, sub(s, p), true, jit)
            }
            Source::Box { c, tx, ty, hw, hh } => {
                let (a, b) = ((1.15 * a).clamp(-1.0, 1.0) * hw, (1.15 * b).clamp(-1.0, 1.0) * hh);
                let s = [0, 1, 2].map(|j| c[j] + tx[j] * a + ty[j] * b);
                self.clear(p, sub(s, p), true, jit)
            }
            Source::Sun { to, spread } => {
                let (e1, e2) = self.sun_axes;
                let d = unit([0, 1, 2].map(|j| to[j] + (e1[j] * a + e2[j] * b) * spread), *to);
                self.clear(p, d, false, jit)
            }
        }
    }

    /// The share of the light that reaches pixel `(x, y)` of the reduced field.
    fn visible(&self, x: usize, y: usize, draft: bool) -> f32 {
        let r = self.r;
        let i = y * r.w + x;
        let (Some(z), f) = (r.z.get(i).copied(), r.f) else { return 1.0 };
        let (fw, fh) = (r.w * f, r.h * f);
        let (u, v) = (((x * f + f / 2) as f32 + 0.5) / fw as f32, ((y * f + f / 2) as f32 + 0.5) / fh as f32);
        let p = [(u - 0.5) * self.kx * z, (v - 0.5) * self.ky * z, z];
        let turn = noise(x, y, 1) * std::f32::consts::TAU;
        let jit = noise(x, y, 2);
        let first = (0..FIRST).filter(|k| self.ray(p, *k, turn, jit)).count();
        if draft || first == 0 || first == FIRST {
            return first as f32 / FIRST as f32;
        }
        // (a pixel in a penumbra: more rays across the whole light)
        let more = (FIRST..FIRST + MORE).filter(|k| self.ray(p, *k, turn, jit)).count();
        (first + more) as f32 / (FIRST + MORE) as f32
    }
}

/// `v` (the reduced field's visibility) averaged over each pixel's 3 × 3 neighbours that lie on the same surface (a log depth
/// within [`SAME`] of its own): the rays' fixed noise (each pixel's own turn of the rays and offset of the steps) goes, and
/// a shadow stays on its own surface.
fn smoothed(r: &Reduced, v: &[f32]) -> Vec<f32> {
    let mut out = vec![1f32; v.len()];
    out.par_chunks_mut(r.w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let own = r.lz.get(y * r.w + x).copied().unwrap_or(0.0);
            let (mut sum, mut n) = (0f32, 0f32);
            for yy in y.saturating_sub(1)..(y + 2).min(r.h) {
                for xx in x.saturating_sub(1)..(x + 2).min(r.w) {
                    let q = yy * r.w + xx;
                    if (r.lz.get(q).copied().unwrap_or(f32::MAX) - own).abs() < SAME {
                        sum += v.get(q).copied().unwrap_or(1.0);
                        n += 1.0;
                    }
                }
            }
            *o = if n > 0.0 { sum / n } else { v.get(y * r.w + x).copied().unwrap_or(1.0) };
        }
    });
    out
}

/// How far apart (log depth) two neighbouring pixels may be and still be the same surface to the smoothing of the visibility.
const SAME: f32 = 0.08;

impl Field {
    /// The reduced field the march runs on (made once).
    pub(crate) fn reduced(&self, draft: bool) -> &Reduced {
        if draft {
            self.reduced_draft.get_or_init(|| Reduced::of(self, LOW_SIDE_DRAFT))
        } else {
            self.reduced.get_or_init(|| Reduced::of(self, LOW_SIDE))
        }
    }

    /// How much of `light` (set up as `rig`) each pixel of the reduced field sees, `None` for a light that takes no
    /// shadow (a rim, a fill). It depends on the light's place, depth, Softness and type numbers and the field, not on
    /// its Amount or the shadow amount, so the caller keeps it until the light moves.
    pub fn visibility(&self, rig: &Rig, light: &Light, draft: bool) -> Option<Visibility> {
        let soft = if light.softness.is_finite() { light.softness.clamp(0.0, 1.0) as f32 } else { 0.5 };
        let src = match &rig.shape {
            Shape::Point | Shape::Spot { .. } => Source::Point { pos: rig.pos, radius: rig.r2.max(0.0).sqrt() * POINT_SHARE },
            Shape::Sun { to, .. } => Source::Sun { to: *to, spread: SUN_MIN + SUN_SPREAD * soft * soft },
            Shape::Box { c, tx, ty, hw, hh, .. } => Source::Box { c: *c, tx: *tx, ty: *ty, hw: *hw, hh: *hh },
            Shape::Rim { .. } | Shape::Fill { .. } => return None,
        };
        let r = self.reduced(draft);
        let long = self.width.max(self.height).max(1) as f32;
        let sun_axes = match &src {
            Source::Sun { to, .. } => frame(*to),
            _ => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        };
        let light_z = match &src {
            Source::Point { pos, .. } => Some(pos[2]),
            Source::Box { c, .. } => Some(c[2]),
            Source::Sun { .. } => None,
        };
        let march = March {
            r,
            src,
            kx: LENS * self.width as f32 / long,
            ky: LENS * self.height as f32 / long,
            z_cut: self.near.exp() * 0.5,
            z_cap: self.far.exp() * 3.0,
            sun_axes,
            light_z,
        };
        let mut v = vec![1f32; r.w * r.h];
        v.par_chunks_mut(r.w.max(1)).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let s = march.visible(x, y, draft);
                *o = if s.is_finite() { s.clamp(0.0, 1.0) } else { 1.0 };
            }
        });
        Some(Visibility { w: r.w, h: r.h, v: smoothed(r, &v), draft })
    }

    /// `lobe` with the shadows `vis` cast, `amount` of them (0 none, 1 all): each pixel's light is multiplied by
    /// `1 − amount · (1 − visibility)`, the visibility brought to the field's size edge-aware (see the module docs).
    /// A visibility of another size, or no amount, leaves the lobe as it is.
    pub fn shadowed(&self, mut lobe: Lobe, vis: &Visibility, amount: f64) -> Lobe {
        let a = if amount.is_finite() { amount.clamp(0.0, 1.0) as f32 } else { 0.0 };
        let r = self.reduced(vis.draft);
        let (w, h) = (self.width, self.height);
        if a <= 0.0 || vis.w != r.w || vis.h != r.h || vis.v.len() != r.w * r.h || lobe.m.len() != w * h || w == 0 {
            return lobe;
        }
        if vis.draft {
            // (a drag: the nearest sample, no filter)
            lobe.m.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let by = (y / r.f).min(r.h - 1) * r.w;
                for (x, m) in row.iter_mut().enumerate() {
                    let s = vis.v.get(by + (x / r.f).min(r.w - 1)).copied().unwrap_or(1.0);
                    *m *= 1.0 - a * (1.0 - if s.is_finite() { s.clamp(0.0, 1.0) } else { 1.0 });
                }
            });
            return lobe;
        }
        let f = r.f as f32;
        let (is_lum, is_depth) = (1.0 / (2.0 * SIGMA_LUM * SIGMA_LUM), 1.0 / (2.0 * SIGMA_DEPTH * SIGMA_DEPTH));
        lobe.m.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            // the reduced samples sit at the centre of their blocks
            let fy = ((y as f32 - (r.f / 2) as f32) / f).clamp(0.0, (r.h - 1) as f32);
            let (y0, ty) = (fy as usize, fy - fy.floor());
            let y1 = (y0 + 1).min(r.h - 1);
            for (x, m) in row.iter_mut().enumerate() {
                let fx = ((x as f32 - (r.f / 2) as f32) / f).clamp(0.0, (r.w - 1) as f32);
                let (x0, tx) = (fx as usize, fx - fx.floor());
                let x1 = (x0 + 1).min(r.w - 1);
                let i = y * w + x;
                let (gi, lz) = (self.gi.get(i).copied().unwrap_or(0.0), self.ln_z.get(i).copied().unwrap_or(0.0));
                let (mut sum, mut weights, mut nearest) = (0f32, 0f32, (f32::MAX, 1.0f32));
                for (qy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                    for (qx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                        let q = qy * r.w + qx;
                        let (Some(v), Some(g), Some(l)) = (vis.v.get(q), r.gi.get(q), r.lz.get(q)) else { continue };
                        let d = (gi - g) * (gi - g) * is_lum + (lz - l) * (lz - l) * is_depth;
                        let wt = wx * wy * (-d).exp();
                        sum += v * wt;
                        weights += wt;
                        if d < nearest.0 {
                            nearest = (d, *v);
                        }
                    }
                }
                // (no sample like the pixel at all: the one most like it)
                let s = if weights > 1e-6 { sum / weights } else { nearest.1 };
                let s = if s.is_finite() { s.clamp(0.0, 1.0) } else { 1.0 };
                *m *= 1.0 - a * (1.0 - s);
            }
        });
        lobe
    }
}
