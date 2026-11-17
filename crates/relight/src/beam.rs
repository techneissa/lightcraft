//! Light beams through haze (issue #317, `fork/relight.md`): a light's Haze makes the air glow where the light
//! reaches it, as a single-scattering medium seen along each pixel's line of sight.
//!
//! - **Up to the pixel's own depth.** Each pixel's camera ray is walked through the air from in front of the nearest
//!   thing to the pixel's surface, and sums the light the air there scatters towards the camera, dimmed by the haze in
//!   front of it (`exp(−σ·distance)`). A near subject ends its pixels' rays early, so it hides the beam behind it and the
//!   beam shows round it.
//! - **Cut by the same blockers as the shadows.** Each point of the air sends one ray towards the light through
//!   the depth map, held against the same blockers as a surface's ([`crate::shadow`], [`crate::occluder`]): air behind a
//!   person, a pole or a window's frame gets no light, so the beam breaks into shafts (parallel for a sun, a cone for a
//!   spot, the opening's shape for a softbox). Each ray aims at another point of the light, so a big light gives shafts
//!   with soft edges.
//! - **Forward scatter.** Haze scatters light mostly onward (the Henyey–Greenstein phase function, one number `g`):
//!   with the light behind the scene shining towards the camera the beams glow most, with the light behind the camera
//!   they are faint. The Forward Scatter control sets `g` from 0 (every way alike) to [`G_MAX`].
//! - **Dust.** The haze's density varies through the scene as a 3D noise of the scene's own points (not of the screen),
//!   so it stays put while the light moves.
//!
//! **Cost.** The rays towards the light, the dear part, depend only on the light's place, size and type and the
//! field ([`Shafts`]: kept by the caller until the light moves); Haze, Forward Scatter and Dust only weigh them again
//! ([`Field::beam`], one pass). The rays are traced along lines of sight every second pixel each way of the reduced field
//! the shadows' full march uses (every fourth for a drag, with half the steps), each line from the air's start to the
//! farthest thing; each pixel reads off the lines round it the air up to its own depth, so a beam stops at a near
//! thing's outline however coarse the lines. [`Field::beam_plane`] brings the result to the field's size with the
//! shadows' edge-aware filter.
//!
//! The glow is light added on top of what the pixel shows, in the light's own colour: [`Field::beam_plane`] gives it in units
//! of 18 % grey, and the finish stage adds it in scene-linear light after the lights' gain (CPU and GPU alike), so a beam shows
//! over black and takes no colour from the pixel under it. All of it is drawn from numbers here; nothing is read.

use rayon::prelude::*;

use crate::effects::Effects;
use crate::field::{Field, LENS};
use crate::kinds::{Shape, air_light, travel};
use crate::light::{Light, Lobe, Rig};
use crate::occluder::Reduced;

/// Steps along each line of sight, from the air's start to the farthest thing: for the release and the export, and for a drag.
const STEPS: usize = 32;
const STEPS_DRAFT: usize = 16;
/// Rays per step of a line of sight, aimed across the light and started across the line's cell.
const RAYS: usize = 2;
/// Two steps of a line of sight this close in what reaches them (of 255) give the steps between them, unmarched.
const RUN: u8 = 8;
/// Every how many steps a line of sight is marched first (the steps between only where the light changes): the release's,
/// and a drag's.
const FIRST_EVERY: usize = 2;
const FIRST_EVERY_DRAFT: usize = 2;
/// Lines of sight are traced every this many pixels of the reduced field each way (a quarter of its pixels; a drag's a
/// sixteenth), and every pixel reads its beam off the lines round it at its own depth.
const STRIDE: usize = 2;
const STRIDE_DRAFT: usize = 4;
/// The most forward the scatter gets at Forward Scatter 100 (0.85: about 150 times more onward than back).
pub const G_MAX: f32 = 0.85;
/// The glow of a light, in units of 18 % grey, per unit of optical depth and of the light's own strength: the radiance a
/// single-scattering medium of albedo 1 gives for the irradiance that lights a grey surface to grey (`1 / (4 · 0.18)`:
/// the isotropic phase's `1 / 4π` over a grey surface's `0.18 / π`).
const BEAM_SCALE: f32 = 1.0 / (4.0 * 0.18);
/// The most a beam adds to a pixel, in units of 18 % grey per unit of the light's strength (keeps every number finite).
const ADDED_MAX: f32 = 16.0;
/// How far apart (log depth) two neighbouring pixels may be and still be on the same surface to the smoothing.
const SAME: f32 = 0.08;
/// How much a line counts towards a pixel at a depth it saw only behind near things.
const BLIND: f32 = 0.02;
/// The air begins at this share of the nearest thing's depth.
const START: f32 = 0.3;
/// The dust's cells over the photo's farthest depth, and how far the density varies at Dust 100 (± this).
const DUST_CELLS: f32 = 14.0;
const DUST_SWING: f32 = 0.85;

