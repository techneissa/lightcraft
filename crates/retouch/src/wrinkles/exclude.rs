//! Wrinkles v2, what is never a wrinkle: the ears, the nostrils and the face's edge, from the face mesh.
//!
//! - **Ears**: outside the face's outline, from a little above the brows to a little below the mouth. An ear's folds are
//!   deep, dark and look like wrinkles; the outline's points lie at the face's edge, so everything beyond it there is
//!   ear, temple hair or cheek's far side.
//! - **Face edge**: a ring on both sides of the outline, from the brows down (the top of the outline crosses the forehead,
//!   whose lines must reach it, so there it is not excluded; the hairline is cut by the Facial Skin mask).
//! - **Nostrils**: a disc on each, round the mesh's nostril points.
//! - **Eyes**: each eye's opening and a thin band round it, so the lash lines and the lids' edges are never a wrinkle.

use lightcraft_raster::{Plane, par_rows};

use super::areas::{self, P};
use super::heal::smoothstep;
use crate::FaceCtx;

/// The mesh points of the face's outline, clockwise from the top.
pub const OVAL: [usize; 36] = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234,
    127, 162, 21, 54, 103, 67, 109,
];
/// The mesh points round each nostril (the subject's right, then left): the wing, the bottom and the hole.
const NOSTRILS: [[usize; 3]; 2] = [[129, 98, 240], [358, 327, 460]];
/// The brows' upper edge (the forehead's lower limit) and the mouth's corners.
/// Each eye's opening, round from the outer corner along the lower lid and back along the upper one.
pub const EYE_RINGS: [[usize; 16]; 2] = [
    [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246],
    [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466],
];
/// The band round an eye: full within `LID_IN` of its opening, none past `LID_OUT` (fractions of the face); and round
/// each corner a disc, full within `CORNER_IN`, none past `CORNER_OUT`: the corner's dark wedge and the lash line's end
/// are the eye's, not lines to even out.
const LID_IN: f64 = 0.01;
const LID_OUT: f64 = 0.02;
const CORNER_IN: f64 = 0.02;
const CORNER_OUT: f64 = 0.032;
const BROWS: [usize; 10] = [70, 63, 105, 66, 107, 336, 296, 334, 293, 300];
const MOUTH: [usize; 2] = [61, 291];
const EYES: [usize; 2] = [33, 263];
const CHIN: usize = 152;

/// The ring on each side of the outline: full within `RING_IN` of it, none past `RING_OUT` (fractions of the face).
const RING_IN: f64 = 0.006;
const RING_OUT: f64 = 0.016;
/// The nostril's disc: full inside `NOSTRIL_IN`, none past `NOSTRIL` (fractions of the face).
const NOSTRIL_IN: f64 = 0.03;
const NOSTRIL: f64 = 0.05;
/// The side of the tiles whose depth inside the outline is measured once, pixels.
const TILE: usize = 16;

/// 1 where a wrinkle is never searched (0…1, soft), for a `w × h` crop whose face is `ctx` (mapped into the crop).
/// `None` without the face mesh.
#[cfg(test)]
pub(super) fn excluded(ctx: &FaceCtx<'_>, w: usize, h: usize) -> Option<Plane> {
    excluded_where(ctx, w, h, None)
}

