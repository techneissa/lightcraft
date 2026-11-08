//! The eyes from the face mesh: each eye's opening (the lid margins, 16 mesh points) and its iris (a centre and four
//! points on the rim), in photo pixels. The face parser's eye class takes in lids and lashes; the mesh's opening is
//! the eye itself, so when the mesh saw a face the eyes are cut to it.

use crate::facemesh::MeshOut;
use crate::image::Crop;

/// MediaPipe's eye contours, in order around each opening.
const OPENINGS: [[usize; 16]; 2] = [
    [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246],
    [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466],
];
/// Iris centres; the four rim points follow each.
const IRIS_CENTRES: [usize; 2] = [468, 473];
/// The mesh must be at least this sure it saw a face.
const MIN_PRESENCE: f32 = 0.5;

/// What the mesh says about the eyes (empty when it was not sure).
#[derive(Default)]
pub struct MeshEyes {
    /// Centre x, y and radius.
    pub irises: Vec<(f32, f32, f32)>,
    pub openings: Vec<Vec<(f32, f32)>>,
}

pub(crate) fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A class probability as a mask value: weak guesses (under 1/4) go, sure ones (over 3/4) are whole, a soft band
/// between keeps the edge.
pub(crate) fn sharpen(p: f32) -> f32 {
    smoothstep(0.25, 0.75, p)
}

impl MeshEyes {
    pub fn from_mesh(mesh: &MeshOut, crop: &Crop) -> Self {
        if mesh.presence < MIN_PRESENCE {
            return Self::default();
        }
        let at = |k: usize| mesh.points.get(k).map(|p| crop.to_photo(p[0], p[1])).filter(|(x, y)| x.is_finite() && y.is_finite());
        let irises = IRIS_CENTRES
            .iter()
            .filter_map(|&c| {
                let (x, y) = at(c)?;
                let rim: Vec<f32> = (1..=4).filter_map(|k| at(c + k)).map(|(px, py)| ((px - x).powi(2) + (py - y).powi(2)).sqrt()).collect();
                let r = rim.iter().sum::<f32>() / rim.len().max(1) as f32;
                (r.is_finite() && r > 0.0).then_some((x, y, r))
            })
            .collect();
        let openings = OPENINGS.iter().filter_map(|ring| ring.iter().map(|&k| at(k)).collect::<Option<Vec<_>>>()).collect();
        Self { irises, openings }
    }

    pub fn has_openings(&self) -> bool {
        !self.openings.is_empty()
    }

    /// How much a photo pixel is inside an eye opening (a one-pixel soft edge).
    pub fn opening(&self, x: f32, y: f32) -> f32 {
        self.openings.iter().fold(0.0f32, |a, poly| a.max(smoothstep(-0.75, 0.75, signed_inside(poly, x, y))))
    }

    /// How much a photo pixel is inside an iris disc; `None` when the mesh gave none.
    pub fn iris(&self, x: f32, y: f32) -> Option<f32> {
        if self.irises.is_empty() {
            return None;
        }
        Some(self.irises.iter().fold(0.0f32, |a, &(cx, cy, r)| a.max(smoothstep(r + 0.75, r - 0.75, ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()))))
    }
}

/// Distance from a point to a closed polygon's outline, positive inside, negative outside.
fn signed_inside(poly: &[(f32, f32)], x: f32, y: f32) -> f32 {
    let n = poly.len();
    if n < 3 {
        return -1.0;
    }
    let mut inside = false;
    let mut best = f32::MAX;
    for i in 0..n {
        let (Some(&(ax, ay)), Some(&(bx, by))) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
            inside = !inside;
        }
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let d = ((x - ax - t * dx).powi(2) + (y - ay - t * dy).powi(2)).sqrt();
        best = best.min(d);
    }
    if inside { best } else { -best }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_a_square_is_positive_and_outside_negative() {
        let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!((signed_inside(&sq, 5.0, 5.0) - 5.0).abs() < 1e-5);
        assert!((signed_inside(&sq, 12.0, 5.0) + 2.0).abs() < 1e-5);
        assert_eq!(signed_inside(&sq[..2], 1.0, 1.0), -1.0);
        let eyes = MeshEyes { irises: vec![(5.0, 5.0, 2.0)], openings: vec![sq.to_vec()] };
        assert!(eyes.opening(5.0, 5.0) > 0.99 && eyes.opening(20.0, 5.0) == 0.0);
        assert!(eyes.iris(5.0, 5.0).unwrap() > 0.99 && eyes.iris(9.0, 5.0).unwrap() == 0.0);
        assert!(MeshEyes::default().iris(0.0, 0.0).is_none());
    }
}
