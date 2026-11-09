//! Where Stray Hair looks for strands: a weight per cell of a coarse grid (0…1), from the face's part masks.
//!
//! The one place the area's extent is decided: the Refine brush (#99) adds its strokes to, or takes them from, what
//! [`weights`] returns.

use lightcraft_raster::Plane;
use lightcraft_raster::morph::{max_filter, min_filter};

use super::grid::{Grid, smoothstep};
use crate::{Part, Scene};

/// The parts Stray Hair reads.
pub const PARTS: &[Part] = &[
    Part::EntirePerson,
    Part::FacialSkin,
    Part::BodySkin,
    Part::Hair,
    Part::Eyebrows,
    Part::EyeWhites,
    Part::Iris,
    Part::Lips,
    Part::Teeth,
    Part::Others,
];

/// How far outside the main hair mass strands are looked for, in faces.
pub const BAND: f64 = 0.5;
/// How far past the searched pixels the search reads (the fills' samples beside a strand, the blurs), in faces.
pub const PAD: f64 = 0.05;
/// The search stays within this many faces of the face's box: to each side, above, and below it (long hair past
/// the shoulders is not searched).
pub const CAP_SIDE: f64 = 1.2;
pub const CAP_TOP: f64 = 1.0;
pub const CAP_BELOW: f64 = 1.6;
/// A coarse cell's side, in faces.
pub const COARSE: f64 = 0.01;
/// Strands narrower than twice this (in faces) are not part of the main hair mass.
const CORE_OPEN: f64 = 0.02;
/// The Facial Skin mask is closed over gaps this wide (in faces), so the strands it was cut around are in it.
const SKIN_CLOSE: f64 = 0.02;
/// Strands fade in over this distance from the main hair mass, in faces: a strand leaving the mass is not cut off
/// at its edge.
const FADE: (f32, f32) = (0.004, 0.03);
/// How far the parts left alone are grown, in faces: eyebrows; eyes (the lashes and lids); lips and teeth;
/// Others (glasses, jewellery).
const BROWS: f64 = 0.025;
const EYES: f64 = 0.06;
const MOUTH: f64 = 0.02;
const OTHERS: f64 = 0.015;

/// Where to look, per cell of a coarse grid (each 0…1).
pub(super) struct Area {
    /// How much to look for strands.
    pub weight: Vec<f32>,
    /// Skin: Facial Skin (closed) and Body Skin.
    pub skin: Vec<f32>,
    /// The main hair mass.
    pub core: Vec<f32>,
}