fn unit(v: f64, d: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { d }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Which of the light's rays a light's points of the air send: how much of the light reaches each step of each traced
/// line of sight (0 none .. 255 all), before the haze. The lines run from the air's start to the farthest thing, every
/// `stride` pixels of the reduced field each way. It depends on the light's place, size and type numbers and the field,
/// not on Amount, Haze, Forward Scatter or Dust.
#[derive(Clone, Debug)]
pub struct Shafts {
    /// The reduced field's size, and the grid of traced lines over it.
    w: usize,
    h: usize,
    gw: usize,
    gh: usize,
    stride: usize,
    steps: usize,
    vis: Vec<u8>,
}

impl Shafts {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.vis.len()
    }
}

/// A light's beam on the reduced field: the light the air in front of each pixel scatters towards the camera, in units of
/// 18 % grey per unit of the light's strength.
#[derive(Clone, Debug)]
pub struct Beam {
    w: usize,
    h: usize,
    s: Vec<f32>,
}

impl Beam {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.s.len() * 4
    }

    /// The beam at pixel `(x, y)` of the reduced field (tests).
    #[cfg(test)]
    pub(crate) fn at(&self, x: usize, y: usize) -> f32 {
        self.s.get(y * self.w + x).copied().unwrap_or(0.0)
    }

    #[cfg(test)]
    pub(crate) fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }
}

/// A line of sight: its unit direction `e`, its length per unit depth `len`, the distances along it where the air begins
/// and ends, and how its steps are spread (`equi`: crowded towards where the line passes nearest the light, by equal angles
/// seen from the light, so a small light's glow is never stepped over; none for a sun, whose light is everywhere alike:
/// its steps are even in log depth).
struct Path {
    e: [f32; 3],
    len: f32,
    sa: f32,
    sb: f32,
    /// The distance of the line's nearest point to the light, that point's distance from the line's start, and the angles
    /// the line's two ends make there.
    equi: Option<(f32, f32, f32, f32)>,
    n: usize,
}

impl Path {
    /// The `k`th step: its distance along the line and the length of line it stands for.
    fn step(&self, k: usize) -> (f32, f32) {
        let t = (k as f32 + 0.5) / self.n as f32;
        match self.equi {
            Some((s0, d, ta, tb)) => {
                let th = ta + (tb - ta) * t;
                let s = s0 + d * th.tan();
                let ds = (tb - ta) / self.n as f32 * (d * d + (s - s0) * (s - s0)) / d;
                (s.clamp(self.sa, self.sb), ds)
            }
            // (evenly in log depth: as many steps in front of a near thing as the picture gives it room)
            None => {
                let span = (self.sb / self.sa).ln();
                let s = self.sa * (span * t).exp();
                (s, s * span / self.n as f32)
            }
        }
    }

    /// Where distance `s` along the line falls in its steps (0 the air's start .. `n` its end).
    fn index(&self, s: f32) -> f32 {
        let t = match self.equi {
            Some((s0, d, ta, tb)) => (((s - s0) / d).atan() - ta) / (tb - ta),
            None => (s / self.sa).ln() / (self.sb / self.sa).ln(),
        };
        if t.is_finite() { (t * self.n as f32).clamp(0.0, self.n as f32) } else { 0.0 }
    }

