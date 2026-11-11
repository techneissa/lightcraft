//! The reshape warp the pipeline reads ([`Field`]): what each slider moves ([`super::kernels`]), summed and sampled.
//!
//! Units: points are in *photo units*, the normalised photo scaled by its aspect so both axes have the same scale
//! (`x · width / long`, `y · height / long`). A circle stays a circle, and a size that is a fraction of the face is the
//! same on a preview and on the full-size photo.

use std::fmt;

use super::Reshape;
use super::kernels::{FaceGeom, Kernel, MESH, MESH_POINTS, add, kernels};
use crate::Feature;
use crate::input::Fnv;

/// The cell of the sampled warp, a fraction of the face's smaller side (width or height).
const CELL: f64 = 1.0 / 40.0;
/// Most grid points along one side of a face's grid (a face's grid is about 90 cells across).
const MAX_SIDE: usize = 400;
/// Each cell keeps at least this share of its area, at every corner: the warp never folds or pinches.
const MIN_AREA: f64 = 0.25;

/// How far from the photo's edge a move across it is faded out, a fraction of the face's smaller side.
const EDGE_MARGIN: f64 = 0.15;

/// 0 at and past the photo's edge (`t` ≤ 0), 1 from `t` = 1 inward, smooth (C¹) between.
fn edge_ramp(t: f64) -> f64 {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
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
            let Some(g) = FaceGeom::new(&mesh) else { continue };
            let size = g.w.min(g.h);
            let photo = Some(([sx, sy], EDGE_MARGIN * size));
            // each slider is first scaled down alone until it holds, so one that would fold (a move across the
            // photo's edge, a double chin out of the frame) does not take the others down with it
            let mut ks = Vec::new();
            for c in Reshape::CONTROLS {
                let v = c.read(s);
                if v == 0.0 {
                    continue;
                }
                let mut one = Reshape::default();
                c.write(&mut one, v);
                let mut own = kernels(&mesh, &one);
                if let Some(mut p) = Patch::sample(&own, size * CELL, photo) {
                    let t = p.unfold();
                    own = own.into_iter().map(|k| k.scaled(t)).collect();
                }
                ks.extend(own);
            }
            if let Some(mut p) = Patch::sample(&ks, size * CELL, photo) {
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
        (149, 0.345, 0.925),
        (378, 0.655, 0.925),
        (176, 0.37, 0.94),
        (400, 0.63, 0.94),
        (148, 0.42, 0.96),
        (377, 0.58, 0.96),
        (1, 0.5, 0.6),
        (64, 0.366, 0.63),
        (98, 0.39, 0.655),
        (294, 0.634, 0.63),
        (327, 0.61, 0.655),
        (129, 0.357, 0.64),
        (358, 0.643, 0.64),
        (168, 0.5, 0.40),
        (2, 0.5, 0.68),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reshape::Kind;

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
}
