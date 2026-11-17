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
//! - **Rim**: no place; the near side of depth edges and the surfaces turning away from the camera there.
//! - **Fill**: no place; lifts the shadows, more on near things than far, from the camera's side.
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
    /// The cone's half-angle, in degrees (5..85).
    pub angle: f64,
    /// How soft the cone's edge is, 0 (hard) .. 1 (the whole cone fades).
    pub edge: f64,
}

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
}

/// A [`LightKind::Rim`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rim {
    /// How far the rim reaches into things, 0 (a thin edge) .. 1 (wide).
    pub width: f64,
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

    /// The numbers the kind's lobe depends on, as bits for a cache key (the tag, then its own numbers).
    pub fn key_bits(&self) -> Vec<u64> {
        let b = f64::to_bits;
        match self {
            LightKind::Point => vec![0],
            LightKind::Spot(s) => vec![1, b(s.aim_x), b(s.aim_y), b(s.angle), b(s.edge)],
            LightKind::Sun(s) => vec![2, b(s.azimuth), b(s.elevation)],
            LightKind::Softbox(s) => vec![3, b(s.width), b(s.height), b(s.rotation)],
            LightKind::Rim(r) => vec![4, b(r.width)],
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
    },
    Rim {
        /// Where the surface counts as turned away (the cosine to the camera under which the term is whole), and how
        /// far the depth edge is looked for (in long-side units).
        turned: f32,
        reach: f32,
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
                let outer = finite(s.angle, 5.0, 85.0, 30.0).to_radians();
                let inner = outer * (1.0 - 0.95 * finite(s.edge, 0.0, 1.0, 0.4));
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
                Shape::Box { c: pos, tx: [cos, sin, 0.0], ty: [-sin, cos, 0.0], hw, hh, eps2: 1.5 * (cell(hw) * cell(hw) + cell(hh) * cell(hh)) + 1e-6 }
            }
            LightKind::Rim(r) => {
                let w = finite(r.width, 0.0, 1.0, 0.5) as f32;
                Shape::Rim { turned: 0.15 + 0.35 * w, reach: 0.005 + 0.03 * w }
            }
            LightKind::Fill(f) => Shape::Fill { near: finite(f.near, 0.0, 1.0, 0.5) as f32, top: -1.0 + 3.5 * softness },
        }
    }
}

/// Points across a softbox on a side (`BOX_GRID²` samples per pixel).
const BOX_GRID: usize = 4;

/// The directions the Rim light looks for a farther surface along (eight), turned by half a step on every other ring so
/// the rings don't line up into rays.
const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
const RING: [(f32, f32); 8] = [(1.0, 0.0), (H, H), (0.0, 1.0), (-H, H), (-1.0, 0.0), (-H, -H), (0.0, -1.0), (H, -H)];
/// The rings' radii as shares of the reach.
const RINGS: [f32; 3] = [0.34, 0.67, 1.0];

impl Shape {
    /// Whether the shape is made for a fixed peak (1: a fully lit point) and not the photo's most lit point: a sun, a
    /// rim and a fill have no distance, so the Amount is what a surface that fully faces them gets; normalizing to the
    /// photo's best would light everything alike when nothing faces them well.
    pub(crate) fn fixed_peak(&self) -> bool {
        matches!(self, Shape::Sun { .. } | Shape::Rim { .. } | Shape::Fill { .. })
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
        Shape::Box { c, tx, ty, hw, hh, eps2 } => {
            let mut sum = 0.0f32;
            for j in 0..BOX_GRID {
                let b = ((j as f32 + 0.5) / BOX_GRID as f32 * 2.0 - 1.0) * hh;
                for i in 0..BOX_GRID {
                    let a = ((i as f32 + 0.5) / BOX_GRID as f32 * 2.0 - 1.0) * hw;
                    let s = [c[0] + tx[0] * a + ty[0] * b, c[1] + tx[1] * a + ty[1] * b, c[2]];
                    let d = [s[0] - p[0], s[1] - p[1], s[2] - p[2]];
                    let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    let r = r2.sqrt().max(1e-9);
                    // (the box faces into the scene: a surface in front of it gets nothing from it)
                    let emit = (-d[2] / r).max(0.0);
                    sum += facing(rig.face, rig.wrap, dot(n, d) / r) * emit * (eps2 / (eps2 + r2));
                }
            }
            sum / (BOX_GRID * BOX_GRID) as f32
        }
        Shape::Rim { turned, reach } => {
            // how far the surface turns from the camera (1 face on, 0 edge on)
            let c = -dot(n, norm(p, [0.0, 0.0, 1.0]));
            let away = 1.0 - smooth(0.05, *turned, c);
            // whether it is the near side of a step in depth: the largest rise in log depth within the reach
            let long = field.width.max(field.height).max(1) as f32;
            let (ru, rv) = (reach / (field.width.max(1) as f32 / long), reach / (field.height.max(1) as f32 / long));
            // (a step counts for more the nearer it is)
            let mut edge = 0.0f32;
            for (k, r) in RINGS.iter().enumerate() {
                let (sin, cos) = if k % 2 == 1 { (0.382_683_43f32, 0.923_879_5f32) } else { (0.0, 1.0) };
                for (dx, dy) in RING {
                    let (dx, dy) = (dx * cos - dy * sin, dx * sin + dy * cos);
                    let (slz, _, _) = field.at((u + dx * ru * r).clamp(0.0, 1.0), (v + dy * rv * r).clamp(0.0, 1.0));
                    edge = edge.max(smooth(0.12, 0.45, slz - lz) * (1.0 - 0.7 * r));
                }
            }
            edge * (0.1 + 0.9 * away)
        }
        Shape::Fill { near, top } => {
            let (_, _, lum) = field.at(u, v);
            let span = (field.far - field.near).max(1e-3);
            let nearness = 1.0 - ((lz - field.near) / span).clamp(0.0, 1.0);
            let by_depth = (1.0 - near) + near * nearness;
            let shadow = 1.0 - smooth(-6.0, *top, lum);
            let c = (-dot(n, norm(p, [0.0, 0.0, 1.0]))).clamp(0.0, 1.0);
            by_depth * shadow * (1.0 - rig.face + rig.face * c)
        }
    };
    if value.is_finite() { value.max(0.0) } else { 0.0 }
}
