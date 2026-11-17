//! Light types (issue #300): what a light is besides a point. Each type changes how a pixel's `falloff · facing` (the
//! number a [`Lobe`](crate::Lobe) holds before it is normalized by the photo's most lit point) is made; everything
//! after it (the lobe, Amount, Warmth, the roll-off, the colour planes, the ⌥ view) is shared with the point light, so
//! every type adds up with the others and renders alike on the CPU and the GPU.
//!
//! - **Point**: today's light ([`crate::light`]; nothing here touches it).
//! - **Spot**: the point light seen through a cone: a place, an aim (a point on the photo; the cone points at the
//!   surface there), a half-angle and an edge softness.
//! - **Sun**: light from one direction (azimuth round the scene's vertical axis, elevation above its horizon), no fall-off
//!   with distance; a surface gets what it faces.
//! - **Softbox**: a rectangle on the photo (place, size as shares of the photo's width and height, rotation) at the
//!   light's depth, facing into the scene; each pixel sums the light of a grid of points across it, so a bigger box
//!   is softer and wraps more round shapes.
//! - **Rim**: no place; a thin bright edge along the near side of depth steps (the subject's outline), from one side or
//!   all round ([`crate::rim`]).
//! - **Fill**: no place; lifts the shadows (by the luminance the photo shows, its exposure included), more on near things
//!   than far, from the camera's side.
//!
//! Positions are normalized photo coordinates, as the point light's. Every number is read leniently: not a number is
//! its default, the rest is clamped to its range.

use crate::field::{Field, LENS};
use crate::light::{Rig, facing, point_term};

/// The cone of a [`LightKind::Spot`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// What it points at, in normalized photo coordinates (the surface there; may lie a little outside the photo).
    pub aim_x: f64,
    pub aim_y: f64,
    /// The cone's half-angle, in degrees ([`MIN_ANGLE`]..85: a stage fixture's narrow beam to a wide flood).
    pub angle: f64,
    /// How soft the cone's edge is, 0 (hard) .. 1 (the whole cone fades).
    pub edge: f64,
    /// A sharp-edged cone (issue #334, a stage beam): the edge is a thin fixed ramp (a tenth of the half-angle) whatever `edge` says.
    pub sharp: bool,
}

/// The narrowest cone a spot makes, its half-angle in degrees (issue #334: a stage beam is a few degrees; it was 5).
pub const MIN_ANGLE: f64 = 2.0;
/// How much of a sharp cone's half-angle its edge ramps over (a tenth: the depth map's noise moves where a surface sits by a few pixels, and a thinner ramp turned a pool on a face into a jagged shape).
const SHARP_RAMP: f64 = 0.1;

/// The direction of a [`LightKind::Sun`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    /// Where it shines from round the scene's vertical axis, in degrees: 0 from the camera's side (front light),
    /// 90 from the right, 180 from behind the scene, −90 from the left.
    pub azimuth: f64,
    /// How high it is, in degrees: 0 on the horizon, 90 straight above.
    pub elevation: f64,
}

/// The rectangle of a [`LightKind::Softbox`], at the light's place and depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Softbox {
    /// Its width as a share of the photo's width and its height as a share of the photo's height (before rotation).
    pub width: f64,
    pub height: f64,
    /// Rotation in the photo's plane, in degrees (clockwise as the photo shows it).
    pub rotation: f64,
    /// Where it shines (issue #335), as two tilts from straight into the scene, in degrees (−80..80): `aim_h` towards the
    /// photo's right (negative: left), `aim_v` towards its bottom (negative: up). 0 and 0 is the straight-in softbox every
    /// edit saved before this has; the rectangle turns with the aim, and the light it gives narrows towards it.
    pub aim_h: f64,
    pub aim_v: f64,
}

/// A [`LightKind::Rim`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rim {
    /// How far the rim reaches into things, 0 (a thin edge) .. 1 (wide).
    pub width: f64,
    /// The side of the photo the light comes from, in degrees: 0 from above, 90 from the right, ±180 from below, −90
    /// from the left. It counts as far as `one_sided` does.
    pub angle: f64,
    /// How one-sided the light is, 0 (the whole outline) .. 1 (only the side that faces `angle`).
    pub one_sided: f64,
}

impl Default for Rim {
    fn default() -> Self {
        Rim { width: 0.5, angle: 90.0, one_sided: 0.0 }
    }
}

/// A [`LightKind::Fill`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    /// How much more near things get than far ones, 0 (alike) .. 1 (only the nearest).
    pub near: f64,
}

