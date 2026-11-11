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
const DOUBLE_CHIN: f64 = 0.05; // under the chin, of H
const NOSE: f64 = 0.035; // each wing of the nose, of W
const FOREHEAD: f64 = 0.07; // the top of the forehead, of H
const EYES: f64 = 0.16; // the eye's centre is enlarged by 1 / (1 − this)

/// The radius each move fades out at: a fraction of the face's width, or for the eyes of the eye's half width.
const SLIM_R: f64 = 0.28;
const JAW_R: f64 = 0.22;
const CHIN_R: f64 = 0.22;
const DOUBLE_CHIN_R: f64 = 0.2;
const NOSE_R: f64 = 0.09;
const FOREHEAD_R: f64 = 0.32;
const EYES_R: f64 = 2.0;
/// How far below the jaw line the soft area Double Chin lifts lies, a fraction of H.
const UNDER_CHIN: f64 = 0.10;
/// How far above the mesh's top (mid forehead) Forehead's moves sit, a fraction of H: nearer the hairline, so the
/// brows stay.
const ABOVE_FOREHEAD: f64 = 0.04;

/// Mesh points (MediaPipe face mesh indices). "Right" is the subject's: on the left of a frontal photo.
mod point {
    pub const TOP: usize = 10;
    pub const CHIN: usize = 152;
    pub const RIGHT_CHEEK: usize = 234;
    pub const LEFT_CHEEK: usize = 454;
    pub const NOSE_TIP: usize = 1;
    /// The subject's right eye's outer and inner corners, then the left eye's inner and outer.
    pub const EYE_CORNERS: [usize; 4] = [33, 133, 362, 263];
    /// The face's contour on each side, cheekbone to jaw corner (Slim Face).
    pub const CHEEKS: [[usize; 4]; 2] = [[234, 93, 132, 58], [454, 323, 361, 288]];
    /// The jaw's corners (Jaw).
    pub const JAW: [[usize; 3]; 2] = [[172, 136, 150], [397, 365, 379]];
    /// The chin (Chin, and below it Double Chin).
    pub const CHIN_LINE: [usize; 3] = [152, 148, 377];
    pub const UNDER_CHIN: [usize; 5] = [152, 148, 377, 176, 400];
    /// The nose's wings (Nose).
    pub const NOSE_WINGS: [[usize; 2]; 2] = [[64, 98], [294, 327]];
    /// The top of the face's contour (Forehead).
    pub const FOREHEAD: [usize; 3] = [10, 109, 338];
}

/// One move of a slider: around `centre`, fading out at `radius`.
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
    /// The pixels about the centre are enlarged by `1 / (1 − k)` at the centre (`k` < 0 makes them smaller).
    Scale(f64),
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
        let w = bump(q2.sqrt());
        match self.kind {
            Kind::Move(m) => [-m[0] * w, -m[1] * w],
            Kind::Scale(k) => [-k * dx * w, -k * dy * w],
        }
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
    let under = share(s.double_chin);
    if under != 0.0 {
        let centres: Vec<[f64; 2]> = point::UNDER_CHIN.iter().filter_map(|&i| g.at(i)).map(|p| add(p, scale(g.down, UNDER_CHIN * h))).collect();
        let shifted = FaceGeom { mesh: &centres, ..g };
        let all: Vec<usize> = (0..centres.len()).collect();
        moves(&shifted, &all, DOUBLE_CHIN_R * w, DOUBLE_CHIN * h * under, |_| up, &mut out);
    }
    for side in point::NOSE_WINGS {
        let tip = g.at(point::NOSE_TIP);
        let outward = |p: [f64; 2]| match tip {
            Some(t) if dot(sub(p, t), g.across) < 0.0 => scale(g.across, -1.0),
            _ => g.across,
        };
        moves(&g, &side, NOSE_R * w, NOSE * w * share(s.nose), outward, &mut out);
    }
    let top = share(s.forehead);
    if top != 0.0 {
        let centres: Vec<[f64; 2]> = point::FOREHEAD.iter().filter_map(|&i| g.at(i)).map(|p| add(p, scale(up, ABOVE_FOREHEAD * h))).collect();
        let shifted = FaceGeom { mesh: &centres, ..g };
        let all: Vec<usize> = (0..centres.len()).collect();
        moves(&shifted, &all, FOREHEAD_R * w, FOREHEAD * h * top, |_| up, &mut out);
    }
    let k = EYES * share(s.eye_size);
    if k != 0.0 {
        for pair in [[0, 1], [2, 3]] {
            let [a, b] = pair.map(|i| point::EYE_CORNERS.get(i).and_then(|&j| g.at(j)));
            if let (Some(a), Some(b)) = (a, b) {
                let half = len(sub(a, b)) / 2.0;
                if half > 1e-9 {
                    out.push(Kernel { centre: mid(a, b), radius: EYES_R * half, kind: Kind::Scale(k) });
                }
            }
        }
    }
    out
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
