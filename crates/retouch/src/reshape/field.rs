//! The reshape warp: what each slider moves ([`kernels`]) and the sampled warp the pipeline reads ([`Field`]).
//!
//! Units: points are in *photo units*, the normalised photo scaled by its aspect so both axes have the same scale
//! (`x · width / long`, `y · height / long`). A circle stays a circle, and a size that is a fraction of the face is the
//! same on a preview and on the full-size photo.

use std::fmt;

use super::Reshape;
use crate::input::Fnv;

/// Where the face mesh starts in [`crate::Face::landmarks`]: after YuNet's five.
pub const MESH: usize = 5;
/// The face mesh's points (MediaPipe's order: 468 face points, then 5 per iris).
pub const MESH_POINTS: usize = 478;

/// The cell of the sampled warp, a fraction of the face's smaller side (width or height).
const CELL: f64 = 1.0 / 40.0;
/// Most grid points along one side of a face's grid (a face's grid is about 90 cells across).
const MAX_SIDE: usize = 400;
/// Each cell keeps at least this share of its area, at every corner: the warp never folds or pinches.
const MIN_AREA: f64 = 0.25;

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

/// How far from the photo's edge a move across it is faded out, a fraction of the face's smaller side.
const EDGE_MARGIN: f64 = 0.15;