/// What a light is. See the module docs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LightKind {
    #[default]
    Point,
    Spot(Spot),
    Sun(Sun),
    Softbox(Softbox),
    Rim(Rim),
    Fill(Fill),
}

impl LightKind {
    /// Whether it has a place on the photo (a handle; the position `x`, `y` counts).
    pub fn has_place(&self) -> bool {
        !matches!(self, LightKind::Rim(_) | LightKind::Fill(_))
    }

    /// Whether its source can be seen as a bright point where the light is placed (issue #334): a Sun shines from a direction,
    /// not from its handle's place, so a glow there would sit where the light doesn't come from.
    pub fn shows_source(&self) -> bool {
        self.has_place() && !matches!(self, LightKind::Sun(_))
    }

    /// The numbers the kind's lobe depends on, as bits for a cache key (the tag, then its own numbers).
    pub fn key_bits(&self) -> Vec<u64> {
        let b = f64::to_bits;
        match self {
            LightKind::Point => vec![0],
            LightKind::Spot(s) => {
                let mut v = vec![1, b(s.aim_x), b(s.aim_y), b(s.angle), b(s.edge)];
                // (a soft cone hashes as it did before sharp ones)
                if s.sharp {
                    v.push(1);
                }
                v
            }
            LightKind::Sun(s) => vec![2, b(s.azimuth), b(s.elevation)],
            LightKind::Softbox(s) if s.aim_h == 0.0 && s.aim_v == 0.0 => vec![3, b(s.width), b(s.height), b(s.rotation)],
            LightKind::Softbox(s) => vec![3, b(s.width), b(s.height), b(s.rotation), b(s.aim_h), b(s.aim_v)],
            LightKind::Rim(r) => vec![4, b(r.width), b(r.angle), b(r.one_sided)],
            LightKind::Fill(f) => vec![5, b(f.near)],
        }
    }
}

/// A kind set up on a field: the numbers every pixel needs.
#[derive(Clone, Debug)]
pub(crate) enum Shape {
    Point,
    Spot {
        /// Unit direction of the cone's axis, and the cosines of its outer and inner edge.
        dir: [f32; 3],
        cos_outer: f32,
        cos_inner: f32,
    },
    Sun {
        /// Unit vector towards the sun, and the exponent of the facing term.
        to: [f32; 3],
        sharp: f32,
    },
    Box {
        c: [f32; 3],
        tx: [f32; 3],
        ty: [f32; 3],
        hw: f32,
        hh: f32,
        /// The squared radius each sample of the box shines as (keeps a pixel on the box finite).
        eps2: f32,
        /// The way it shines (a unit vector; `[0, 0, 1]` straight into the scene) and how narrow its light is about it
        /// (the exponent of the cosine off the axis: 1 for the straight-in box, more the more it is turned, issue #335).
        n: [f32; 3],
        focus: f32,
    },
    Rim {
        /// The rim over the field, made when a pixel first asks for it ([`crate::rim`]).
        rim: std::sync::Arc<crate::rim::RimShape>,
    },
    Fill {
        near: f32,
        /// The log2 luminance under which a pixel is in shadow for it (a pixel above is left alone).
        top: f32,
    },
}

fn finite(v: f64, lo: f64, hi: f64, default: f64) -> f64 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