/// Where to look on `grid` (a coarse grid of face `face`), or `None` when the face has neither a Hair nor a Facial
/// Skin mask.
pub(super) fn weights(scene: &Scene<'_>, face: usize, grid: &Grid) -> Option<Area> {
    let ctx = scene.faces.get(face)?;
    let has = |p: Part| ctx.face.mask(p).is_some();
    if !has(Part::Hair) && !has(Part::FacialSkin) {
        return None;
    }
    let planes = sample(scene, face, grid);
    let get = |p: Part| planes.iter().find(|(q, _)| *q == p).map(|(_, v)| v);
    let empty = Plane::new(grid.w, grid.h);
    let plane = |p: Part| get(p).unwrap_or(&empty);
    let r = |faces: f64| grid.cells(faces).round().max(0.0) as usize;

    // the main hair mass, and each cell's distance from it (in faces)
    let hair = plane(Part::Hair);
    let core = max_filter(&min_filter(&hair.map(|v| if v > 0.5 { 1.0 } else { 0.0 }), r(CORE_OPEN)), r(CORE_OPEN));
    let dist = distance(&core).map(|d| (d as f64 * grid.cell / grid.face) as f32);
    let has_hair = core.data.iter().any(|v| *v > 0.5);
    let skin = plane(Part::FacialSkin);
    let skin = min_filter(&max_filter(&skin.map(|v| smoothstep(0.2, 0.6, v)), r(SKIN_CLOSE)), r(SKIN_CLOSE));
    let grow = |parts: &[Part], by: f64| -> Plane {
        let mut m = Plane::new(grid.w, grid.h);
        for p in parts {
            if let Some(v) = get(*p) {
                m = m.zip_map(v, f32::max);
            }
        }
        max_filter(&m.map(|v| smoothstep(0.1, 0.5, v)), r(by))
    };
    let left_alone = [
        grow(&[Part::Eyebrows], BROWS),
        grow(&[Part::EyeWhites, Part::Iris], EYES),
        grow(&[Part::Lips, Part::Teeth], MOUTH),
        grow(&[Part::Others], OTHERS),
    ];
    let person = get(Part::EntirePerson);
    let body = plane(Part::BodySkin);
    let mut out = vec![0.0f32; grid.len()];
    for (k, o) in out.iter_mut().enumerate() {
        let at = |p: &Plane| p.data.get(k).copied().unwrap_or(0.0);
        let d = if has_hair { dist.data.get(k).copied().unwrap_or(f32::INFINITY) } else { f32::INFINITY };
        let fade = smoothstep(FADE.0, FADE.1, d);
        let band = 1.0 - smoothstep(0.6 * BAND as f32, BAND as f32, d);
        let outer = if has_hair { band } else { 0.0 };
        let mut w = outer.max(at(&skin)) * fade;
        for a in &left_alone {
            w *= 1.0 - at(a);
        }
        // clothes: the person's pixels that are neither hair nor skin
        if let Some(p) = person {
            let clothes = (at(p) - at(hair).max(at(&skin)).max(at(body))).max(0.0);
            w *= 1.0 - smoothstep(0.3, 0.7, clothes);
        }
        *o = if w.is_finite() { w.clamp(0.0, 1.0) } else { 0.0 };
    }
    // skin for the strand rules: the face's and the body's (a lock over a hand is not stray)
    let skin = skin.zip_map(body, |f, b| f.max(smoothstep(0.2, 0.6, b)));
    Some(Area { weight: out, skin: skin.data, core: core.data })
}

/// The face's masks of [`PARTS`] on `grid`'s cells.
fn sample(scene: &Scene<'_>, face: usize, grid: &Grid) -> Vec<(Part, Plane)> {
    let Some(ctx) = scene.faces.get(face) else { return Vec::new() };
    let masks: Vec<_> = PARTS.iter().filter_map(|p| ctx.face.mask(*p).map(|m| (*p, m))).collect();
    let mut planes: Vec<(Part, Plane)> = masks.iter().map(|(p, _)| (*p, Plane::new(grid.w, grid.h))).collect();
    let view = scene.view();
    for j in 0..grid.h {
        for i in 0..grid.w {
            let (x, y) = grid.centre(i, j);
            let (nx, ny) = view.to_norm(x, y);
            for ((_, m), (_, plane)) in masks.iter().zip(planes.iter_mut()) {
                if let Some(v) = plane.data.get_mut(j * grid.w + i) {
                    *v = m.sample(nx, ny);
                }
            }
        }
    }
    planes
}

/// Each cell's distance from the nearest cell of `core` above a half, in cells (two-pass chamfer, 1 and √2);
/// infinite where `core` has none.
fn distance(core: &Plane) -> Plane {
    let (w, h) = (core.width, core.height);
    let mut d = core.map(|v| if v > 0.5 { 0.0 } else { f32::INFINITY });
    let diag = std::f32::consts::SQRT_2;
    let at = |d: &Plane, x: isize, y: isize| -> f32 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize { f32::INFINITY } else { d.get(x as usize, y as usize) }
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let v = at(&d, x, y)
                .min(at(&d, x - 1, y) + 1.0)
                .min(at(&d, x, y - 1) + 1.0)
                .min(at(&d, x - 1, y - 1) + diag)
                .min(at(&d, x + 1, y - 1) + diag);
            d.set(x as usize, y as usize, v);
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let v = at(&d, x, y)
                .min(at(&d, x + 1, y) + 1.0)
                .min(at(&d, x, y + 1) + 1.0)
                .min(at(&d, x + 1, y + 1) + diag)
                .min(at(&d, x - 1, y + 1) + diag);
            d.set(x as usize, y as usize, v);
        }
    }
    d
}