    fn point(&self, s: f32) -> [f32; 3] {
        self.e.map(|c| c * s)
    }
}

/// The Henyey–Greenstein phase function for asymmetry `g` at the cosine `cos` between the light's travel and the way to
/// the camera, over the isotropic one's (1 for `g = 0`).
fn phase(g: f32, cos: f32) -> f32 {
    let den = (1.0 + g * g - 2.0 * g * cos).max(1e-6);
    (1.0 - g * g) / (den * den.sqrt())
}

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / 16_777_216.0
}

/// Smooth value noise at `p` (cells), 0..1.
fn noise(p: [f32; 3]) -> f32 {
    if !p.iter().all(|c| c.is_finite() && c.abs() < 1e6) {
        return 0.5;
    }
    let f = p.map(f32::floor);
    let t = [0, 1, 2].map(|k| {
        let u = p[k] - f[k];
        u * u * (3.0 - 2.0 * u)
    });
    let (x, y, z) = (f[0] as i32, f[1] as i32, f[2] as i32);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let face = |dz: i32| {
        let row = |dy: i32| lerp(hash3(x, y + dy, z + dz), hash3(x + 1, y + dy, z + dz), t[0]);
        lerp(row(0), row(1), t[1])
    };
    lerp(face(0), face(1), t[2])
}

/// The dust's density at camera-space point `q` (1 on average), for Dust `dust` (0..1) and cells of `cell`.
fn dust_at(q: [f32; 3], cell: f32, dust: f32) -> f32 {
    if dust <= 0.0 {
        return 1.0;
    }
    let p = q.map(|c| c / cell);
    // (two octaves: wisps, and finer motes in them)
    let n = 0.65 * noise(p) + 0.35 * noise(p.map(|c| 2.7 * c + 11.0));
    (1.0 + dust * DUST_SWING * 2.0 * (n - 0.5) / 0.65).max(0.0)
}

impl Field {
    /// The camera's extent per unit depth on each axis.
    fn lens(&self) -> (f32, f32) {
        let long = self.width.max(self.height).max(1) as f32;
        (LENS * self.width as f32 / long, LENS * self.height as f32 / long)
    }

    /// The line of sight through pixel `(x, y)` of the reduced field `r`, from the air's start to the farthest thing, for a
    /// light set up as `rig`, in `n` steps; `None` when it has no air.
    fn line(&self, r: &Reduced, (x, y): (usize, usize), rig: &Rig, n: usize) -> Option<Path> {
        let f = r.f;
        let (fw, fh) = ((r.w * f) as f32, (r.h * f) as f32);
        let (u, v) = (((x * f + f / 2) as f32 + 0.5) / fw, ((y * f + f / 2) as f32 + 0.5) / fh);
        let (kx, ky) = self.lens();
        let d = [(u - 0.5) * kx, (v - 0.5) * ky, 1.0];
        let len = dot(d, d).sqrt();
        let e = d.map(|c| c / len);
        let (start, end) = (START * self.near.exp(), self.far.exp());
        if !(end.is_finite() && start.is_finite() && start > 0.0 && end > start && len.is_finite()) {
            return None;
        }
        let (sa, sb) = (start * len, end * len);
        let equi = match rig.shape {
            Shape::Sun { .. } => None,
            _ => {
                let p = rig.pos;
                let s0 = dot(p, e);
                let off = [p[0] - e[0] * s0, p[1] - e[1] * s0, p[2] - e[2] * s0];
                let d = dot(off, off).sqrt().max(1e-4 * sb);
                let (ta, tb) = (((sa - s0) / d).atan(), ((sb - s0) / d).atan());
                (s0.is_finite() && d.is_finite() && tb > ta).then_some((s0, d, ta, tb))
            }
        };
        Some(Path { e, len, sa, sb, equi, n })
    }

