//! Shadows (issue #315, `fork/relight.md`): a light is blocked by what the photo shows. For each pixel's surface point
//! the light's rays are walked through the depth map towards the light (a ray towards the light's place for a point, a
//! spot and a softbox, parallel rays along the direction for a sun), as in a screen-space shadow march. What blocks a
//! ray is decided by [`crate::occluder`]: only a thing with farther stuff on both sides of it (a person, a box, a pole,
//! not a floor, a wall or a staircase), taken to be about as deep as it is wide. A ray is blocked where it passes
//! through such a thing: the ray is followed on the picture through every pixel it crosses, and the range of depth it has
//! over each pixel is held against the thing's there ([`held`]), so a thing one pixel wide, crossed sideways or in depth,
//! still blocks in full, and a light or a surface moving in depth changes the shadow smoothly (the thing fades in just
//! behind its front and out behind its back). The pixel that holds back most decides the ray.
//!
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
pub(crate) use crate::occluder::Reduced;
use crate::occluder::{Blocker, TILE};

/// The long side of the reduced field the march runs on (the reduction is the whole number of field pixels that brings
/// it closest to this from above: 4 for the usual 1600-pixel field, 1 for a small one), for a drag's draft and for the full march.
const LOW_SIDE_DRAFT: usize = 200;
pub(crate) const LOW_SIDE: usize = 400;
/// Rays of the first pass (one at the light's middle and three round its edge); a pixel whose rays disagree (it is in
/// a penumbra) gets [`MORE`] more across the whole light.
pub(crate) const FIRST: usize = 4;
pub(crate) const MORE: usize = 12;
/// A ray counts as inside a thing from this ratio of its front's depth and in full from the second (a little behind
/// its front: the surface a ray starts on, a body's own curve and the depth model's ripples don't shadow it), or from a
/// quarter and a half of the thing's depth when it is thinner (a pole: it still blocks in full).
const FRONT: (f32, f32) = (1.03, 1.08);
/// How deep a surface that is no thing (a wall, a bank of leaves) is to the air's rays (issue #317), as ratios of its front's
/// depth: in full to the first, fading to nothing at the second. The depth map shows only its front; a window or a gap in
/// it lets the light through.
const WALL: (f32, f32) = (1.25, 1.4);
/// An air's ray leaving through a side of the picture is held in full by the edge once it goes this far into the scene per
/// unit of its length (the cosine of its angle to the camera's axis), less in part below.
const EDGE_INTO: f32 = 0.15;
/// How far apart the first rays of a pixel may be before the penumbra pass runs (they agree on a partial value where a light is hidden in
/// part: nothing to refine).
const SPREAD: f32 = 0.05;
/// How much of the light's own radius (the sphere its fall-off comes from) a point or a spot's shadow uses.
const POINT_SHARE: f32 = 0.5;
/// What the sun's angular radius gains in the air (radians, about 3°): forward scattering in the haze spreads a shaft's edge
/// (issue #317).
const AIR_SUN_SPREAD: f32 = 0.05;
/// The sun's angular radius (radians) at Softness 0 and the most it adds at Softness 1.
const SUN_MIN: f32 = 0.012;
const SUN_SPREAD: f32 = 0.26;
/// Where the weights of the upsampling fall to 1/e: the photo's brightness (cube root of linear light) and log depth.
const SIGMA_LUM: f32 = 0.15;
/// (differences under these count for nothing: the photo's texture, the depth model's ripples and a floor's own slope between two samples
/// are no edge, and the shadow's edge is then not blocky)
const DEAD_LUM: f32 = 0.08;
const DEAD_DEPTH: f32 = 0.05;
/// (the depth's is narrow: a step of 5–10 % of depth, a near thing's outline, must not let its shadow leak across)
const SIGMA_DEPTH: f32 = 0.05;

impl LightKind {
    /// Whether a light of this type casts shadows: a point, a spot, a sun and a softbox do, a rim and a fill have no
    /// place the light comes from.
    pub fn takes_shadows(&self) -> bool {
        matches!(self, LightKind::Point | LightKind::Spot(_) | LightKind::Sun(_) | LightKind::Softbox(_))
    }
}

