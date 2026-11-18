//! Where a light's pattern lands (issue #316, `fork/relight.md`): a projector that throws the pattern's slide onto the
//! surfaces, with the light's opening (a window, or a round gap in a canopy) and the edge blur real light gives.
//!
//! - **Where the projector sits.** A point light (and a softbox, a rim, a fill) throws its pattern from a projector on the
//!   camera's ray through the middle of the light's pool (where its light falls, which moves smoothly with the light),
//!   looking along that ray, from just behind the camera ([`FRONT`]). So the window sits where the light lands, the
//!   pattern falls on the first surface the light reaches (a subject and the wall behind it alike, its stripes straight
//!   across both; the subject's shadow is cut out of it by the shadow march), and there is no side of the scene it
//!   flips to as the light's depth moves. A spot throws it along its
//!   own cone from its own place. A sun's pattern is thrown the same way, centred on its handle: its slanted parallel
//!   rays turned every error of the depth map into a bend of the stripes, and moved a near subject out of the window.
//! - **Where the light comes from.** Light through a window comes from far away, so a patterned point light or softbox
//!   shines from far along its own direction ([`Field::pattern_rig`], [`FAR_SOURCE`]): the window is evenly lit, and
//!   the subject's shadow, marched from there, falls in the light's direction across the pattern.
//! - **Size.** A cell of the pattern is a fixed share of the frame at the scene's median distance from the projector
//!   ([`CELLS`]), so Scale looks the same wherever the light sits; nearer things take smaller cells, farther ones larger,
//!   as a projected slide does.
//! - **Opening.** Blinds and window panes live inside a window ([`WINDOW`]; a softbox's own shape), leaves and dapple
//!   inside a large, soft round gap ([`CANOPY`]); outside it the wall is in shade. A spot's opening is its cone, which
//!   its own light already has.
//! - **Blur.** The pattern sits at a gobo between the light and the surfaces ([`GOBO`] of the median distance from the
//!   projector). An edge is as wide as the light's size seen from the gobo times how far past the gobo the surface lies
//!   (the similar triangles of a penumbra), so an edge is sharp near the gobo and softer on what is farther; the
//!   pattern's Softness and the light's own Softness set the light's size. A surface seen at a grazing angle smears it
//!   wider, and a field pixel's own footprint is the least blur (no moiré on fine stripes far away).

use crate::effects::{Effects, Pattern};
use crate::field::{Field, LENS};
use crate::kinds::Shape;
use crate::light::Rig;

/// Where the projector sits on the camera's ray through the middle of the light's pool: this share of the nearest
/// thing's depth (the field's near depth), just behind the camera. Close to the camera's own centre the pattern hardly
/// shifts with depth, so a depth map's blurred outline doesn't bend the stripes round a subject; a little behind it, a
/// real step in depth still moves them.
pub(crate) const FRONT: f32 = -0.25;
/// The gobo's distance from the projector, a share of the scene's median distance from it.
pub(crate) const GOBO: f32 = 0.3;
/// The least share of the light's size a pattern's edge is blurred by (the penumbra of a surface 1.5 times the gobo's distance
/// from the projector), so nothing near the gobo, a face most of all, takes a razor edge.
pub(crate) const PAST_MIN: f32 = 0.33;
/// Cells across the frame's long side at the scene's median depth at Scale 0 (Scale 1: a tenth of it).
pub(crate) const CELLS: f32 = 34.0;
/// A window's half-width and half-height at the scene's median depth, shares of the frame's long side there.
pub(crate) const WINDOW: (f32, f32) = (0.24, 0.3);
/// A canopy gap's radius (leaves, dapple), the same way, and how much of it fades (a share of the radius).
pub(crate) const CANOPY: (f32, f32) = (0.46, 0.45);
/// How far a patterned lamp's light comes from: at least this many times the depth of the middle of its pool.
pub(crate) const FAR_SOURCE: f32 = 2.0;

/// The opening a pattern lives in, in cells about the projector's axis (in the pattern's turned frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Opening {
    /// None: a spot's cone is its opening.
    None,
    /// A window `2 hx` cells wide and `2 hy` cells high.
    Rect { hx: f32, hy: f32 },
    /// A round gap of radius `r` cells, fading over its outer `feather` cells.
    Round { r: f32, feather: f32 },
}

/// Where a field point lands on the pattern.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    /// The pattern's coordinates, in cells, about the projector's axis.
    pub u: f32,
    pub v: f32,
    /// The edge's half-width there, in cells.
    pub blur: f32,
}

/// A light's projector set up on a field, for one pattern.
#[derive(Clone, Debug)]
pub(crate) struct Projector {
    /// The projector's place.
    origin: [f32; 3],
    axis: [f32; 3],
    e1: [f32; 3],
    e2: [f32; 3],
    /// Cells per unit of tangent.
    freq: f32,
    sin: f32,
    cos: f32,
    /// The gobo's distance from the projector.
    gobo: f32,
    /// The light's radius seen from the gobo (tangent units).
    alpha: f32,
    /// The least distance from the projector a point counts at (a point nearer than that is stretched no further).
    d_min: f32,
    /// A field pixel's size per unit of depth (`LENS / long side`).
    px: f32,
    pub opening: Opening,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    if l.is_finite() && l > 1e-9 { [v[0] / l, v[1] / l, v[2] / l] } else { fallback }
}