    /// How much of `light` (set up as `rig`) reaches the air along the traced lines of sight: the rays towards it, through
    /// the same blockers as the shadows. `None` for a light with no place (a rim, a fill). `draft`: a drag's (every fourth
    /// pixel each way, half the steps). The caller keeps it until the light moves.
    pub fn shafts(&self, rig: &Rig, light: &Light, draft: bool) -> Option<Shafts> {
        if !light.kind.has_place() {
            return None;
        }
        // (the march runs on the full reduced grid: on a coarser one thin leaves stop blocking; only the lines are fewer)
        let march = self.march(rig, light, false)?;
        let r = march.r;
        let (n, stride, every) = if draft { (STEPS_DRAFT, STRIDE_DRAFT, FIRST_EVERY_DRAFT) } else { (STEPS, STRIDE, FIRST_EVERY) };
        let (gw, gh) = (r.w.div_ceil(stride), r.h.div_ceil(stride));
        // (a cell's extent per unit depth, each way)
        let (kx, ky) = self.lens();
        let cell = (kx * stride as f32 / r.w.max(1) as f32, ky * stride as f32 / r.h.max(1) as f32);
        let mut vis = vec![255u8; gw * gh * n];
        vis.par_chunks_mut((gw * n).max(1)).enumerate().for_each(|(gy, row)| {
            for (gx, px) in row.chunks_mut(n).enumerate() {
                let (x, y) = line_pixel(r, stride, gx, gy);
                let Some(path) = self.line(r, (x, y), rig, n) else { continue };
                let ray = |k: usize| -> u8 {
                    let q = path.point(path.step(k).0);
                    // (air the light doesn't reach needs no ray)
                    if air_light(rig, q) <= 1e-4 {
                        return 255;
                    }
                    // (each step aims across the light, a few rays averaged: the shafts' edges are soft, wider farther from
                    // the gap; and each starts at another place in the line's cell, so the line stands for its whole cell and
                    // a shaft thinner than the cell is caught in part, not missed or hit in full. Every line takes the same
                    // places and the same points of the light: random ones per line leave blotches the size of a few lines)
                    let v = (0..RAYS)
                        .map(|i| {
                            let m = (k * RAYS + i) as f32;
                            let (a, b) = ((0.5 + m * 0.754_877_7).fract() - 0.5, (0.5 + m * 0.569_840_3).fract() - 0.5);
                            let p = [q[0] + a * cell.0 * q[2], q[1] + b * cell.1 * q[2], q[2]];
                            march.air_seen(p, k * RAYS + i, 1, 0.0)
                        })
                        .sum::<f32>()
                        / RAYS as f32;
                    if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 255 }
                };
                marched(px, every, &ray);
            }
        });
        Some(Shafts { w: r.w, h: r.h, gw, gh, stride, steps: n, vis })
    }

    /// The beam of `rig`'s light through the haze of effects `e` (Haze, Mist, Forward Scatter, Dust, and the light's pattern), from
    /// its `shafts` (traced from `rig`, or for a patterned lamp from its far source, [`Field::pattern_rig`]): the light the
    /// air in front of each pixel of the reduced field scatters towards the camera. All 0 for no haze or mist, or for shafts of
    /// another field.
    ///
    /// Each traced line sums its air step by step; the steps are smoothed across neighbouring lines (only steps in front of
    /// a line's own surface count: what lies behind a near thing comes from the lines beside it), and each pixel reads the
    /// sum up to its own depth off the four lines round it. So the beam is as sharp as the depth at a near thing's outline,
    /// and soft everywhere else.
    pub fn beam(&self, shafts: &Shafts, rig: &Rig, e: &Effects) -> Beam {
        let r = self.reduced(false);
        let (w, h, n, stride, gw, gh) = (r.w, r.h, shafts.steps, shafts.stride, shafts.gw, shafts.gh);
        let mut s = vec![0f32; w * h];
        // (the haze, and with Mist the layer over the ground, issue #331: no air, no beam)
        let Some(medium) = self.medium(e) else { return Beam { w, h, s } };
        if shafts.w != w || shafts.h != h || shafts.vis.len() != gw * gh * n || n == 0 || stride == 0 {
            return Beam { w, h, s };
        }
        let zfar = self.far.exp().max(1e-6);
        let g = G_MAX * unit(e.scatter, 0.5);
        let dust = unit(e.dust, 0.0);
        let cell = zfar / DUST_CELLS;
        // (a patterned lamp shines from far along its direction, as its lobe and shadows do, issue #316: the shafts were traced
        // from there too)
        let far = self.pattern_rig(rig, e);
        let from = far.as_ref().unwrap_or(rig);
        // (the light's pattern cuts the air too: blinds and window panes make shafts of their shape)
        let pattern = self.air_pattern(rig, from, e);
        let lines: Vec<Option<Path>> = (0..gw * gh).into_par_iter().map(|j| self.line(r, line_pixel(r, stride, j % gw, j / gw), from, n)).collect();
        // (each line's air step by step, and whether the step lies in front of the line's own surface)
        let mut air = vec![0f32; gw * gh * n];
        let mut front = vec![false; gw * gh * n];
        air.par_chunks_mut(n).zip(front.par_chunks_mut(n)).enumerate().for_each(|(j, (out, ok))| {
            let Some(Some(path)) = lines.get(j) else { return };
            let (x, y) = line_pixel(r, stride, j % gw, j / gw);
            let own = r.z.get(y * w + x).copied().unwrap_or(0.0) * path.len;
            let to_camera = path.e.map(|c| -c);
            for (k, (o, f)) in out.iter_mut().zip(ok.iter_mut()).enumerate() {
                let (d, ds) = path.step(k);
                *f = d <= own;
                let q = path.point(d);
                let light = air_light(from, q) * pattern.as_ref().map_or(1.0, |p| p.at(q));
                if light <= 0.0 {
                    continue;
                }
                let seen = f32::from(shafts.vis.get(j * n + k).copied().unwrap_or(255)) / 255.0;
                let scatter = phase(g, dot(travel(from, q), to_camera));
                let v = ds * medium.at(q) * dust_at(q, cell, dust) * (-medium.depth(path.e, d)).exp() * light * seen * scatter;
                *o = if v.is_finite() { v.max(0.0) } else { 0.0 };
            }
        });
        // (once for the release, twice over a drag's fewer lines: soft, and no line stands out)
        let (mut air, mut front) = across(&air, &front, (gw, gh, n));
        if stride > STRIDE {
            (air, front) = across(&air, &front, (gw, gh, n));
        }
        // (each line's sum up to the end of each step)
        let mut sums = vec![0f32; gw * gh * (n + 1)];
        sums.par_chunks_mut(n + 1).zip(air.par_chunks(n)).for_each(|(c, a)| {
            for k in 0..n {
                let next = c.get(k).copied().unwrap_or(0.0) + a.get(k).copied().unwrap_or(0.0);
                if let Some(o) = c.get_mut(k + 1) {
                    *o = next;
                }
            }
        });
        let half = (stride / 2) as f32;
        s.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            let fy = ((y as f32 - half) / stride as f32).clamp(0.0, (gh - 1) as f32);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(gh - 1);
            for (x, out) in row.iter_mut().enumerate() {
                let Some(z) = r.z.get(y * w + x).copied().filter(|z| z.is_finite() && *z > 0.0) else { continue };
                let fx = ((x as f32 - half) / stride as f32).clamp(0.0, (gw - 1) as f32);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(gw - 1);
                let (mut sum, mut wsum) = (0f32, 0f32);
                for (j, wt) in [
                    (y0 * gw + x0, (1.0 - tx) * (1.0 - ty)),
                    (y0 * gw + x1, tx * (1.0 - ty)),
                    (y1 * gw + x0, (1.0 - tx) * ty),
                    (y1 * gw + x1, tx * ty),
                ] {
                    let Some(Some(path)) = lines.get(j) else { continue };
                    let i = path.index(z * path.len);
                    let k = (i as usize).min(n.saturating_sub(1));
                    let at = |k: usize| sums.get(j * (n + 1) + k).copied().unwrap_or(0.0);
                    // (a line that saw no air at this depth, all behind near things, counts for little)
                    let wt = if front.get(j * n + k).copied().unwrap_or(false) { wt } else { wt * BLIND };
                    sum += wt * (at(k) + (at(k + 1) - at(k)) * (i - k as f32));
                    wsum += wt;
                }
                let v = if wsum > 1e-6 { sum / wsum * BEAM_SCALE } else { 0.0 };
                *out = if v.is_finite() { v.max(0.0) } else { 0.0 };
            }
        });
        // (a surface whose depth ripples, a bank of leaves, takes a beam that ripples with it: twice over its own surface)
        let s = same_surface_mean(r, &same_surface_mean(r, &s, 1), 1);
        Beam { w, h, s }
    }

    /// `beam` at the field's size (edge-aware: a beam stops at a near thing's outline): the light the beam adds on top of each
    /// pixel, in units of 18 % grey per unit of the light's strength ([`crate::Rig::gain`]), whatever the pixel shows (issue #317:
    /// it is added in the light's own colour by the finish stage, so it shows over black and takes no colour from the pixel).
    pub fn beam_plane(&self, beam: &Beam) -> Lobe {
        let r = self.reduced(false);
        let n = self.width * self.height;
        if beam.w != r.w || beam.h != r.h {
            return Lobe { m: vec![0.0; n] };
        }
        let mut m = self.upsampled(r, &beam.s, 0.0);
        m.par_iter_mut().for_each(|v| *v = if v.is_finite() { v.clamp(0.0, ADDED_MAX) } else { 0.0 });
        Lobe { m }
    }

    /// A field-sized plane (`lobe`) over `w × h` render pixels, bilinear; `to_photo` maps a pixel centre to normalized photo
    /// coordinates (a pixel it maps to no number gets 0).
    pub fn plane_at(&self, lobe: &Lobe, w: usize, h: usize, to_photo: &(dyn Fn(f64, f64) -> (f64, f64) + Sync)) -> Vec<f32> {
        let (fw, fh) = (self.width, self.height);
        let mut out = vec![0f32; w.saturating_mul(h)];
        if lobe.m.len() != fw * fh || fw == 0 || fh == 0 {
            return out;
        }
        out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let (u, v) = to_photo(x as f64 + 0.5, y as f64 + 0.5);
                if !(u.is_finite() && v.is_finite()) {
                    continue;
                }
                let fx = (u as f32 * fw as f32 - 0.5).clamp(0.0, (fw - 1) as f32);
                let fy = (v as f32 * fh as f32 - 0.5).clamp(0.0, (fh - 1) as f32);
                let (x0, y0) = (fx as usize, fy as usize);
                let (x1, y1) = ((x0 + 1).min(fw - 1), (y0 + 1).min(fh - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let at = |x: usize, y: usize| lobe.m.get(y * fw + x).copied().unwrap_or(0.0);
                let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
                let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
                *o = top + (bottom - top) * ty;
            }
        });
        out
    }
}