/// How much of a ray whose depth runs over `lo ..= hi` at a pixel the thing there (front at depth `zs`, blocker `b`)
/// holds back: the most of its presence over that range of depth, 0 (none) .. 1. The presence rises from 0 at
/// [`FRONT`]`.0` times the front's depth to 1 at [`FRONT`]`.1` (sooner for a thin thing), stays 1 to the thing's `full`
/// depth and falls to 0 at its `end`.
pub(crate) fn held(zs: f32, b: &Blocker, lo: f32, hi: f32) -> f32 {
    let deep = b.full - 1.0;
    if !(b.s > 0.0 && zs > 0.0 && deep > 1e-4 && b.end >= b.full) {
        return 0.0;
    }
    let (fa, fb) = (1.0 + (FRONT.0 - 1.0).min(0.25 * deep), 1.0 + (FRONT.1 - 1.0).min(0.5 * deep));
    let (za, zb, zc, zd) = (zs * fa, zs * fb, zs * b.full, zs * b.end);
    let (rise, fall) = ((zb - za).max(1e-9), (zd - zc).max(1e-9));
    let up = |z: f32| ((z - za) / rise).clamp(0.0, 1.0);
    let down = |z: f32| if z <= zc { 1.0 } else { ((zd - z) / fall).clamp(0.0, 1.0) };
    // (the presence is a trapezoid: its top, or where its sides cross for a thin one, nearest the ray's range is the most of it there)
    let top = ((za * fall + zd * rise) / (rise + fall)).clamp(za, zd);
    let z = top.clamp(lo.min(hi), hi.max(lo));
    let v = b.s * up(z).min(down(z));
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
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

/// A number in 0..1 from a pixel's place that differs as much as it can between neighbours (interleaved gradient noise): each
/// pixel of a 3 × 3 block turns its rays a different way, so the smoothing ([`smoothed`]) averages the turns out and a wide
/// penumbra is not grainy.
pub(crate) fn spread_noise(x: usize, y: usize) -> f32 {
    let g = 0.067_110_56 * x as f32 + 0.005_837_15 * y as f32;
    (52.982_918 * (g - g.floor())).fract()
}

/// The golden angle (radians): successive points of a sunflower spread round a disc evenly.
const GOLDEN: f32 = 2.399_963;

/// The `k`th of the rays across the light as a point of the unit disc: the first pass's are the middle and three on
/// the edge's 0.8, the rest a sunflower (`turn`: this pixel's rotation of the pattern).
pub(crate) fn offset(k: usize, turn: f32) -> (f32, f32) {
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

/// The cells of side `size` that the segment from `a` to `b` (picture coordinates, reduced pixels, not negative) crosses between
/// its shares `from` and `to`, in order, each with the shares where the segment enters and leaves it; `visit` returns false to
/// stop, and so does this.
fn cells(a: (f32, f32), b: (f32, f32), size: f32, from: f32, to: f32, visit: &mut dyn FnMut(usize, usize, f32, f32) -> bool) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (x0, y0) = (a.0 + dx * from, a.1 + dy * from);
    if !(x0.is_finite() && y0.is_finite() && dx.is_finite() && dy.is_finite() && size > 0.0) {
        return true;
    }
    let (mut cx, mut cy) = ((x0 / size).floor().max(0.0) as usize, (y0 / size).floor().max(0.0) as usize);
    // the share at which the segment next crosses a column's or a row's border, and the share between two of them
    let first = |c: usize, start: f32, d: f32| {
        if d > 0.0 {
            ((c + 1) as f32 * size - start) / d
        } else if d < 0.0 {
            (c as f32 * size - start) / d
        } else {
            f32::INFINITY
        }
    };
    let (mut nx, mut ny) = (first(cx, a.0, dx), first(cy, a.1, dy));
    let (ex, ey) = (if dx != 0.0 { size / dx.abs() } else { f32::INFINITY }, if dy != 0.0 { size / dy.abs() } else { f32::INFINITY });
    let most = (((dx.abs() + dy.abs()) * (to - from).max(0.0) / size).ceil() as usize).saturating_add(3);
    let mut s = from;
    for _ in 0..most {
        let next = nx.min(ny).min(to).max(s);
        if !visit(cx, cy, s, next) {
            return false;
        }
        if next >= to {
            break;
        }
        if nx <= ny {
            nx += ex;
            match if dx > 0.0 { cx.checked_add(1) } else { cx.checked_sub(1) } {
                Some(c) => cx = c,
                None => break,
            }
        } else {
            ny += ey;
            match if dy > 0.0 { cy.checked_add(1) } else { cy.checked_sub(1) } {
                Some(c) => cy = c,
                None => break,
            }
        }
        s = next;
    }
    true
}

/// The march's numbers for one reduced field and one light.
pub(crate) struct March<'a> {
    pub(crate) r: &'a Reduced,
    src: Source,
    /// The camera's half-extent per unit depth, on each axis.
    pub(crate) kx: f32,
    pub(crate) ky: f32,
    /// Nothing is nearer than this (the light never sits in front of it, so the ray ends), and nothing farther than this.
    z_cut: f32,
    z_cap: f32,
    /// The sun's frame ⟂ its direction.
    sun_axes: ([f32; 3], [f32; 3]),
}