/// Two unit vectors across `axis`: the first as near the picture's x as it can be (so a pattern at no turn is level).
fn across(axis: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let x = if axis[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let k = dot(x, axis);
    let e1 = norm([x[0] - k * axis[0], x[1] - k * axis[1], x[2] - k * axis[2]], [1.0, 0.0, 0.0]);
    let e2 = [axis[1] * e1[2] - axis[2] * e1[1], axis[2] * e1[0] - axis[0] * e1[2], axis[0] * e1[1] - axis[1] * e1[0]];
    (e1, norm(e2, [0.0, 1.0, 0.0]))
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    let mid = v.len() / 2;
    let (_, m, _) = v.select_nth_unstable_by(mid, f32::total_cmp);
    Some(*m)
}

/// The middle of where light `rig` falls on the field (the mean of the surface points it lights, weighted by the square of
/// its light, on a coarse grid; the light's own place when it lights nothing). It moves smoothly with the light.
fn pool(field: &Field, rig: &Rig) -> [f32; 3] {
    let (gw, gh) = (48usize, 32usize);
    let (mut sum, mut at) = (0f32, [0f32; 2]);
    for i in 0..gw * gh {
        let (u, v) = (((i % gw) as f32 + 0.5) / gw as f32, ((i / gw) as f32 + 0.5) / gh as f32);
        let (lz, n, _) = field.at(u, v);
        let l = field.light_at(rig, u, v, lz, n);
        let k = if l.is_finite() { l * l } else { 0.0 };
        sum += k;
        at = [at[0] + k * u, at[1] + k * v];
    }
    if !sum.is_finite() || sum <= 1e-12 {
        return rig.position();
    }
    let (u, v) = (at[0] / sum, at[1] / sum);
    let (lz, _, _) = field.at(u, v);
    field.point(u, v, lz)
}

fn unit(v: f64, d: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { d }
}

impl Projector {
    /// The projector of light `rig` for effects `e` on `field`.
    pub(crate) fn new(field: &Field, rig: &Rig, e: &Effects) -> Projector {
        let (w, h) = (field.width, field.height);
        let n = w * h;
        let long = w.max(h).max(1) as f32;
        let pos = rig.position();
        // (a sample of the scene's points: about 4000)
        let step = (n / 4096).max(1);
        let points: Vec<[f32; 3]> = (0..n)
            .step_by(step)
            .map(|i| {
                let (lz, _, _) = field.at_px(i);
                field.point(((i % w.max(1)) as f32 + 0.5) / w.max(1) as f32, ((i / w.max(1)) as f32 + 0.5) / h.max(1) as f32, lz)
            })
            .collect();
        let z_med = median(points.iter().map(|p| p[2]).collect()).unwrap_or(1.0).max(1e-6);
        let cells = CELLS * 10f32.powf(-unit(e.scale, 0.5));
        let cell = LENS * z_med / cells;
        let ps = unit(e.softness, 0.3);
        let size = 0.5 + rig.soft;
        let turn = if e.rotation.is_finite() { (e.rotation % 360.0).to_radians() as f32 } else { 0.0 };
        let (sin, cos) = turn.sin_cos();
        let opening = match (&rig.shape, e.pattern) {
            (Shape::Spot { .. }, _) => Opening::None,
            (_, Pattern::Leaves | Pattern::Dapple) => Opening::Round { r: CANOPY.0 * cells, feather: CANOPY.0 * CANOPY.1 * cells },
            (Shape::Box { hw, hh, .. }, _) => {
                // (a softbox is the window: its shape, at the window's area)
                let aspect = (hw / hh.max(1e-6)).clamp(0.1, 10.0);
                let area = WINDOW.0 * WINDOW.1;
                Opening::Rect { hx: (area * aspect).sqrt() * cells, hy: (area / aspect).sqrt() * cells }
            }
            _ => Opening::Rect { hx: WINDOW.0 * cells, hy: WINDOW.1 * cells },
        };
        let px = LENS / long;
        let (origin, axis) = match rig.shape {
            // a spot throws it along its cone from its place
            Shape::Spot { dir, .. } => (pos, dir),
            _ => {
                // looking at the middle of the light's pool (a sun's: the handle), from in front of everything, near the
                // camera's own axis: so the pattern lands on the first surface the light reaches, a subject as well as
                // the wall behind it, its stripes straight across both (the subject's shadow is cut out of it by the
                // shadow march from the light's own direction); slanted rays would turn every error of the depth map
                // into a bend of the stripes
                let target = if matches!(rig.shape, Shape::Sun { .. }) { pos } else { pool(field, rig) };
                let axis = norm(target, [0.0, 0.0, 1.0]);
                let k = if axis[2] > 1e-6 { FRONT * field.near.exp() / axis[2] } else { 0.0 };
                (axis.map(|c| c * k), axis)
            }
        };
        // (the sun's own disc is 0.0047 radians across its radius; haze and the pattern's Softness widen it)
        let alpha = if matches!(rig.shape, Shape::Sun { .. }) { (0.0047 + 0.06 * ps * ps) * size } else { (0.002 + 0.08 * ps * ps) * size };
        let (e1, e2) = across(axis);
        let d_med = median(points.iter().map(|p| dot(sub(*p, origin), axis)).filter(|d| *d > 0.0).collect()).unwrap_or(z_med).max(1e-6);
        Projector { origin, axis, e1, e2, freq: d_med / cell, sin, cos, gobo: GOBO * d_med, alpha, d_min: 0.05 * d_med, px, opening }
    }

    /// The depth of the middle of the scene along the projector's axis (the median distance's point): where the pattern lands
    /// on the surfaces it is drawn for (issue #317: the air's points are carried there along the light's rays).
    pub(crate) fn plane_depth(&self) -> f32 {
        let z = self.origin[2] + self.axis[2] * self.gobo / GOBO;
        if z.is_finite() && z > 0.0 { z } else { 1.0 }
    }

    /// Where the field point `p` (normal `n`) lands on the pattern.
    #[inline]
    pub(crate) fn sample(&self, p: [f32; 3], n: [f32; 3]) -> Sample {
        let d = sub(p, self.origin);
        let s = dot(d, self.axis);
        let dist = s.max(self.d_min);
        // (a surface nearer than the gobo, the subject's face in front of a far pool, took no penumbra at all and its patches were
        // cut hard: it takes at least the blur of a surface a little past the gobo, issue #346)
        let past = (1.0 - self.gobo / dist).max(PAST_MIN);
        // (in tangent units: the penumbra over the distance, and a pixel's footprint)
        let blur = self.alpha * past + 0.5 * self.px * p[2] / dist;
        let (tx, ty, dir) = (dot(d, self.e1) / dist, dot(d, self.e2) / dist, norm(d, self.axis));
        // (a surface seen at a grazing angle smears the pattern: its edges widen; gently, as the depth's normals are
        // never that sure)
        let incidence = dot(n, dir).abs().clamp(0.35, 1.0).sqrt();
        let (u, v) = ((tx * self.cos - ty * self.sin) * self.freq, (tx * self.sin + ty * self.cos) * self.freq);
        let blur = blur * self.freq / incidence;
        let clean = |x: f32, d: f32| if x.is_finite() { x.clamp(-1e5, 1e5) } else { d };
        Sample { u: clean(u, 1e5), v: clean(v, 1e5), blur: clean(blur, 0.0).max(1e-4) }
    }
}

impl Field {
    /// The light `rig` shines as when it throws pattern `e` (issue #316): a point light or a softbox shines from far along
    /// its own direction from the middle of its pool (at least [`FAR_SOURCE`] times that point's depth away, the same
    /// angular size), like daylight through a window, so the window is evenly lit and its shadows fall the light's way.
    /// `None` for no pattern or another type (a spot is its own projector, a sun is far already, a rim and a fill have no
    /// place). Its lobe and shadows are made from this; the pattern's coordinates from `rig`.
    pub fn pattern_rig(&self, rig: &Rig, e: &Effects) -> Option<Rig> {
        if e.pattern == Pattern::None || !matches!(rig.shape, Shape::Point | Shape::Box { .. }) {
            return None;
        }
        let pool = pool(self, rig);
        let z = pool[2].max(1e-6);
        let (pos, len) = (rig.position(), dot(sub(rig.position(), pool), sub(rig.position(), pool)).sqrt());
        // (towards the light; leaning a little towards the camera, so a light on its own pool still has a direction)
        let towards = norm(pool, [0.0, 0.0, 1.0]);
        let d = sub(pos, pool);
        let dir = norm([d[0] - 0.05 * z * towards[0], d[1] - 0.05 * z * towards[1], d[2] - 0.05 * z * towards[2]], [0.0, 0.0, -1.0]);
        // (an aimed softbox's light travels along its aim, issue #335: the shaft, the pattern's slant and the shadows follow it)
        let dir = match rig.shape {
            Shape::Box { n, .. } if n != [0.0, 0.0, 1.0] => [-n[0], -n[1], -n[2]],
            _ => dir,
        };
        let reach = len.max(FAR_SOURCE * z);
        let mut far = rig.clone();
        far.pos = [pool[0] + dir[0] * reach, pool[1] + dir[1] * reach, pool[2] + dir[2] * reach];
        // (as large as it looked from the pool)
        let grow = reach / len.max(0.2 * z);
        far.r2 = rig.r2 * grow * grow;
        far.shape = Shape::Point;
        far.peak = self.peak_of(&far);
        Some(far)
    }
}