/// [`excluded`] worked out only where `only` (a `w × h` plane) is above 0, and 0 elsewhere: the search needs it only
/// inside the wrinkle areas (issue #253).
pub(super) fn excluded_where(ctx: &FaceCtx<'_>, w: usize, h: usize, only: Option<&Plane>) -> Option<Plane> {
    let pt = |i: usize| ctx.landmarks.get(areas::FIRST + i).copied().flatten();
    let oval: Option<Vec<P>> = OVAL.iter().map(|&i| pt(i)).collect();
    let oval = oval?;
    let nostrils: Option<Vec<P>> = NOSTRILS
        .iter()
        .map(|n| {
            let ps: Option<Vec<P>> = n.iter().map(|&i| pt(i)).collect();
            let ps = ps?;
            let k = ps.len() as f64;
            Some((ps.iter().map(|p| p.0).sum::<f64>() / k, ps.iter().map(|p| p.1).sum::<f64>() / k))
        })
        .collect();
    let nostrils = nostrils?;
    let eyes_open: Option<Vec<Vec<P>>> = EYE_RINGS.iter().map(|ring| ring.iter().map(|&i| pt(i)).collect()).collect();
    let eyes_open = eyes_open?;
    // the face's "down" axis and where the brows and the mouth lie along it
    let eyes: Option<Vec<P>> = EYES.iter().map(|&i| pt(i)).collect();
    let eyes = eyes?;
    let mid = ((eyes[0].0 + eyes[1].0) / 2.0, (eyes[0].1 + eyes[1].1) / 2.0);
    let chin = pt(CHIN)?;
    let (dx, dy) = (chin.0 - mid.0, chin.1 - mid.1);
    let len = (dx * dx + dy * dy).sqrt();
    if !(len.is_finite() && len > 1e-9) {
        return None;
    }
    let down = (dx / len, dy / len);
    let along = |p: P| (p.0 - mid.0) * down.0 + (p.1 - mid.1) * down.1;
    let mean_along = |ids: &[usize]| -> Option<f64> {
        let v: Option<Vec<f64>> = ids.iter().map(|&i| pt(i).map(along)).collect();
        let v = v?;
        Some(v.iter().sum::<f64>() / v.len().max(1) as f64)
    };
    let (t_brow, t_mouth) = (mean_along(&BROWS)?, mean_along(&MOUTH)?);
    let s = ctx.size;
    let f = |v: f64| (v * s) as f32;
    let (ring_in, ring_out, nostril_in, nostril) = (f(RING_IN), f(RING_OUT), f(NOSTRIL_IN), f(NOSTRIL));
    let (lid_in, lid_out, corner_in, corner_out) = (f(LID_IN), f(LID_OUT), f(CORNER_IN), f(CORNER_OUT));
    let corners: Vec<P> = EYE_RINGS.iter().flat_map(|r| [r[0], r[8]]).filter_map(pt).collect();
    let (brow_top, brow_in, mouth_in, mouth_out) =
        (t_brow as f32 - f(0.08), t_brow as f32 - f(0.03), t_mouth as f32 + f(0.10), t_mouth as f32 + f(0.16));
    // (Most pixels lie well inside the outline, where the ears and the face's edge add nothing: the outline's distance
    // moves by at most a pixel a pixel, so one distance per tile shows a whole tile that far in. Issue #253.)
    let tiles_x = w.div_ceil(TILE);
    let deep: Vec<bool> = (0..tiles_x * h.div_ceil(TILE))
        .map(|i| {
            let half = TILE as f64 / 2.0;
            let c = (((i % tiles_x) * TILE) as f64 + half, ((i / tiles_x) * TILE) as f64 + half);
            super::skip_idle() && areas::polygon_inside(&oval, c) - half * std::f64::consts::SQRT_2 >= f64::from(ring_out) + 1e-3
        })
        .collect();
    // (an eye adds nothing to a pixel this far outside its box)
    let eye_boxes: Vec<[f64; 4]> = eyes_open
        .iter()
        .map(|e| {
            let far = f64::from(lid_out) + 1e-3;
            e.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
                [b[0].min(p.0 - far), b[1].min(p.1 - far), b[2].max(p.0 + far), b[3].max(p.1 + far)]
            })
        })
        .collect();
    let mut out = Plane::new(w, h);
    par_rows(&mut out.data, w.max(1), |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            if super::skip_idle() && only.is_some_and(|o| o.data.get(y * w + x).is_none_or(|v| *v <= 0.0)) {
                continue;
            }
            let p = (x as f64 + 0.5, y as f64 + 0.5);
            let t = along(p) as f32;
            let (ear, ring) = if deep.get((y / TILE) * tiles_x + x / TILE).copied().unwrap_or(false) {
                (0.0, 0.0)
            } else {
                let d = areas::polygon_inside(&oval, p) as f32;
                // the ears: beyond the outline, between the brows and the mouth
                let band = smoothstep(brow_top, brow_in, t) * (1.0 - smoothstep(mouth_in, mouth_out, t));
                let ear = band * (1.0 - smoothstep(-ring_in, ring_in, d));
                // the face's edge, below the brows
                let ring = (1.0 - smoothstep(ring_in, ring_out, d.abs())) * smoothstep(t_brow as f32, t_brow as f32 + f(0.03), t);
                (ear, ring)
            };
            let nose = nostrils
                .iter()
                .map(|n| {
                    let dist = (((p.0 - n.0).powi(2) + (p.1 - n.1).powi(2)).sqrt()) as f32;
                    1.0 - smoothstep(nostril_in, nostril, dist)
                })
                .fold(0.0f32, f32::max);
            let eye = eyes_open
                .iter()
                .zip(&eye_boxes)
                .filter(|(_, b)| !super::skip_idle() || (p.0 > b[0] && p.1 > b[1] && p.0 < b[2] && p.1 < b[3]))
                .map(|(e, _)| 1.0 - smoothstep(lid_in, lid_out, -(areas::polygon_inside(e, p) as f32)))
                .fold(0.0f32, f32::max);
            let corner = corners
                .iter()
                .map(|c| 1.0 - smoothstep(corner_in, corner_out, (((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt()) as f32))
                .fold(0.0f32, f32::max);
            *v = ear.max(ring).max(nose).max(eye).max(corner);
        }
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Face, Input, NormRect};
    use crate::{Scene, WholePhoto};

    const W: usize = 320;
    const H: usize = 400;
    /// The face's box in the test picture: 200 × 200 px at (60, 40).
    const BOX: [f64; 4] = [60.0, 40.0, 200.0, 200.0];

    fn face() -> Face {
        let at = |(fx, fy): (f64, f64)| ((BOX[0] + fx * BOX[2]) / W as f64, (BOX[1] + fy * BOX[3]) / H as f64);
        let five = [(0.33, 0.45), (0.67, 0.45), (0.5, 0.62), (0.38, 0.78), (0.62, 0.78)];
        let landmarks = five.into_iter().map(at).chain((0..areas::MESH).map(|i| at(areas::frontal(i)))).collect();
        Face {
            landmarks,
            ..Face::boxed(NormRect::new(BOX[0] / W as f64, BOX[1] / H as f64, (BOX[0] + BOX[2]) / W as f64, (BOX[1] + BOX[3]) / H as f64))
        }
    }

    /// The exclusion at the face's fractional point (`fx`, `fy`).
    fn at(plane: &Plane, fx: f64, fy: f64) -> f32 {
        let (x, y) = ((BOX[0] + fx * BOX[2]) as usize, (BOX[1] + fy * BOX[3]) as usize);
        plane.data.get(y * W + x).copied().unwrap_or(f32::NAN)
    }

    #[test]
    fn ears_nostrils_and_the_face_edge_are_excluded_and_the_forehead_cheeks_and_neck_are_not() {
        let input = Input::new(vec![face()]);
        let view = WholePhoto { width: W, height: H };
        let scene = Scene::new(&input, &view, W, H);
        let excluded = excluded(&scene.faces[0], W, H).expect("the mesh is there");
        // the ear: beyond the outline at ear height (the mesh's outline lies at x 0.06 there)
        assert!(at(&excluded, -0.03, 0.5) > 0.99, "the right ear");
        assert!(at(&excluded, 1.03, 0.5) > 0.99, "the left ear");
        // the nostrils
        assert!(at(&excluded, 0.44, 0.655) > 0.99, "the right nostril");
        assert!(at(&excluded, 0.56, 0.655) > 0.99, "the left nostril");
        // the face's edge, just inside the outline below the brows
        assert!(at(&excluded, 0.065, 0.55) > 0.5, "the face's edge");
        // the forehead (the outline's top crosses it), a cheek, the middle of the neck
        assert!(at(&excluded, 0.5, 0.18) < 0.01, "the forehead");
        assert!(at(&excluded, 0.5, 0.05) < 0.01, "the top of the outline");
        assert!(at(&excluded, 0.25, 0.6) < 0.01, "the cheek");
        assert!(at(&excluded, 0.5, 1.2) < 0.01, "the neck");
        // the eyes and a thin band round them, not the cheek below
        assert!(at(&excluded, 0.33, 0.45) > 0.99, "the right eye");
        assert!(at(&excluded, 0.33, 0.47) > 0.99, "its lower lid's edge");
        assert!(at(&excluded, 0.33, 0.52) < 0.01, "under the eye");
        assert!(at(&excluded, 0.24, 0.45) > 0.99, "just past the eye's outer corner");
        assert!(at(&excluded, 0.19, 0.47) < 0.01, "the crow's feet further out");
        // above the ears' band the side of the head is not excluded
        assert!(at(&excluded, -0.03, 0.05) < 0.01, "the side of the forehead");
    }

    #[test]
    fn a_face_without_the_mesh_has_no_exclusions() {
        let mut f = face();
        f.landmarks.truncate(5);
        let input = Input::new(vec![f]);
        let view = WholePhoto { width: W, height: H };
        let scene = Scene::new(&input, &view, W, H);
        assert!(excluded(&scene.faces[0], W, H).is_none());
    }
}