/// The reduced field's pixel the line `(gx, gy)` of a grid of every `stride` pixels goes through.
fn line_pixel(r: &Reduced, stride: usize, gx: usize, gy: usize) -> (usize, usize) {
    ((gx * stride + stride / 2).min(r.w.saturating_sub(1)), (gy * stride + stride / 2).min(r.h.saturating_sub(1)))
}

/// The steps of one line of sight (`px`), from `ray`: every `every`th step and the last are marched, and the steps between
/// two of them only where the two differ by more than [`RUN`] (halving the gap each time); the rest are blended.
fn marched(px: &mut [u8], every: usize, ray: &dyn Fn(usize) -> u8) {
    let n = px.len();
    let mut marks: Vec<usize> = (0..n).step_by(every.max(1)).chain(n.checked_sub(1)).collect();
    marks.dedup();
    for &k in &marks {
        if let Some(o) = px.get_mut(k) {
            *o = ray(k);
        }
    }
    for pair in marks.windows(2) {
        if let [a, b] = *pair {
            refine(px, a, b, ray);
        }
    }
}

/// The steps strictly between `a` and `b` (both marched): blended when the two agree, else the middle one is marched and
/// each half refined (depth at most log₂ of the gap).
fn refine(px: &mut [u8], a: usize, b: usize, ray: &dyn Fn(usize) -> u8) {
    if b <= a + 1 {
        return;
    }
    let (va, vb) = (px.get(a).copied().unwrap_or(255), px.get(b).copied().unwrap_or(255));
    if va.abs_diff(vb) <= RUN {
        for k in a + 1..b {
            let t = (k - a) as f32 / (b - a) as f32;
            if let Some(o) = px.get_mut(k) {
                *o = (f32::from(va) + (f32::from(vb) - f32::from(va)) * t).round() as u8;
            }
        }
        return;
    }
    let m = (a + b) / 2;
    if let Some(o) = px.get_mut(m) {
        *o = ray(m);
    }
    refine(px, a, m, ray);
    refine(px, m, b, ray);
}

