//! What each Face Reshape slider moves: the kernels ([`kernels`]) the sampled warp ([`super::Field`]) adds up.
//!
//! Units: points are in *photo units* (see [`super::field`]).

use super::Reshape;

/// Where the face mesh starts in [`crate::Face::landmarks`]: after YuNet's five.
pub const MESH: usize = 5;
/// The face mesh's points (MediaPipe's order: 468 face points, then 5 per iris).
pub const MESH_POINTS: usize = 478;

/// How far each slider moves the face at ±100: a fraction of the face's width (`W`) or height (`H`).
const SLIM: f64 = 0.05; // the cheeks and the jaw's sides, each, of W
const JAW: f64 = 0.045; // the jaw's corners, each, of W
const CHIN: f64 = 0.07; // the chin, of H
const DOUBLE_CHIN: f64 = 0.06; // the soft area under the jaw line, toward it, of H
const NOSE: f64 = 0.18; // the lower nose's width, × (1 ± this)
const FOREHEAD: f64 = 0.07; // the top of the forehead, of H
const EYES: f64 = 0.18; // each eye's size, × (1 ± this)

/// The radius each move fades out at: a fraction of the face's width, or for the eyes of the eye's half width.
const SLIM_R: f64 = 0.28;
const JAW_R: f64 = 0.22;
const CHIN_R: f64 = 0.22;
const FOREHEAD_R: f64 = 0.32;

/// Nose: the part narrowed or widened evenly, an ellipse about the middle of the nose's wings: across, this many
/// times the wings' half width, narrowing upward by [`NOSE_TAPER`] as the bridge does; up, this share of the way
/// to the top of the bridge; down, to the base of the nose. The change fades out from it to [`NOSE_OUTER`] times
/// its size: below the eyes' inner corners and the glasses' bridge above, before the upper lip below and the
/// mouth's corners beside.
const NOSE_ACROSS: f64 = 1.15;
const NOSE_TAPER: f64 = 0.4;
const NOSE_UP: f64 = 0.45;
const NOSE_DOWN: f64 = 1.0;
const NOSE_OUTER: f64 = 1.9;
/// Eye Size: the part scaled evenly, an ellipse about the eye's corners: along the eye, this many times its half
/// width; above and below, this many half widths (the lids and lashes). The change fades out from it to
/// [`EYES_OUTER`] times its size, below the brows.
const EYES_ALONG: f64 = 1.15;
const EYES_ACROSS: f64 = 0.6;
const EYES_OUTER: f64 = 1.85;
/// Double Chin: the band under the jaw line it pulls toward the line, fractions of H: nothing at the line, the full
/// pull from [`TUCK_NEAR`] below it, fading out by [`TUCK_FAR`]; along the jaw each pull spreads over
/// [`TUCK_SIDE`] times the spacing of the jaw line's points.
const TUCK_NEAR: f64 = 0.07;
const TUCK_FAR: f64 = 0.28;
const TUCK_SIDE: f64 = 2.0;
/// Double Chin pulls the jaw's corners this share of the pull under the chin, rising smoothly toward the chin.
const TUCK_CORNERS: f64 = 0.35;
/// How far above the mesh's top (mid forehead) Forehead's moves sit, a fraction of H: nearer the hairline, so the
/// brows stay.
const ABOVE_FOREHEAD: f64 = 0.04;

