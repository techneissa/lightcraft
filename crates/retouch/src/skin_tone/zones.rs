//! Where the face's own structure lies, from the face mesh: the eyes, the lips, the nostrils and the folds from the
//! nose to the mouth's corners. Even Tone keeps their darkness (it is the face's shape, not a blotch), and Redness
//! leaves the lips' and eyes' rims alone. A face without the mesh (YuNet's five points only) has no zones.

use super::grid::Grid;
use crate::FaceCtx;

/// Where the face's landmarks hold the 478 of the face mesh: after YuNet's five.
const MESH_START: usize = 5;
/// The mesh's eye outlines (MediaPipe's numbering), the subject's right then left.
const EYES: [[usize; 16]; 2] = [
    [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246],
    [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466],
];
/// The mesh's outer lip outline.
const LIPS: [usize; 20] = [61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146];
/// The points round each nostril (the wing, the bottom and the hole); a zone is a disc round their middle.
const NOSTRILS: [[usize; 3]; 2] = [[129, 98, 240], [358, 327, 460]];
/// Each fold from the nose's wing to the mouth's corner.
const FOLDS: [[usize; 2]; 2] = [[129, 61], [358, 291]];

/// A zone's soft edge: full within the first distance (a fraction of the face) of its shape, nothing past the second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach(pub f64, pub f64);

/// How far each zone reaches, fractions of the face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reaches {
    pub eyes: Reach,
    pub lips: Reach,
    pub nostrils: Reach,
    pub folds: Reach,
}

/// Even Tone: the eyes' rims and lash shadows, the lips' edge, the nostrils and the nose-to-mouth folds keep their
/// darkness.
pub const EVEN: Reaches = Reaches { eyes: Reach(0.01, 0.03), lips: Reach(0.006, 0.02), nostrils: Reach(0.03, 0.05), folds: Reach(0.008, 0.02) };
/// Redness: only the rims of the lips and eyes (where a red lip or a red eye rim meets the skin's mask).
pub const RED: Reaches = Reaches { eyes: Reach(0.005, 0.02), lips: Reach(0.004, 0.015), nostrils: Reach(0.0, 0.0), folds: Reach(0.0, 0.0) };

type P = (f64, f64);

/// The face's zone shapes in image pixels; `None` without the mesh.
pub(super) struct Shapes {
    eyes: [Vec<P>; 2],
    lips: Vec<P>,
    nostrils: [P; 2],
    folds: [[P; 2]; 2],
    size: f64,
}

impl Shapes {
    pub fn of(face: &FaceCtx<'_>) -> Option<Shapes> {
        let pt = |i: usize| face.landmarks.get(MESH_START + i).copied().flatten();
        let poly = |idx: &[usize]| idx.iter().map(|&i| pt(i)).collect::<Option<Vec<P>>>();
        let centre = |idx: &[usize; 3]| {
            let ps = poly(idx)?;
            Some((ps.iter().map(|p| p.0).sum::<f64>() / 3.0, ps.iter().map(|p| p.1).sum::<f64>() / 3.0))
        };
        Some(Shapes {
            eyes: [poly(&EYES[0])?, poly(&EYES[1])?],
            lips: poly(&LIPS)?,
            nostrils: [centre(&NOSTRILS[0])?, centre(&NOSTRILS[1])?],
            folds: [[pt(FOLDS[0][0])?, pt(FOLDS[0][1])?], [pt(FOLDS[1][0])?, pt(FOLDS[1][1])?]],
            size: face.size,
        })
    }

    /// The box (`[x0, y0, x1, y1]`, image pixels) outside which every zone of `r` is 0.
    fn bounds(&self, r: &Reaches) -> [f64; 4] {
        let grow = r.eyes.1.max(r.lips.1).max(r.nostrils.1).max(r.folds.1) * self.size;
        let points = self.eyes.iter().flatten().chain(&self.lips).chain(&self.nostrils).chain(self.folds.iter().flatten());
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in points {
            b = [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)];
        }
        [b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow]
    }

    /// The zone at image point `p` (0…1).
    pub fn at(&self, p: P, r: &Reaches) -> f32 {
        let ramp = |d: f64, reach: Reach| {
            let (full, none) = (reach.0 * self.size, reach.1 * self.size);
            if none <= full {
                return 0.0;
            }
            ((none - d) / (none - full)).clamp(0.0, 1.0)
        };
        let mut z = 0.0f64;
        for eye in &self.eyes {
            z = z.max(ramp(signed_distance(eye, p), r.eyes));
        }
        z = z.max(ramp(signed_distance(&self.lips, p), r.lips));
        for n in &self.nostrils {
            z = z.max(ramp(((p.0 - n.0).powi(2) + (p.1 - n.1).powi(2)).sqrt(), r.nostrils));
        }
        for [a, b] in &self.folds {
            z = z.max(ramp(segment_distance(*a, *b, p), r.folds));
        }
        if z.is_finite() { z as f32 } else { 0.0 }
    }

    /// The zones at every cell's centre of `grid` (row by row).
    pub fn cells(&self, grid: &Grid, r: &Reaches) -> Vec<f32> {
        let mut out = vec![0.0f32; grid.gw * grid.gh];
        let b = self.bounds(r);
        lightcraft_raster::par_rows(&mut out, grid.gw.max(1), |gy, row| {
            let y = grid.y0 as f64 + (gy as f64 + 0.5) * grid.cell;
            if !(b[1]..=b[3]).contains(&y) {
                return;
            }
            for (gx, z) in row.iter_mut().enumerate() {
                let x = grid.x0 as f64 + (gx as f64 + 0.5) * grid.cell;
                if (b[0]..=b[2]).contains(&x) {
                    *z = self.at((x, y), r);
                }
            }
        });
        out
    }
}

/// The distance from `p` to segment `a`–`b`.
fn segment_distance(a: P, b: P, p: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 1e-12 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

/// The distance from `p` to polygon `poly`'s edge, negative inside it.
fn signed_distance(poly: &[P], p: P) -> f64 {
    let mut d = f64::INFINITY;
    let mut inside = false;
    for (i, a) in poly.iter().enumerate() {
        let b = poly.get(i + 1).or(poly.first()).copied().unwrap_or(*a);
        d = d.min(segment_distance(*a, b, p));
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    if inside { -d } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances_to_a_square_and_a_segment() {
        let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!((signed_distance(&sq, (5.0, 5.0)) + 5.0).abs() < 1e-9);
        assert!((signed_distance(&sq, (13.0, 5.0)) - 3.0).abs() < 1e-9);
        assert!((segment_distance((0.0, 0.0), (10.0, 0.0), (5.0, 4.0)) - 4.0).abs() < 1e-9);
        assert!((segment_distance((0.0, 0.0), (0.0, 0.0), (3.0, 4.0)) - 5.0).abs() < 1e-9);
    }
}