impl March<'_> {
    /// How far along `d` from `p` the ray stays in the picture and between [`March::z_cut`] and [`March::z_cap`], and whether it leaves through one of the picture's sides (not past the nearest or farthest depth).
    fn exit_side(&self, p: [f32; 3], d: [f32; 3]) -> (f32, bool) {
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
        let t_side = t;
        if d[2] < -1e-9 {
            t = t.min((p[2] - self.z_cut) / -d[2]);
        } else if d[2] > 1e-9 {
            t = t.min((self.z_cap - p[2]) / d[2]);
        }
        if t.is_finite() { (t.max(0.0), t_side <= t) } else { (0.0, false) }
    }

    /// Where `q` (a point of the scene) falls on the reduced field: its pixel's column and row (clamped to the picture), and
    /// its place in reduced pixels.
    fn pixel(&self, q: [f32; 3]) -> Option<((usize, usize), (f32, f32))> {
        if q[2].is_nan() || q[2] <= 1e-6 {
            return None;
        }
        let r = self.r;
        let iz = 1.0 / q[2];
        let (u, v) = ((q[0] * iz / self.kx + 0.5) * r.w as f32, (q[1] * iz / self.ky + 0.5) * r.h as f32);
        if !(u.is_finite() && v.is_finite()) {
            return None;
        }
        Some((((u.max(0.0) as usize).min(r.w - 1), (v.max(0.0) as usize).min(r.h - 1)), (u, v)))
    }

    /// How much of the light the ray from `p` along `d` (`d` spans the whole way to the light when `whole`, else it is a direction)
    /// lets through: 1 when nothing blocks it, 0 when a thing does. The ray is followed on the picture through every reduced pixel
    /// it crosses ([`cells`]: a thing one pixel wide is never stepped over), skipping each [`TILE`] × [`TILE`] block where nothing can
    /// hold it, and held against the thing at each pixel over the depths the ray has there ([`held`]: along a line on the picture,
    /// 1 / depth changes evenly, so those depths are exact). The pixel that holds back most decides.
    ///
    /// `air`: the ray starts in the air (issue #317, [`crate::beam`]) and is also held by every surface it passes just behind
    /// that is no thing standing in front of others (a wall, a far bank of leaves, taken to be [`WALL`] deep: light from behind
    /// them comes only through their gaps), except open sky ([`Reduced::sky`]); a thing keeps its own depth. A surface's own ray is not (its own depth ripples would shade it).
    fn clear_from(&self, p: [f32; 3], d: [f32; 3], whole: bool, air: bool) -> f32 {
        let r = self.r;
        let (out, side) = self.exit_side(p, d);
        let mut end = out;
        if whole {
            end = end.min(1.0);
        }
        // (a sun's air ray that leaves through a side of the picture going deeper: the scene goes on past the edge as the edge
        // shows it, so unless the edge is open sky it passes behind it there; a light with a place is nearby, its ray is clear)
        let edge = if air && !whole && side && d[2] > 0.0 {
            let q = [p[0] + d[0] * out, p[1] + d[1] * out, p[2] + d[2] * out];
            let into = d[2] / (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
            match self.pixel(q) {
                Some(((x, y), _)) if !r.sky.get(y * r.w + x).copied().unwrap_or(true) => 1.0 - (into / EDGE_INTO).clamp(0.0, 1.0),
                _ => 1.0,
            }
        } else {
            1.0
        };
        if end.is_nan() || end <= 1e-6 {
            return 1.0;
        }
        // (only the part of the ray between the nearest blocker's front and the farthest one's end can be held back; the
        // air's ray, by any surface: all of it)
        let (zlo, zhi) = if air { (0.0, f32::INFINITY) } else { r.span };
        let (t_in, t_out) = if d[2].abs() > 1e-9 {
            let (ta, tb) = ((zlo - p[2]) / d[2], (zhi - p[2]) / d[2]);
            (ta.min(tb).max(0.0), ta.max(tb).min(end))
        } else if (zlo..=zhi).contains(&p[2]) {
            (0.0, end)
        } else {
            return 1.0;
        };
        if t_in.is_nan() || t_out.is_nan() || t_out <= t_in {
            return 1.0;
        }
        let at = |t: f32| [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
        let (qa, qb) = (at(t_in), at(t_out));
        let (Some((_, a)), Some((_, b))) = (self.pixel(qa), self.pixel(qb)) else { return 1.0 };
        let (ia, ib) = (1.0 / qa[2], 1.0 / qb[2]);
        let depths = |s0: f32, s1: f32| {
            let (z0, z1) = (1.0 / (ia + (ib - ia) * s0), 1.0 / (ia + (ib - ia) * s1));
            (z0.min(z1), z0.max(z1))
        };
        // (inside the picture: a ray end on its border sits on its last pixel)
        let inside = |q: (f32, f32)| (q.0.clamp(0.0, r.w as f32 - 1e-3), q.1.clamp(0.0, r.h as f32 - 1e-3));
        let (a, b) = (inside(a), inside(b));
        let mut through = 1.0f32;
        // (the air's ray goes out through a gap onto open sky when it crosses a sky pixel at least as deep as the last surface it
        // passed, the one framing the gap; then nothing more holds it, the picture's edge included)
        let mut out = false;
        let mut last_z: Option<f32> = None;
        cells(a, b, TILE as f32, 0.0, 1.0, &mut |tx, ty, s0, s1| {
            let (lo, hi) = depths(s0, s1);
            if tx >= r.tw {
                return true;
            }
            let t = ty * r.tw + tx;
            let blocks = r.tiles.get(t).is_some_and(|&(near, far)| hi >= near && lo <= far);
            let behind = air
                && r.tile_zmin.get(t).is_some_and(|zmin| hi > zmin * FRONT.0 && lo < r.tile_zmax.get(t).copied().unwrap_or(f32::INFINITY) * WALL.1);
            let sky_here = air && r.tile_sky.get(t).copied().unwrap_or(false);
            if !(blocks || behind || sky_here) {
                // (passed in front of everything here: its nearest surface is the last the ray passed)
                if air && let Some(z) = r.tile_zmin.get(t).filter(|z| z.is_finite()) {
                    last_z = Some(*z);
                }
                return true;
            }
            cells(a, b, 1.0, s0, s1, &mut |x, y, s0, s1| {
                if x >= r.w || y >= r.h {
                    return true;
                }
                let k = y * r.w + x;
                let (Some(zs), Some(blk)) = (r.z.get(k), r.blk.get(k)) else { return true };
                let (lo, hi) = depths(s0, s1);
                if air {
                    if r.sky.get(k).copied().unwrap_or(false) {
                        if hi >= last_z.unwrap_or(r.back_z) * FRONT.0 {
                            out = true;
                            return false;
                        }
                        return true;
                    }
                    last_z = Some(*zs);
                }
                through = through.min(1.0 - held(*zs, blk, lo, hi));
                if air && !r.sky.get(k).copied().unwrap_or(false) {
                    // (a surface that is no thing standing in front of others holds the air's ray: the backdrop as far behind
                    // it as the ray goes, a wall or a nearer bank of leaves as a slab [`WALL`] deep; a thing keeps its own
                    // depth, `held`)
                    let open = 1.0 - blk.s.clamp(0.0, 1.0);
                    let past = if r.back.get(k).copied().unwrap_or(false) {
                        open * ((hi / zs - FRONT.0) / (FRONT.1 - FRONT.0)).clamp(0.0, 1.0)
                    } else {
                        held(*zs, &Blocker { s: open, full: WALL.0, end: WALL.1 }, lo, hi)
                    };
                    through = through.min(1.0 - past);
                }
                through > 0.0
            })
        });
        if out { through } else { through.min(edge) }
    }

    /// The ray `k` from `p` (a surface point) towards the light, turned by `turn`; whether it reaches the light.
    fn ray(&self, p: [f32; 3], k: usize, turn: f32) -> f32 {
        self.toward(p, offset(k, turn))
    }

    /// Whether the ray from `p` (a surface point, or a point of the air for the beams, [`crate::beam`]) towards the point `(a, b)`
    /// of the light (its unit disc, or its rectangle) reaches it: 1 all of it .. 0 blocked.
    pub(crate) fn toward(&self, p: [f32; 3], (a, b): (f32, f32)) -> f32 {
        self.toward_from(p, (a, b), false)
    }

    /// [`March::toward`] from a point of the air (issue #317): every surface the ray passes behind holds it too.
    pub(crate) fn toward_air(&self, p: [f32; 3], (a, b): (f32, f32)) -> f32 {
        self.toward_from(p, (a, b), true)
    }

    fn toward_from(&self, p: [f32; 3], (a, b): (f32, f32), air: bool) -> f32 {
        match &self.src {
            Source::Point { pos, radius } => {
                let axis = unit(sub(*pos, p), [0.0, 0.0, -1.0]);
                let (e1, e2) = frame(axis);
                let s = [0, 1, 2].map(|j| pos[j] + (e1[j] * a + e2[j] * b) * radius);
                self.clear_from(p, sub(s, p), true, air)
            }
            Source::Box { c, tx, ty, hw, hh } => {
                let (a, b) = ((1.15 * a).clamp(-1.0, 1.0) * hw, (1.15 * b).clamp(-1.0, 1.0) * hh);
                let s = [0, 1, 2].map(|j| c[j] + tx[j] * a + ty[j] * b);
                self.clear_from(p, sub(s, p), true, air)
            }
            Source::Sun { to, spread } => {
                let (e1, e2) = self.sun_axes;
                // (in the air the sun's disc counts wider: the haze's own scattering spreads a shaft's edge, so the edge softens
                // as it goes from the gap that makes it, issue #317)
                let spread = if air { spread + AIR_SUN_SPREAD } else { *spread };
                let d = unit([0, 1, 2].map(|j| to[j] + (e1[j] * a + e2[j] * b) * spread), *to);
                self.clear_from(p, d, false, air)
            }
        }
    }

    /// How much of the light reaches `p`, a point of the air (issue #317): `n` rays aimed across the light (the `k`th of a
    /// sunflower turned by `turn` and onwards), averaged, so the light's edge is soft.
    pub(crate) fn air_seen(&self, p: [f32; 3], k: usize, n: usize, turn: f32) -> f32 {
        let n = n.max(1);
        let sum: f32 = (0..n).map(|j| self.toward_air(p, offset(FIRST + (k * n + j) % MORE, turn))).sum();
        sum / n as f32
    }

    /// The share of the light that reaches pixel `(x, y)` of the reduced field.
    fn visible(&self, x: usize, y: usize, draft: bool) -> f32 {
        let r = self.r;
        let i = y * r.w + x;
        let (Some(z), f) = (r.z.get(i).copied(), r.f) else { return 1.0 };
        let (fw, fh) = (r.w * f, r.h * f);
        let (u, v) = (((x * f + f / 2) as f32 + 0.5) / fw as f32, ((y * f + f / 2) as f32 + 0.5) / fh as f32);
        let p = [(u - 0.5) * self.kx * z, (v - 0.5) * self.ky * z, z];
        let turn = spread_noise(x, y) * std::f32::consts::TAU;
        let rays: Vec<f32> = (0..FIRST).map(|k| self.ray(p, k, turn)).collect();
        let (first, lo, hi) = (rays.iter().sum::<f32>(), rays.iter().copied().fold(1.0, f32::min), rays.iter().copied().fold(0.0, f32::max));
        if draft || hi - lo < SPREAD {
            return first / FIRST as f32;
        }
        // (a pixel in a penumbra: more rays across the whole light)
        let more: f32 = (FIRST..FIRST + MORE).map(|k| self.ray(p, k, turn)).sum();
        (first + more) / (FIRST + MORE) as f32
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
            let mine = v.get(y * r.w + x).copied().unwrap_or(1.0);
            let (mut sum, mut n) = (0f32, 0f32);
            for yy in y.saturating_sub(1)..(y + 2).min(r.h) {
                for xx in x.saturating_sub(1)..(x + 2).min(r.w) {
                    let q = yy * r.w + xx;
                    let theirs = v.get(q).copied().unwrap_or(1.0);
                    // (a neighbour far from this one in value is the other side of a hard edge: kept apart, so an edge near its object stays sharp)
                    if (r.lz.get(q).copied().unwrap_or(f32::MAX) - own).abs() < SAME && (theirs - mine).abs() < EDGE {
                        sum += theirs;
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
/// How far apart the values of two neighbours may be for the smoothing to average them (more is an edge of the shadow).
const EDGE: f32 = 2.0;

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
        let march = self.march(rig, light, draft)?;
        let r = march.r;
        let mut v = vec![1f32; r.w * r.h];
        v.par_chunks_mut(r.w.max(1)).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let s = march.visible(x, y, draft);
                *o = if s.is_finite() { s.clamp(0.0, 1.0) } else { 1.0 };
            }
        });
        // (the full one twice: a wide penumbra's rays differ most, and two passes leave no grain)
        let v = smoothed(r, &v);
        let v = if draft { v } else { smoothed(r, &v) };
        Some(Visibility { w: r.w, h: r.h, v, draft })
    }

    /// `light`'s shadow rays (set up as `rig`) on the reduced field, `None` for a light that takes no shadow.
    pub(crate) fn march(&self, rig: &Rig, light: &Light, draft: bool) -> Option<March<'_>> {
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
        Some(March {
            r,
            src,
            kx: LENS * self.width as f32 / long,
            ky: LENS * self.height as f32 / long,
            z_cut: self.near.exp() * 0.5,
            z_cap: self.far.exp() * 3.0,
            sun_axes,
        })
    }

    /// What blocks light, on the reduced field of the full march (pictures): its width and height, and for each pixel how sure
    /// it is the front of a thing (0..1) and how deep that thing is taken to be in full, as a share of its front's depth
    /// (see [`crate::occluder`]).
    pub fn blocker_plane(&self) -> (usize, usize, Vec<[f32; 2]>) {
        let r = self.reduced(false);
        (r.w, r.h, r.blk.iter().map(|b| [b.s, b.full - 1.0]).collect())
    }

    /// The visibility `vis` at the field's size, 0..1 per field pixel (pictures and tests; the render multiplies it into a lobe).
    pub fn visibility_plane(&self, vis: &Visibility) -> Vec<f32> {
        self.shadowed(Lobe { m: vec![1.0; self.width * self.height] }, vis, 1.0).m
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
        let up = self.upsampled(r, &vis.v, 1.0);
        lobe.m.par_iter_mut().zip(&up).for_each(|(m, s)| *m *= 1.0 - a * (1.0 - s.clamp(0.0, 1.0)));
        lobe
    }

    /// `v` (one value per pixel of the reduced field `r`) at the field's size, edge-aware (see the module docs); a value that is
    /// not a number reads as `fallback`.
    pub(crate) fn upsampled(&self, r: &Reduced, v: &[f32], fallback: f32) -> Vec<f32> {
        let (w, h) = (self.width, self.height);
        let mut out = vec![fallback; w * h];
        if v.len() != r.w * r.h || w == 0 || r.w == 0 || r.h == 0 {
            return out;
        }
        let f = r.f as f32;
        let (is_lum, is_depth) = (1.0 / (2.0 * SIGMA_LUM * SIGMA_LUM), 1.0 / (2.0 * SIGMA_DEPTH * SIGMA_DEPTH));
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
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
                let (mut sum, mut weights, mut nearest) = (0f32, 0f32, (f32::MAX, fallback));
                for (qy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                    for (qx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                        let q = qy * r.w + qx;
                        let (Some(v), Some(g), Some(l)) = (v.get(q), r.gi.get(q), r.lz.get(q)) else { continue };
                        let (dg, dl) = (((gi - g).abs() - DEAD_LUM).max(0.0), ((lz - l).abs() - DEAD_DEPTH).max(0.0));
                        let d = dg * dg * is_lum + dl * dl * is_depth;
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
                *m = if s.is_finite() { s } else { fallback };
            }
        });
        out
    }
}