/// Mesh points (MediaPipe face mesh indices). "Right" is the subject's: on the left of a frontal photo.
mod point {
    pub const TOP: usize = 10;
    pub const CHIN: usize = 152;
    pub const RIGHT_CHEEK: usize = 234;
    pub const LEFT_CHEEK: usize = 454;
    /// The subject's right eye's outer and inner corners, then the left eye's inner and outer.
    pub const EYE_CORNERS: [usize; 4] = [33, 133, 362, 263];
    /// The face's contour on each side, cheekbone to jaw corner (Slim Face).
    pub const CHEEKS: [[usize; 4]; 2] = [[234, 93, 132, 58], [454, 323, 361, 288]];
    /// The jaw's corners (Jaw).
    pub const JAW: [[usize; 3]; 2] = [[172, 136, 150], [397, 365, 379]];
    /// The chin (Chin).
    pub const CHIN_LINE: [usize; 3] = [152, 148, 377];
    /// The jaw line from one corner under the chin to the other (Double Chin).
    pub const JAW_LINE: [usize; 13] = [172, 136, 150, 149, 176, 148, 152, 377, 400, 378, 379, 365, 397];
    /// The nose's wings at their widest, the top of its bridge (between the eyes) and its base (Nose).
    pub const NOSE_WINGS: [usize; 2] = [129, 358];
    pub const NOSE_TOP: usize = 168;
    pub const NOSE_BASE: usize = 2;
    /// The top of the face's contour (Forehead).
    pub const FOREHEAD: [usize; 3] = [10, 109, 338];
}

/// One move of a slider: around `centre`, nothing past `radius`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kernel {
    pub centre: [f64; 2],
    pub radius: f64,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// The pixels at the centre move by this (photo units).
    Move([f64; 2]),
    /// The pixels inside an ellipse about the centre are scaled about it by `by` (along `axes[0]`, along
    /// `axes[1]`; 1 leaves them), evenly all over it, so its shape and texture are kept. `half`: the ellipse's half
    /// size along `axes[0]`, and along `axes[1]` before the centre and past it; before the centre its size along
    /// `axes[0]` narrows smoothly by up to `taper` (a share). Outside it the change fades out smoothly (C²), to
    /// nothing at `outer` times its size.
    Zoom { axes: [[f64; 2]; 2], half: [f64; 3], taper: f64, outer: f64, by: [f64; 2] },
    /// The pixels on the far side of a line through the centre (along `normal` from it) are pulled toward the line
    /// by `lift` (photo units): nothing at the line, so it stays where it is, the full pull at `near` from it,
    /// fading out by `far`; along the line the pull fades out over `side` each way.
    Tuck { normal: [f64; 2], lift: f64, near: f64, far: f64, side: f64 },
}

/// 1 up to 1, 0 from `outer` on, smooth (C²: a smootherstep) between.
fn falloff(q: f64, outer: f64) -> f64 {
    if q.is_nan() || q >= outer {
        return 0.0;
    }
    if q <= 1.0 {
        return 1.0;
    }
    let t = (q - 1.0) / (outer - 1.0);
    1.0 - t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}

/// 0 at 0 and below, 1 at 1 and above, smooth (C²) between.
fn rise(t: f64) -> f64 {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}

/// The Wendland bump: 1 at the centre, 0 from `q = 1` on, smooth (C²) all the way.
fn bump(q: f64) -> f64 {
    if q >= 1.0 {
        return 0.0;
    }
    let a = 1.0 - q;
    a * a * a * a * (4.0 * q + 1.0)
}

impl Kernel {
    /// Where the output point `p` reads from, less `p`: the inverse of the move (a pixel the move takes to `c + m`
    /// is read at `c + m` from `c`).
    pub(super) fn back(&self, p: [f64; 2]) -> [f64; 2] {
        let (dx, dy) = (p[0] - self.centre[0], p[1] - self.centre[1]);
        let r2 = self.radius * self.radius;
        let q2 = (dx * dx + dy * dy) / r2;
        if q2.is_nan() || q2 >= 1.0 {
            return [0.0, 0.0];
        }
        let d = [dx, dy];
        match self.kind {
            Kind::Move(m) => {
                let w = bump(q2.sqrt());
                [-m[0] * w, -m[1] * w]
            }
            Kind::Zoom { axes: [a, b], half, taper, outer, by } => {
                let (u, v) = (dot(d, a), dot(d, b));
                let (hu, hv) = if v < 0.0 { (half[0] * (1.0 - taper * rise(-v / half[1])), half[1]) } else { (half[0], half[2]) };
                let q = ((u / hu).powi(2) + (v / hv).powi(2)).sqrt();
                let f = falloff(q, outer);
                // a point shows what was 1 / by as far from the centre
                let (ku, kv) = ((1.0 / by[0] - 1.0) * u * f, (1.0 / by[1] - 1.0) * v * f);
                let out = add(scale(a, ku), scale(b, kv));
                if out[0].is_finite() && out[1].is_finite() { out } else { [0.0, 0.0] }
            }
            Kind::Tuck { normal, lift, near, far, side } => {
                let (s, t) = (dot(d, normal), dot(d, [-normal[1], normal[0]]));
                if s <= 0.0 || s >= far || side <= 0.0 {
                    return [0.0, 0.0];
                }
                let g = if s < near { rise(s / near) } else { 1.0 - rise((s - near) / (far - near)) };
                // a point shows what was further from the line
                let w = lift * g * bump(t.abs() / side);
                if w.is_finite() { scale(normal, w) } else { [0.0, 0.0] }
            }
        }
    }

