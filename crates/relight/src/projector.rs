//! Where a light's pattern lands (issue #316, `fork/relight.md`): a projector that throws the pattern's slide onto the
//! surfaces, with the light's opening (a window, or a round gap in a canopy) and the edge blur real light gives.
//!
//! - **Where the projector sits.** A point light (and a softbox, a rim, a fill) throws its pattern from a projector on the
//!   camera's ray through the middle of the light's pool (where its light falls, which moves smoothly with the light),
//!   looking along that ray, from in front of everything the photo shows (behind the camera, [`FRONT`], like a window
//!   behind the photographer). So the window sits where the light lands, the pattern falls on everything there, and
//!   there is no side of the scene it flips to as the light's depth moves. A spot throws it along its
//!   own cone from its own place; a sun throws it with parallel rays along its direction (stripes from one window stay
//!   parallel; they don't converge on a point).
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

/// Where the projector sits on the camera's ray through the light: this share of the nearest thing's depth (the field's
/// near depth); below 0 it is behind the camera, like a window behind the photographer, so a near thing takes cells
/// about half the size of the scene's middle (closer would cut a near subject's pattern off at its outline).
pub(crate) const FRONT: f32 = -0.5;
/// The gobo's distance from the projector, a share of the scene's median distance from it.
pub(crate) const GOBO: f32 = 0.3;
/// Cells across the frame's long side at the scene's median depth at Scale 0 (Scale 1: a tenth of it).
pub(crate) const CELLS: f32 = 34.0;
/// A window's half-width and half-height at the scene's median depth, shares of the frame's long side there.
pub(crate) const WINDOW: (f32, f32) = (0.24, 0.3);
/// A canopy gap's radius (leaves, dapple), the same way, and how much of it fades (a share of the radius).
pub(crate) const CANOPY: (f32, f32) = (0.46, 0.45);
/// For a sun (parallel rays): the window's distance in front of the handle's point, a share of the median depth.
const SUN_GOBO: f32 = 0.5;

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
    /// Parallel rays (a sun): `origin` is the point the window's middle lands on, else the projector's place.
    parallel: bool,
    origin: [f32; 3],
    axis: [f32; 3],
    e1: [f32; 3],
    e2: [f32; 3],
    /// Cells per unit of tangent (a projector) or of length (parallel rays).
    freq: f32,
    sin: f32,
    cos: f32,
    /// The gobo's distance from the projector (or, for parallel rays, in front of `origin`).
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
        if let Shape::Sun { to, .. } = rig.shape {
            // parallel rays travel away from the sun; the window's middle lands on the handle's ray at the median depth
            let axis = norm([-to[0], -to[1], -to[2]], [0.0, 0.0, 1.0]);
            let (e1, e2) = across(axis);
            let origin = if pos[2] > 1e-9 { pos.map(|c| c * z_med / pos[2]) } else { [0.0, 0.0, z_med] };
            return Projector {
                parallel: true,
                origin,
                axis,
                e1,
                e2,
                freq: 1.0 / cell,
                sin,
                cos,
                gobo: SUN_GOBO * z_med,
                // (the sun's own disc is 0.0047 radians across its radius; haze and the pattern's Softness widen it)
                alpha: (0.0047 + 0.08 * ps * ps) * size,
                d_min: 0.0,
                px,
                opening,
            };
        }
        let (origin, axis) = match rig.shape {
            // a spot throws it along its cone from its place
            Shape::Spot { dir, .. } => (pos, dir),
            _ => {
                // looking at the middle of the light's pool, from in front of everything (whatever the light's depth)
                let axis = norm(pool(field, rig), [0.0, 0.0, 1.0]);
                let k = if axis[2] > 1e-6 { FRONT * field.near.exp() / axis[2] } else { 0.0 };
                (axis.map(|c| c * k), axis)
            }
        };
        let (e1, e2) = across(axis);
        let d_med = median(points.iter().map(|p| dot(sub(*p, origin), axis)).filter(|d| *d > 0.0).collect()).unwrap_or(z_med).max(1e-6);
        Projector {
            parallel: false,
            origin,
            axis,
            e1,
            e2,
            freq: d_med / cell,
            sin,
            cos,
            gobo: GOBO * d_med,
            alpha: (0.002 + 0.08 * ps * ps) * size,
            d_min: 0.05 * d_med,
            px,
            opening,
        }
    }

    /// Where the field point `p` (normal `n`) lands on the pattern.
    #[inline]
    pub(crate) fn sample(&self, p: [f32; 3], n: [f32; 3]) -> Sample {
        let d = sub(p, self.origin);
        let s = dot(d, self.axis);
        let (tx, ty, blur, dir) = if self.parallel {
            let past = (s + self.gobo).max(0.0);
            (dot(d, self.e1), dot(d, self.e2), self.alpha * past + 0.5 * self.px * p[2], self.axis)
        } else {
            let dist = s.max(self.d_min);
            let past = (1.0 - self.gobo / dist).max(0.0);
            // (in tangent units: the penumbra over the distance, and a pixel's footprint)
            let blur = self.alpha * past + 0.5 * self.px * p[2] / dist;
            (dot(d, self.e1) / dist, dot(d, self.e2) / dist, blur, norm(d, self.axis))
        };
        // (a surface seen at a grazing angle smears the pattern: its edges widen; gently, as the depth's normals are
        // never that sure)
        let incidence = dot(n, dir).abs().clamp(0.35, 1.0).sqrt();
        let (u, v) = ((tx * self.cos - ty * self.sin) * self.freq, (tx * self.sin + ty * self.cos) * self.freq);
        let blur = blur * self.freq / incidence;
        let clean = |x: f32, d: f32| if x.is_finite() { x.clamp(-1e5, 1e5) } else { d };
        Sample { u: clean(u, 1e5), v: clean(v, 1e5), blur: clean(blur, 0.0).max(1e-4) }
    }
}