/// 0 at and past the photo's edge (`t` ≤ 0), 1 from `t` = 1 inward, smooth (C¹) between.
fn edge_ramp(t: f64) -> f64 {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
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
    fn back(&self, p: [f64; 2]) -> [f64; 2] {
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

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
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
struct FaceGeom<'a> {
    mesh: &'a [[f64; 2]],
    /// Across the face (from the subject's right eye to the left) and down it (forehead to chin).
    across: [f64; 2],
    down: [f64; 2],
    /// The face's width (cheek to cheek) and height (mesh top to chin).
    w: f64,
    h: f64,
    /// A point on the face's middle line, between the cheeks.
    middle: [f64; 2],
}

impl<'a> FaceGeom<'a> {
    fn new(mesh: &'a [[f64; 2]]) -> Option<FaceGeom<'a>> {
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

/// One face's warp, sampled: where each grid point reads from, less the point.
#[derive(Clone)]
struct Patch {
    x0: f64,
    y0: f64,
    cell: f64,
    nx: usize,
    ny: usize,
    back: Vec<[f32; 2]>,
}

impl Patch {
    /// The grid over every kernel's reach, with a cell to spare (so its edge points don't move); `None` for no kernels.
    /// `photo`: the photo's size (photo units) and the margin its edges hold within: a move across an edge fades to
    /// nothing at it, so the photo's border maps onto itself and nothing is read from past it.
    fn sample(kernels: &[Kernel], cell: f64, photo: Option<([f64; 2], f64)>) -> Option<Patch> {
        let reach = |f: fn(&Kernel) -> f64| kernels.iter().map(f).fold(f64::NAN, f64::min);
        let x0 = reach(|k| k.centre[0] - k.radius) - cell;
        let y0 = reach(|k| k.centre[1] - k.radius) - cell;
        let x1 = -reach(|k| -(k.centre[0] + k.radius)) + cell;
        let y1 = -reach(|k| -(k.centre[1] + k.radius)) + cell;
        if !(x0.is_finite() && y0.is_finite() && x1 > x0 && y1 > y0) {
            return None;
        }
        // a cell large enough that the grid stays within MAX_SIDE points a side
        let cell = cell.max((x1 - x0).max(y1 - y0) / (MAX_SIDE - 1) as f64);
        let nx = (((x1 - x0) / cell).ceil() as usize + 1).clamp(2, MAX_SIDE);
        let ny = (((y1 - y0) / cell).ceil() as usize + 1).clamp(2, MAX_SIDE);
        let mut back = Vec::with_capacity(nx * ny);
        for j in 0..ny {
            for i in 0..nx {
                let p = [x0 + i as f64 * cell, y0 + j as f64 * cell];
                let mut d = kernels.iter().fold([0.0, 0.0], |a, k| add(a, k.back(p)));
                if let Some((size, margin)) = photo {
                    for (k, dk) in d.iter_mut().enumerate() {
                        let at = p.get(k).copied().unwrap_or(0.0);
                        let end = size.get(k).copied().unwrap_or(0.0);
                        *dk *= edge_ramp(at / margin) * edge_ramp((end - at) / margin);
                    }
                }
                back.push([d[0] as f32, d[1] as f32]);
            }
        }
        Some(Patch { x0, y0, cell, nx, ny, back })
    }

    fn d(&self, i: usize, j: usize) -> [f64; 2] {
        self.back.get(j * self.nx + i).map_or([0.0, 0.0], |d| [f64::from(d[0]), f64::from(d[1])])
    }

    /// Whether, with every move scaled by `t`, each cell keeps at least [`MIN_AREA`] of its area at each corner.
    fn holds(&self, t: f64) -> bool {
        let c = self.cell;
        let at = |i: usize, j: usize| {
            let d = self.d(i, j);
            [i as f64 * c + t * d[0], j as f64 * c + t * d[1]]
        };
        let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
        let min = MIN_AREA * c * c;
        (0..self.ny.saturating_sub(1)).all(|j| {
            (0..self.nx.saturating_sub(1)).all(|i| {
                let (a, b, cc, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                cross(a, b, d) > min && cross(b, cc, a) > min && cross(cc, d, b) > min && cross(d, a, cc) > min
            })
        })
    }

    /// Scale the moves down until no cell folds (the share kept; 1 when they already hold).
    fn unfold(&mut self) -> f64 {
        if self.holds(1.0) {
            return 1.0;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..16 {
            let m = (lo + hi) / 2.0;
            if self.holds(m) { lo = m } else { hi = m }
        }
        for d in &mut self.back {
            *d = [(f64::from(d[0]) * lo) as f32, (f64::from(d[1]) * lo) as f32];
        }
        lo
    }

    /// Where `p` reads from, bilinear between the grid points; `p` itself outside the grid.
    fn source(&self, p: [f64; 2]) -> [f64; 2] {
        let (fx, fy) = ((p[0] - self.x0) / self.cell, (p[1] - self.y0) / self.cell);
        let (mx, my) = ((self.nx - 1) as f64, (self.ny - 1) as f64);
        if !(fx >= 0.0 && fy >= 0.0 && fx < mx && fy < my) {
            return p;
        }
        let (i, j) = (fx.floor() as usize, fy.floor() as usize);
        let (tx, ty) = (fx - i as f64, fy - j as f64);
        let (a, b, c, d) = (self.d(i, j), self.d(i + 1, j), self.d(i, j + 1), self.d(i + 1, j + 1));
        let lerp = |k: usize| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        };
        [p[0] + lerp(0), p[1] + lerp(1)]
    }
}

/// The reshape of every face of a photo, sampled: for a point of the reshaped photo, the point of the photo it shows
/// ([`Field::source`]). Faces are applied one after the other; each face's warp is one to one and leaves everything
/// outside its grid in place, so the whole is too.
#[derive(Clone)]
pub struct Field {
    /// Normalised → photo units.
    sx: f64,
    sy: f64,
    patches: Vec<Patch>,
    fingerprint: u64,
}

impl fmt::Debug for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Field({} faces, {:016x})", self.patches.len(), self.fingerprint)
    }
}

/// Fields are equal when their fingerprints are (a hash of every grid value).
impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
    }
}

impl Field {
    /// The warp of `faces` (each a face's landmarks, normalised to the photo, with [`MESH_POINTS`] mesh points from
    /// [`MESH`] on, and its settings) on a photo of `width × height` (any unit: only the aspect counts). `None` when
    /// nothing moves: every face off, without a mesh, or not a face.
    pub fn build(faces: &[(&[(f64, f64)], &Reshape)], width: f64, height: f64) -> Option<Field> {
        let long = width.max(height);
        if !(width > 0.0 && height > 0.0 && long.is_finite()) {
            return None;
        }
        let (sx, sy) = (width / long, height / long);
        let mut patches = Vec::new();
        for (landmarks, s) in faces {
            if s.is_off_shape() {
                continue;
            }
            let Some(mesh) = landmarks.get(MESH..MESH + MESH_POINTS) else { continue };
            let mesh: Vec<[f64; 2]> = mesh.iter().map(|(x, y)| [x * sx, y * sy]).collect();
            let ks = kernels(&mesh, s);
            let Some(g) = FaceGeom::new(&mesh) else { continue };
            let size = g.w.min(g.h);
            if let Some(mut p) = Patch::sample(&ks, size * CELL, Some(([sx, sy], EDGE_MARGIN * size))) {
                p.unfold();
                if p.back.iter().any(|d| d[0] != 0.0 || d[1] != 0.0) {
                    patches.push(p);
                }
            }
        }
        if patches.is_empty() {
            return None;
        }
        let mut h = Fnv::new();
        for v in [sx, sy] {
            h.u64(v.to_bits());
        }
        for p in &patches {
            for v in [p.x0, p.y0, p.cell] {
                h.u64(v.to_bits());
            }
            h.u64(p.nx as u64);
            h.u64(p.ny as u64);
            for d in &p.back {
                h.u64(u64::from(d[0].to_bits()) << 32 | u64::from(d[1].to_bits()));
            }
        }
        Some(Field { sx, sy, patches, fingerprint: h.0 })
    }

    /// The point of the photo (normalised) that the reshaped photo shows at `(x, y)` (normalised). A point outside
    /// every face's grid is itself.
    pub fn source(&self, x: f64, y: f64) -> (f64, f64) {
        let start = [x * self.sx, y * self.sy];
        let p = self.patches.iter().fold(start, |p, patch| patch.source(p));
        // (a point no face moves is given back as it came, without the round trip through photo units)
        if p == start { (x, y) } else { (p[0] / self.sx, p[1] / self.sy) }
    }

    /// The areas the warp can move (normalised `[x0, y0, x1, y1]`, one per face): every point outside them is itself.
    pub fn areas(&self) -> Vec<[f64; 4]> {
        self.patches
            .iter()
            .map(|p| {
                let (x1, y1) = (p.x0 + (p.nx - 1) as f64 * p.cell, p.y0 + (p.ny - 1) as f64 * p.cell);
                [p.x0 / self.sx, p.y0 / self.sy, x1 / self.sx, y1 / self.sy]
            })
            .collect()
    }

    /// The warp as one flat table, for a GPU kernel that reads it as `f32`s (the counts and offsets are `u32` bits,
    /// read back with `bitcast<u32>`): `[faces, sx, sy]`, then per face `[x0, y0, cell, nx, ny, offset]`, then each
    /// face's grid from its `offset` on, `nx · ny` pairs row by row (x then y, photo units). [`Field::source`] is the
    /// reference.
    pub fn gpu_table(&self) -> Vec<f32> {
        let bits = |v: usize| f32::from_bits(u32::try_from(v).unwrap_or(u32::MAX));
        let head = 3 + 6 * self.patches.len();
        let mut out = vec![bits(self.patches.len()), self.sx as f32, self.sy as f32];
        let mut offset = head;
        for p in &self.patches {
            out.extend([p.x0 as f32, p.y0 as f32, p.cell as f32, bits(p.nx), bits(p.ny), bits(offset)]);
            offset += 2 * p.back.len();
        }
        for p in &self.patches {
            out.extend(p.back.iter().flatten());
        }
        out
    }

    /// A hash of every grid value, for cache keys.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// Whether, sampled on each face's own grid, the warp keeps every cell the same way round with at least
    /// `min_area` of its area at each corner (for tests: [`Field::build`] holds it at 0.25).
    pub fn keeps_cells(&self, min_area: f64) -> bool {
        self.patches.iter().all(|p| {
            let c = p.cell;
            let at = |i: usize, j: usize| p.source([p.x0 + i as f64 * c, p.y0 + j as f64 * c]);
            let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
            let min = min_area * c * c;
            (0..p.ny.saturating_sub(1)).all(|j| {
                (0..p.nx.saturating_sub(1)).all(|i| {
                    let (a, b, cc, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                    cross(a, b, d) > min && cross(b, cc, a) > min && cross(cc, d, b) > min && cross(d, a, cc) > min
                })
            })
        })
    }
}

/// A stand-in face mesh for a frontal face in box `b` (normalised): YuNet's five landmarks, then [`MESH_POINTS`]
/// points with the ones the sliders move placed as on a real face and the rest at the box's centre. For tests and
/// the example, where no face mesh model runs.
pub fn sketch_mesh(b: crate::NormRect) -> Vec<(f64, f64)> {
    const AT: &[(usize, f64, f64)] = &[
        (10, 0.5, 0.08),
        (152, 0.5, 0.98),
        (234, 0.08, 0.45),
        (454, 0.92, 0.45),
        (93, 0.10, 0.58),
        (323, 0.90, 0.58),
        (132, 0.14, 0.68),
        (361, 0.86, 0.68),
        (58, 0.19, 0.78),
        (288, 0.81, 0.78),
        (172, 0.22, 0.82),
        (397, 0.78, 0.82),
        (136, 0.27, 0.87),
        (365, 0.73, 0.87),
        (150, 0.32, 0.91),
        (379, 0.68, 0.91),
        (176, 0.37, 0.94),
        (400, 0.63, 0.94),
        (148, 0.42, 0.96),
        (377, 0.58, 0.96),
        (1, 0.5, 0.6),
        (64, 0.41, 0.63),
        (98, 0.43, 0.65),
        (294, 0.59, 0.63),
        (327, 0.57, 0.65),
        (109, 0.38, 0.10),
        (338, 0.62, 0.10),
        (33, 0.22, 0.42),
        (133, 0.40, 0.42),
        (362, 0.60, 0.42),
        (263, 0.78, 0.42),
        (468, 0.31, 0.42),
        (473, 0.69, 0.42),
        (61, 0.38, 0.8),
        (291, 0.62, 0.8),
    ];
    let at = |fx: f64, fy: f64| (b.x0 + fx * b.width(), b.y0 + fy * b.height());
    let mut mesh = vec![at(0.5, 0.5); MESH_POINTS];
    for &(i, fx, fy) in AT {
        if let Some(p) = mesh.get_mut(i) {
            *p = at(fx, fy);
        }
    }
    let five = [468, 473, 1, 61, 291].map(|i| mesh.get(i).copied().unwrap_or(at(0.5, 0.5)));
    five.into_iter().chain(mesh).collect()
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

    /// Two strong moves the other way round, close together, would fold the grid: they are scaled down until every
    /// cell holds, and the warp keeps most of what can be kept.
    #[test]
    fn moves_that_would_fold_are_scaled_down_until_they_hold() {
        let ks = [
            Kernel { centre: [0.0, 0.0], radius: 0.2, kind: Kind::Move([0.15, 0.0]) },
            Kernel { centre: [0.08, 0.0], radius: 0.2, kind: Kind::Move([-0.15, 0.0]) },
        ];
        let mut p = Patch::sample(&ks, 0.005, None).expect("a grid");
        assert!(!p.holds(1.0), "the test's moves must fold");
        let kept = p.unfold();
        assert!(kept > 0.05 && kept < 1.0, "{kept}");
        assert!(p.holds(1.0));
        // its edge points don't move: the patch leaves the rest of the photo in place
        for i in 0..p.nx {
            assert_eq!(p.d(i, 0), [0.0, 0.0]);
            assert_eq!(p.d(i, p.ny - 1), [0.0, 0.0]);
        }
    }

    #[test]
    fn the_bump_is_one_at_the_centre_and_nothing_past_its_radius() {
        assert_eq!(bump(0.0), 1.0);
        assert_eq!(bump(1.0), 0.0);
        assert_eq!(bump(f64::NAN.max(2.0)), 0.0);
        assert!((0..100).all(|i| bump(i as f64 / 100.0) >= bump((i + 1) as f64 / 100.0)));
    }
}