/// 0 below `e0`, 1 above `e1`, smooth between; a step when the edges meet (never NaN).
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 - e0 < 1e-5 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn norm(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l.is_finite() && l > 1e-9 { [v[0] / l, v[1] / l, v[2] / l] } else { fallback }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The direction a softbox with tilts `aim_h` and `aim_v` (degrees) shines in, in camera space: `[0, 0, 1]` for no tilt.
pub fn box_normal(aim_h: f64, aim_v: f64) -> [f32; 3] {
    let t = |v: f64| finite(v, -AIM_MAX, AIM_MAX, 0.0).to_radians().tan() as f32;
    norm([t(aim_h), t(aim_v), 1.0], [0.0, 0.0, 1.0])
}

/// The farthest a softbox is turned from straight in (degrees, each way).
pub const AIM_MAX: f64 = 80.0;

/// `v` turned the way that takes `[0, 0, 1]` to unit vector `n` (the shortest turn).
fn tilted(v: [f32; 3], n: [f32; 3]) -> [f32; 3] {
    let a = [-n[1], n[0], 0.0];
    let cross = |u: [f32; 3], w: [f32; 3]| [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
    let (av, aav) = (cross(a, v), cross(a, cross(a, v)));
    let k = 1.0 / (1.0 + n[2]).max(1e-3);
    [v[0] + av[0] + aav[0] * k, v[1] + av[1] + aav[1] * k, v[2] + av[2] + aav[2] * k]
}

/// How much of a softbox's light leaves towards the point `-d/r` of the scene (`d` the way to the box's sample, `r` its
/// length): the cosine off the box's axis, to the power `focus`. For the straight-in box that is `-d.z / r`, as it
/// always was.
#[inline]
fn box_emit(n: [f32; 3], focus: f32, d: [f32; 3], r: f32) -> f32 {
    let e = (-dot(n, d) / r).max(0.0);
    if focus == 1.0 { e } else { e.powf(focus) }
}

/// The sun's direction towards it in camera space (x right, y down, z away from the camera) for `azimuth` and
/// `elevation` in degrees.
pub fn sun_vector(azimuth: f64, elevation: f64) -> [f32; 3] {
    let az = finite(azimuth, -720.0, 720.0, 0.0).to_radians();
    let el = finite(elevation, 0.0, 90.0, 35.0).to_radians();
    [(el.cos() * az.sin()) as f32, -(el.sin()) as f32, -(el.cos() * az.cos()) as f32]
}

impl Shape {
    /// `kind` set up on `field` for a light at camera-space point `pos` (log depth `lz`).
    pub(crate) fn new(kind: &LightKind, field: &Field, pos: [f32; 3], lz: f32, softness: f32) -> Shape {
        match kind {
            LightKind::Point => Shape::Point,
            LightKind::Spot(s) => {
                let (ax, ay) = (finite(s.aim_x, -0.5, 1.5, 0.5) as f32, finite(s.aim_y, -0.5, 1.5, 0.6) as f32);
                let (alz, _, _) = field.at(ax.clamp(0.0, 1.0), ay.clamp(0.0, 1.0));
                let aim = field.point(ax, ay, alz);
                let dir = norm([aim[0] - pos[0], aim[1] - pos[1], aim[2] - pos[2]], [0.0, 0.0, 1.0]);
                let outer = finite(s.angle, MIN_ANGLE, 85.0, 30.0).to_radians();
                let inner = outer * if s.sharp { 1.0 - SHARP_RAMP } else { 1.0 - 0.95 * finite(s.edge, 0.0, 1.0, 0.4) };
                Shape::Spot { dir, cos_outer: outer.cos() as f32, cos_inner: inner.cos() as f32 }
            }
            LightKind::Sun(s) => {
                // (the softer, the gentler the terminator)
                Shape::Sun { to: sun_vector(s.azimuth, s.elevation), sharp: 1.0 + 2.0 * (1.0 - softness) }
            }
            LightKind::Softbox(b) => {
                let (w, h) = (finite(b.width, 0.02, 1.5, 0.3), finite(b.height, 0.02, 1.5, 0.2));
                let rot = finite(b.rotation, -720.0, 720.0, 0.0).to_radians();
                let (sin, cos) = (rot.sin() as f32, rot.cos() as f32);
                let long = field.width.max(field.height).max(1) as f32;
                let z = lz.exp();
                // (the rectangle as the photo shows it at the light's depth, turned in the photo's plane)
                let hw = 0.5 * w as f32 * field.width as f32 / long * LENS * z;
                let hh = 0.5 * h as f32 * field.height as f32 / long * LENS * z;
                let cell = |half: f32| half / BOX_GRID as f32;
                // (the rectangle turns with its aim; an unaimed one is left exactly as it always was)
                let n = box_normal(b.aim_h, b.aim_v);
                let (mut tx, mut ty) = ([cos, sin, 0.0], [-sin, cos, 0.0]);
                if n != [0.0, 0.0, 1.0] {
                    (tx, ty) = (tilted(tx, n), tilted(ty, n));
                }
                Shape::Box {
                    c: pos,
                    tx,
                    ty,
                    hw,
                    hh,
                    eps2: 1.5 * (cell(hw) * cell(hw) + cell(hh) * cell(hh)) + 1e-6,
                    n,
                    focus: 1.0 + FOCUS * (1.0 - n[2]),
                }
            }
            LightKind::Rim(r) => {
                let w = finite(r.width, 0.0, 1.0, 0.5) as f32;
                let angle = finite(r.angle, -720.0, 720.0, 90.0).to_radians();
                let setup = crate::rim::Setup {
                    reach: 0.004 + 0.036 * w,
                    turned: 0.15 + 0.35 * w,
                    from: [angle.sin() as f32, -(angle.cos()) as f32],
                    one_sided: finite(r.one_sided, 0.0, 1.0, 0.0) as f32,
                };
                Shape::Rim { rim: std::sync::Arc::new(crate::rim::RimShape::new(setup)) }
            }
            LightKind::Fill(f) => Shape::Fill { near: finite(f.near, 0.0, 1.0, 0.5) as f32, top: 0.5 + 2.0 * softness },
        }
    }
}

/// Points across a softbox on a side (`BOX_GRID²` samples per pixel).
const BOX_GRID: usize = 4;
#[cfg(test)]
pub(crate) const BOX_GRID_TEST: usize = BOX_GRID;

/// How much narrower an aimed softbox's light gets with its turn: its cosine's exponent is `1 + FOCUS · (1 − cos turn)`
/// (a box turned 45° gives 2.2, one turned 80°, 4.3).
const FOCUS: f32 = 4.0;

/// How much of a pixel's light a fill gives at full Amount: under half the point light's, so the Amount reads about as
/// strongly as the other types' (a fill spreads over the whole shadow, a point over a pool: at full Amount about 1.5 stops
/// in the deepest shadows of a flat, face-on wall).
const FILL_GAIN: f32 = 0.4;
impl Shape {
    /// Whether the shape is made for a fixed peak (1: a fully lit point) and not the photo's most lit point: a sun, a
    /// rim and a fill have no distance, so the Amount is what a surface that fully faces them gets; normalizing to the
    /// photo's best would light everything alike when nothing faces them well.
    pub(crate) fn fixed_peak(&self) -> bool {
        matches!(self, Shape::Sun { .. } | Shape::Rim { .. } | Shape::Fill { .. })
    }
}

impl Shape {
    /// Make what a light of this shape needs before its pixels are asked for in parallel (a Rim's map: built once here, with
    /// all the threads, not by whichever pixel asks first).
    pub(crate) fn prepare(&self, field: &Field) {
        if let Shape::Rim { rim } = self {
            rim.map(field);
        }
    }
}

/// `falloff · facing` at `(u, v)` (log depth `lz`, normal `n`) for a light that isn't a point. Always finite.
pub(crate) fn light_at(field: &Field, rig: &Rig, (u, v): (f32, f32), lz: f32, n: [f32; 3]) -> f32 {
    let p = field.point(u, v, lz);
    let value = match &rig.shape {
        Shape::Point => point_term(rig, p, n),
        Shape::Spot { dir, cos_outer, cos_inner } => {
            let from = norm([p[0] - rig.pos[0], p[1] - rig.pos[1], p[2] - rig.pos[2]], *dir);
            let inside = smooth(*cos_outer, *cos_inner, dot(from, *dir));
            point_term(rig, p, n) * inside
        }
        Shape::Sun { to, sharp } => {
            let t = ((dot(n, *to) + rig.wrap) / (1.0 + rig.wrap)).clamp(0.0, 1.0);
            1.0 - rig.face + rig.face * t.powf(*sharp)
        }
        Shape::Box { c, tx, ty, hw, hh, eps2, n: bn, focus } => {
            let mut sum = 0.0f32;
            for j in 0..BOX_GRID {
                let b = ((j as f32 + 0.5) / BOX_GRID as f32 * 2.0 - 1.0) * hh;
                for i in 0..BOX_GRID {
                    let a = ((i as f32 + 0.5) / BOX_GRID as f32 * 2.0 - 1.0) * hw;
                    let s = [c[0] + tx[0] * a + ty[0] * b, c[1] + tx[1] * a + ty[1] * b, c[2] + tx[2] * a + ty[2] * b];
                    let d = [s[0] - p[0], s[1] - p[1], s[2] - p[2]];
                    let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    let r = r2.sqrt().max(1e-9);
                    // (the box faces into the scene: a surface in front of it gets nothing from it)
                    let emit = box_emit(*bn, *focus, d, r);
                    sum += facing(rig.face, rig.wrap, dot(n, d) / r) * emit * (eps2 / (eps2 + r2));
                }
            }
            sum / (BOX_GRID * BOX_GRID) as f32
        }
        // (the map holds all of it: the depth edge, how the surface turns, which side)
        Shape::Rim { rim } => rim.map(field).at(u, v),
        Shape::Fill { near, top } => {
            let (_, _, lum) = field.at(u, v);
            // (the pixel's luminance as the photo shows it: the guide's plus the photo's exposure, as the grade reads it)
            let l = lum + rig.ev;
            let span = (field.far - field.near).max(1e-3);
            let nearness = 1.0 - ((lz - field.near) / span).clamp(0.0, 1.0);
            let by_depth = (1.0 - near) + near * nearness;
            // (full in the deepest shadows, fading out linearly to nothing at `top`, a little over mid-grey)
            let shadow = 1.0 - ((l + 6.0) / (top + 6.0)).clamp(0.0, 1.0);
            let c = (-dot(n, norm(p, [0.0, 0.0, 1.0]))).clamp(0.0, 1.0);
            FILL_GAIN * by_depth * shadow * (1.0 - rig.face + rig.face * c)
        }
    };
    if value.is_finite() { value.max(0.0) } else { 0.0 }
}

/// The light a point of the air gets rolls off towards this, as a share of the photo's most lit surface's: the air right at
/// a small light would else take any number (a bulb's core is white, not infinite). Soft and high, so a spot's beam is still
/// brightest at its source and fades along its axis (round 3: a hard cap of 3 made the near part of a beam flat).
const AIR_CAP: f32 = 200.0;

/// How much of `rig`'s light reaches camera-space point `q` of the air (issue #317, [`crate::beam`]): the light's
/// fall-off (its cone for a spot, the box's points for a softbox, all of it for a sun) with no facing term (air faces
/// every way), as a share of the photo's most lit surface's light ([`Rig`]'s peak), rolling off towards [`AIR_CAP`]. A rim and a fill
/// light no air. Always finite.
pub(crate) fn air_light(rig: &Rig, q: [f32; 3]) -> f32 {
    // (the air takes the light's fall-off as an inverse square, the law of a light in the open; the surfaces' steeper `tail`
    // is a look for tight pools, and in the air it made a spot's beam fade out right at its source. It is scaled so it meets
    // the surfaces' fall-off at the photo's most lit point: `peak^(1/tail)` is that point's inverse square)
    let falloff = |pos: [f32; 3]| {
        let d = [pos[0] - q[0], pos[1] - q[1], pos[2] - q[2]];
        rig.r2 / (rig.r2 + dot(d, d))
    };
    let point_peak = rig.peak().max(1e-12).powf(1.0 / rig.tail.max(1e-3));
    let value = match &rig.shape {
        Shape::Point => falloff(rig.pos) * rig.peak() / point_peak,
        Shape::Spot { dir, cos_outer, cos_inner } => {
            let from = norm([q[0] - rig.pos[0], q[1] - rig.pos[1], q[2] - rig.pos[2]], *dir);
            falloff(rig.pos) * smooth(*cos_outer, *cos_inner, dot(from, *dir)) * rig.peak() / point_peak
        }
        Shape::Sun { .. } => 1.0,
        Shape::Box { c, tx, ty, hw, hh, eps2, n: bn, focus } => {
            let mut sum = 0.0f32;
            for j in 0..BOX_GRID {
                let b = ((j as f32 + 0.5) / BOX_GRID as f32 * 2.0 - 1.0) * hh;
                for i in 0..BOX_GRID {
                    let a = ((i as f32 + 0.5) / BOX_GRID as f32 * 2.0 - 1.0) * hw;
                    let d = [c[0] + tx[0] * a + ty[0] * b - q[0], c[1] + tx[1] * a + ty[1] * b - q[1], c[2] + tx[2] * a + ty[2] * b - q[2]];
                    let r2 = dot(d, d);
                    // (the box shines into the scene, as on the surfaces)
                    let emit = box_emit(*bn, *focus, d, r2.sqrt().max(1e-9));
                    sum += emit * (eps2 / (eps2 + r2));
                }
            }
            sum / (BOX_GRID * BOX_GRID) as f32
        }
        Shape::Rim { .. } | Shape::Fill { .. } => 0.0,
    } / rig.peak();
    let value = value.max(0.0);
    let soft = value / (1.0 + value / AIR_CAP);
    if soft.is_finite() { soft } else { 0.0 }
}

/// Where the light of `rig` travels at point `q`, a unit vector (from the light's place; a sun's is the same everywhere).
pub(crate) fn travel(rig: &Rig, q: [f32; 3]) -> [f32; 3] {
    match &rig.shape {
        Shape::Sun { to, .. } => [-to[0], -to[1], -to[2]],
        _ => norm([q[0] - rig.pos[0], q[1] - rig.pos[1], q[2] - rig.pos[2]], [0.0, 0.0, -1.0]),
    }
}