    /// This move with its strength scaled by `t` (0…1).
    pub(super) fn scaled(self, t: f64) -> Kernel {
        let kind = match self.kind {
            Kind::Move(m) => Kind::Move(scale(m, t)),
            Kind::Zoom { axes, half, taper, outer, by } => Kind::Zoom { axes, half, taper, outer, by: by.map(|b| 1.0 + (b - 1.0) * t) },
            Kind::Tuck { normal, lift, near, far, side } => Kind::Tuck { normal, lift: lift * t, near, far, side },
        };
        Kernel { kind, ..self }
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

pub(super) fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}

fn len(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// A slider's value as a share (−1…1); 0 for a value that is not a number.
fn share(v: f64) -> f64 {
    if v.is_finite() { (v / 100.0).clamp(-1.0, 1.0) } else { 0.0 }
}

/// A face as the moves see it: its mesh, its axes and its size.
pub(super) struct FaceGeom<'a> {
    mesh: &'a [[f64; 2]],
    /// Across the face (from the subject's right eye to the left) and down it (forehead to chin).
    across: [f64; 2],
    down: [f64; 2],
    /// The face's width (cheek to cheek) and height (mesh top to chin).
    pub(super) w: f64,
    pub(super) h: f64,
    /// A point on the face's middle line, between the cheeks.
    middle: [f64; 2],
}

impl<'a> FaceGeom<'a> {
    pub(super) fn new(mesh: &'a [[f64; 2]]) -> Option<FaceGeom<'a>> {
        if mesh.len() < MESH_POINTS || mesh.iter().any(|p| !(p[0].is_finite() && p[1].is_finite())) {
            return None;
        }
        let m = |i: usize| mesh.get(i).copied();
        let [a, b, c, d] = point::EYE_CORNERS.map(m);
        let (right_eye, left_eye) = (mid(a?, b?), mid(c?, d?));
        let eyes = sub(left_eye, right_eye);
        let (rc, lc) = (m(point::RIGHT_CHEEK)?, m(point::LEFT_CHEEK)?);
        let (top, chin) = (m(point::TOP)?, m(point::CHIN)?);
        let (w, h) = (len(sub(lc, rc)), len(sub(chin, top)));
        let span = len(eyes);
        // a mesh that is not a face (collapsed, or far from a face's proportions) is left alone
        if !(w > 1e-6 && h > 1e-6 && span > 0.1 * w && (0.4..=3.0).contains(&(h / w))) {
            return None;
        }
        let across = scale(eyes, 1.0 / span);
        let mut down = [-across[1], across[0]];
        if dot(down, sub(chin, top)) < 0.0 {
            down = scale(down, -1.0);
        }
        Some(FaceGeom { mesh, across, down, w, h, middle: mid(rc, lc) })
    }

    fn at(&self, i: usize) -> Option<[f64; 2]> {
        self.mesh.get(i).copied()
    }

    /// The unit vector from `p` toward the face's middle line, across the face.
    fn inward(&self, p: [f64; 2]) -> [f64; 2] {
        if dot(sub(p, self.middle), self.across) > 0.0 { scale(self.across, -1.0) } else { self.across }
    }
}

/// Moves of `amount` (photo units) at mesh points `points`, each toward `dir(point)`, fading out at `radius`: shared
/// out so that a point where several overlap still moves by about `amount`.
fn moves(g: &FaceGeom<'_>, points: &[usize], radius: f64, amount: f64, dir: impl Fn([f64; 2]) -> [f64; 2], out: &mut Vec<Kernel>) {
    let at: Vec<[f64; 2]> = points.iter().filter_map(|&i| g.at(i)).collect();
    if at.is_empty() || amount == 0.0 || radius.is_nan() || radius <= 0.0 {
        return;
    }
    let overlap = at.iter().map(|p| at.iter().map(|q| bump(len(sub(*p, *q)) / radius)).sum::<f64>()).sum::<f64>() / at.len() as f64;
    let each = amount / overlap.max(1.0);
    out.extend(at.iter().map(|&p| Kernel { centre: p, radius, kind: Kind::Move(scale(dir(p), each)) }));
}

/// What `s` moves on the face of mesh `mesh` (photo units, [`MESH_POINTS`] points); empty for a mesh that is not a
/// face or settings that move nothing.
pub fn kernels(mesh: &[[f64; 2]], s: &Reshape) -> Vec<Kernel> {
    let mut out = Vec::new();
    let Some(g) = FaceGeom::new(mesh) else { return out };
    let (w, h) = (g.w, g.h);
    let up = scale(g.down, -1.0);
    for side in point::CHEEKS {
        moves(&g, &side, SLIM_R * w, SLIM * w * share(s.slim_face), |p| g.inward(p), &mut out);
    }
    for side in point::JAW {
        moves(&g, &side, JAW_R * w, JAW * w * share(s.jaw), |p| scale(g.inward(p), -1.0), &mut out);
    }
    moves(&g, &point::CHIN_LINE, CHIN_R * w, CHIN * h * share(s.chin), |_| g.down, &mut out);
    tuck(&g, DOUBLE_CHIN * h * share(s.double_chin), &mut out);
    nose(&g, share(s.nose), &mut out);
    let top = share(s.forehead);
    if top != 0.0 {
        let centres: Vec<[f64; 2]> = point::FOREHEAD.iter().filter_map(|&i| g.at(i)).map(|p| add(p, scale(up, ABOVE_FOREHEAD * h))).collect();
        let shifted = FaceGeom { mesh: &centres, ..g };
        let all: Vec<usize> = (0..centres.len()).collect();
        moves(&shifted, &all, FOREHEAD_R * w, FOREHEAD * h * top, |_| up, &mut out);
    }
    let by = 1.0 + EYES * share(s.eye_size);
    if by != 1.0 {
        for pair in [[0, 1], [2, 3]] {
            let [a, b] = pair.map(|i| point::EYE_CORNERS.get(i).and_then(|&j| g.at(j)));
            if let (Some(a), Some(b)) = (a, b) {
                let half = len(sub(a, b)) / 2.0;
                if half > 1e-9 {
                    let along = scale(sub(b, a), 0.5 / half);
                    let half = [EYES_ALONG * half, EYES_ACROSS * half, EYES_ACROSS * half];
                    let kind = Kind::Zoom { axes: [along, [-along[1], along[0]]], half, taper: 0.0, outer: EYES_OUTER, by: [by, by] };
                    out.push(Kernel { centre: mid(a, b), radius: EYES_OUTER * half[0].max(half[1]), kind });
                }
            }
        }
    }
    out
}

/// Nose: the lower nose narrowed (`amount` < 0) or widened about its middle line, evenly across the wings, so the
/// nostrils and the tip keep their shape; the bridge above follows a little less, and nothing below the nose's
/// base moves.
fn nose(g: &FaceGeom<'_>, amount: f64, out: &mut Vec<Kernel>) {
    let [a, b] = point::NOSE_WINGS.map(|i| g.at(i));
    let (Some(a), Some(b), Some(top), Some(base)) = (a, b, g.at(point::NOSE_TOP), g.at(point::NOSE_BASE)) else { return };
    if amount == 0.0 {
        return;
    }
    let centre = mid(a, b);
    let wings = dot(sub(b, a), g.across).abs() / 2.0;
    let (up, down) = (dot(sub(centre, top), g.down), dot(sub(base, centre), g.down));
    // a mesh with the wings, the bridge or the base out of place is left alone
    if !(wings > 0.02 * g.w && up > 0.02 * g.h && down > 0.005 * g.h) {
        return;
    }
    let half = [NOSE_ACROSS * wings, NOSE_UP * up, NOSE_DOWN * down];
    let kind = Kind::Zoom { axes: [g.across, g.down], half, taper: NOSE_TAPER, outer: NOSE_OUTER, by: [1.0 + NOSE * amount, 1.0] };
    out.push(Kernel { centre, radius: NOSE_OUTER * half[0].max(half[1]).max(half[2]), kind });
}

/// Double Chin: along the jaw line, from one corner under the chin to the other, the soft area under it is pulled
/// up toward it by `lift` (photo units; less toward the corners), while the line itself stays: the jaw to neck
/// contour is tightened. A negative `lift` lets it down.
fn tuck(g: &FaceGeom<'_>, lift: f64, out: &mut Vec<Kernel>) {
    let at: Vec<[f64; 2]> = point::JAW_LINE.iter().filter_map(|&i| g.at(i)).collect();
    let n = at.len();
    if n < 3 || lift == 0.0 || !lift.is_finite() {
        return;
    }
    let spacing = at.windows(2).map(|w| len(sub(w[1], w[0]))).sum::<f64>() / (n - 1) as f64;
    let side = TUCK_SIDE * spacing;
    if side.is_nan() || side <= 1e-9 {
        return;
    }
    let centre = mid(g.middle, at[n / 2]);
    for (i, &p) in at.iter().enumerate() {
        let (prev, next) = (at[i.saturating_sub(1)], at[(i + 1).min(n - 1)]);
        let tangent = sub(next, prev);
        let l = len(tangent);
        if l.is_nan() || l <= 1e-12 {
            continue;
        }
        // the normal away from the face
        let mut normal = [-tangent[1] / l, tangent[0] / l];
        if dot(normal, sub(p, centre)) < 0.0 {
            normal = scale(normal, -1.0);
        }
        let x = (i as f64 - (n - 1) as f64 / 2.0) / ((n - 1) as f64 / 2.0);
        let c = (x * std::f64::consts::FRAC_PI_2).cos();
        let weight = TUCK_CORNERS + (1.0 - TUCK_CORNERS) * c * c;
        let overlap: f64 = at.iter().map(|q| bump(len(sub(p, *q)) / side)).sum();
        let kind = Kind::Tuck { normal, lift: lift * weight / overlap.max(1.0), near: TUCK_NEAR * g.h, far: TUCK_FAR * g.h, side };
        out.push(Kernel { centre: p, radius: (TUCK_FAR * g.h).hypot(side), kind });
    }
}

impl Reshape {
    /// Every slider at 0 (or not a number): the face does not move.
    pub(crate) fn is_off_shape(&self) -> bool {
        [self.slim_face, self.jaw, self.chin, self.double_chin, self.nose, self.forehead, self.eye_size].iter().all(|v| share(*v) == 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bump_is_one_at_the_centre_and_nothing_past_its_radius() {
        assert_eq!(bump(0.0), 1.0);
        assert_eq!(bump(1.0), 0.0);
        assert_eq!(bump(f64::NAN.max(2.0)), 0.0);
        assert!((0..100).all(|i| bump(i as f64 / 100.0) >= bump((i + 1) as f64 / 100.0)));
    }
}