/// `air` (a grid of `gw × gh` lines of `n` steps) smoothed across neighbouring lines, 3 × 3 (1 2 1 each way), step by
/// step: a step takes only the neighbours' steps that lie in front of their own surface (`front`). A step with none
/// keeps its own. Returns the smoothed air and which steps now hold air seen in front of some surface.
fn across(air: &[f32], front: &[bool], (gw, gh, n): (usize, usize, usize)) -> (Vec<f32>, Vec<bool>) {
    let mut out = vec![0f32; air.len()];
    let mut ok = vec![false; air.len()];
    out.par_chunks_mut(n.max(1)).zip(ok.par_chunks_mut(n.max(1))).enumerate().for_each(|(j, (o, f))| {
        let (gx, gy) = (j % gw.max(1), j / gw.max(1));
        for k in 0..n {
            let (mut sum, mut wsum) = (0f32, 0f32);
            for yy in gy.saturating_sub(1)..(gy + 2).min(gh) {
                for xx in gx.saturating_sub(1)..(gx + 2).min(gw) {
                    let q = (yy * gw + xx) * n + k;
                    if front.get(q).copied().unwrap_or(false) {
                        let wt = (if xx == gx { 2.0 } else { 1.0 }) * (if yy == gy { 2.0 } else { 1.0 });
                        sum += wt * air.get(q).copied().unwrap_or(0.0);
                        wsum += wt;
                    }
                }
            }
            let own = air.get(j * n + k).copied().unwrap_or(0.0);
            if let (Some(o), Some(f)) = (o.get_mut(k), f.get_mut(k)) {
                (*o, *f) = if wsum > 0.0 { (sum / wsum, true) } else { (own, false) };
            }
        }
    });
    (out, ok)
}

/// `v` averaged over each pixel's neighbours within `radius` (1: 3 × 3) on the same surface (a log depth within [`SAME`] of its own).
pub(crate) fn same_surface_mean(r: &Reduced, v: &[f32], radius: usize) -> Vec<f32> {
    let mut out = vec![0f32; v.len()];
    out.par_chunks_mut(r.w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let own = r.lz.get(y * r.w + x).copied().unwrap_or(0.0);
            let (mut sum, mut n) = (0f32, 0f32);
            for yy in y.saturating_sub(radius)..(y + radius + 1).min(r.h) {
                for xx in x.saturating_sub(radius)..(x + radius + 1).min(r.w) {
                    let q = yy * r.w + xx;
                    if (r.lz.get(q).copied().unwrap_or(f32::MAX) - own).abs() < SAME {
                        sum += v.get(q).copied().unwrap_or(0.0);
                        n += 1.0;
                    }
                }
            }
            *o = if n > 0.0 { sum / n } else { v.get(y * r.w + x).copied().unwrap_or(0.0) };
        }
    });
    out
}
