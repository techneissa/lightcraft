//! Skin Tone: Even Tone, Redness and Face Light.
//!
//! - **Even Tone** (0…100) evens blotchy skin in the face's Facial Skin and Body Skin: age and sun spots, darker and
//!   lighter patches and colour blotches move towards the skin around them, while the face keeps its shading (the
//!   light across it, the nose's sides, the folds, the eyes' and lips' rims, the nostrils), its pores, freckles and
//!   stubble. Facial Skin and Body Skin are each measured against themselves.
//! - **Redness** (0…100) calms red skin (flushed cheeks, a red nose, red spots): a cell redder than the skin around
//!   it, or than the face's own calm skin, moves towards it, fully at 100; its brightness stays, and the skin keeps
//!   its own hue (it is not greyed).
//! - **Face Light** (−100…+100) lightens or darkens the face against the rest of the photo, up to
//!   [`FACE_LIGHT_STOPS`]: the face's area (Facial Skin, Eyebrows, Eye Whites, Iris, Lips, Teeth) with a soft edge
//!   that falls to nothing at the area's border ([`LIGHT_EDGE`]), so nothing outside it changes.
//!
//! How Even Tone and Redness tell a blotch from the face's form: [`model`]. Every size is a fraction of the face.
//! Colour is measured on a grid of cells ([`CELLS_PER_FACE`] across the face) from the skin's pixels only (each
//! cell is the mask-weighted mean, each blur divides by the blurred mask), so hair, eyes, lips and the background
//! never bleed in: no halo at the mask's edge. The correction is a smooth gain per cell, read back bilinearly and
//! multiplied in, so the texture finer than a cell (pores, hair, grain) keeps its contrast. It is weighted by the
//! mask: a pixel outside the masks never changes, and a face without them is left alone. A slider move samples the
//! cached masks once per pixel of their box and works on the grid; no model runs.

mod grid;
pub mod model;
mod zones;

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f, par_rows};
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, PartMask, Scene, Scope, Spec, Stage, View};
use grid::{Grid, smoothstep, union};

/// How much Face Light ±100 changes the face, in stops.
pub const FACE_LIGHT_STOPS: f32 = 0.6;
/// Cells across the face's size in the colour grid of Facial Skin, and of Body Skin (a body's blotches are larger,
/// and its mask can be many faces large). A cell is never smaller than a pixel.
pub const CELLS_PER_FACE: f64 = 160.0;
pub const BODY_CELLS_PER_FACE: f64 = 64.0;
/// Face Light: the soft edge's width, a fraction of the face.
pub const LIGHT_EDGE: f64 = 0.035;
/// Face Light: cells across the face's size (the light is smooth, so its grid is coarser).
const LIGHT_CELLS_PER_FACE: f64 = 48.0;

/// The parts Even Tone and Redness work on.
pub const SKIN: [Part; 2] = [Part::FacialSkin, Part::BodySkin];
/// The parts Face Light lights: the face, eyes and mouth included.
pub const FACE_AREA: [Part; 6] = [Part::FacialSkin, Part::Eyebrows, Part::EyeWhites, Part::Iris, Part::Lips, Part::Teeth];

/// Rec. 2020 luminance weights (the app's working space).
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// The Skin Tone settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SkinTone {
    /// 0…100; 0 is off.
    pub even_tone: f64,
    /// 0…100; 0 is off.
    pub redness: f64,
    /// −100…+100; 0 is off. Settings saved with the stand-in `amount` slider of `fork/retouch-masks.md` (Facial Skin
    /// up to half again brighter at 100, about 0.59 stop) read it as Face Light (0.6 stop at 100).
    #[serde(alias = "amount")]
    pub face_light: f64,
}

impl Feature for SkinTone {
    const KEY: &'static str = "skinTone";
    const LABEL: &'static str = "Skin Tone";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::amount("retouch.skinTone.evenTone", "Even Tone"), get: |s| s.even_tone, set: |s, v| s.even_tone = v },
        Control { spec: Spec::amount("retouch.skinTone.redness", "Redness"), get: |s| s.redness, set: |s, v| s.redness = v },
        Control { spec: Spec::signed("retouch.skinTone.faceLight", "Face Light"), get: |s| s.face_light, set: |s, v| s.face_light = v },
    ];
    /// Its reads stay inside the areas of its masks (each grid covers a mask's box, and every blur runs on the grid),
    /// which the step already renders whole for a zoomed window: only a pixel of rounding past them.
    const REACH: f64 = 0.02;
    const PARTS: &'static [Part] = &[Part::FacialSkin, Part::BodySkin, Part::Eyebrows, Part::EyeWhites, Part::Iris, Part::Lips, Part::Teeth];

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        let Some(size) = scene.faces.get(face).map(|f| f.size) else { return };
        if img.width == 0 || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let unit = |v: f64, lo: f64| if v.is_finite() { (v.clamp(lo, 100.0) / 100.0) as f32 } else { 0.0 };
        let (even, red, light) = (unit(self.even_tone, 0.0), unit(self.redness, 0.0), unit(self.face_light, -100.0));
        if even > 0.0 || red > 0.0 {
            // the face and the body each against their own skin (a neck or a hand is often paler than the face)
            for part in SKIN {
                tone(img, scene, face, part, size, even, red);
            }
        }
        if light != 0.0 {
            face_light(img, scene, face, size, light);
        }
    }
}

/// The masks of `parts` a face has.
fn masks_of<'a>(scene: &'a Scene<'_>, face: usize, parts: &[Part]) -> Vec<&'a PartMask> {
    let Some(f) = scene.faces.get(face) else { return Vec::new() };
    parts.iter().filter_map(|p| f.face.mask(*p)).collect()
}

/// The highest of `masks` at pixel (`x`, `y`) (as [`Scene::mask_at`], mapping the pixel once; stops at the first
/// that is full).
fn mask_of(view: &dyn View, masks: &[&PartMask], x: usize, y: usize) -> f32 {
    let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
    let mut m = 0.0f32;
    for mask in masks {
        m = m.max(mask.sample(nx, ny));
        if m >= 1.0 {
            return 1.0;
        }
    }
    m
}

fn region(scene: &Scene<'_>, face: usize, parts: &[Part]) -> Option<[usize; 4]> {
    parts.iter().map(|p| scene.mask_box(face, *p)).fold(None, union)
}

/// Even Tone and Redness on one face's skin `part`.
fn tone(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, part: Part, size: f64, even: f32, red: f32) {
    let per_face = if part == Part::FacialSkin { CELLS_PER_FACE } else { BODY_CELLS_PER_FACE };
    let Some(grid) = scene.mask_box(face, part).and_then(|r| Grid::new(r, size, per_face)) else { return };
    let (iw, masks, view) = (img.width, masks_of(scene, face, &[part]), scene.view());
    // the skin mask at every pixel of the region, sampled once
    let mut mask = Plane::new(grid.w, grid.h);
    par_rows(&mut mask.data, grid.w, |py, row| {
        for (px, m) in row.iter_mut().enumerate() {
            *m = mask_of(view, &masks, grid.x0 + px, grid.y0 + py);
        }
    });
    // each cell: the mask-weighted sums of the skin's colour, the mask, and the luminance and its square (texture)
    let mut sums = vec![[0.0f32; 6]; grid.gw * grid.gh];
    {
        let (pixels, mask) = (&img.data, &mask);
        par_rows(&mut sums, grid.gw, |gy, cells| {
            for py in grid.start(gy)..grid.start(gy + 1) {
                let (Some(row), Some(mrow)) = (pixels.get((grid.y0 + py) * iw + grid.x0..), mask.data.get(py * grid.w..)) else { continue };
                for (px, (p, m)) in row.iter().zip(mrow).take(grid.w).enumerate() {
                    if *m <= 0.0 {
                        continue;
                    }
                    if let Some(c) = cells.get_mut(grid.cell_of(px, grid.gw)) {
                        let v = p.map(|v| if v.is_finite() { v.clamp(0.0, 64.0) } else { 0.0 });
                        let y = LUMA[0] * v[0] + LUMA[1] * v[1] + LUMA[2] * v[2];
                        *c = [c[0] + m * v[0], c[1] + m * v[1], c[2] + m * v[2], c[3] + m, c[4] + m * y, c[5] + m * y * y];
                    }
                }
            }
        });
    }
    let shapes = scene.faces.get(face).and_then(zones::Shapes::of);
    let zone = |r: &zones::Reaches| shapes.as_ref().map(|s| s.cells(&grid, r)).unwrap_or_default();
    let (even_zone, red_zone) = (zone(&zones::EVEN), zone(&zones::RED));
    let cells = model::Cells { gw: grid.gw, gh: grid.gh, sums, area: (grid.cell * grid.cell) as f32 };
    let inputs = model::Inputs { even, red, per_face: (size / grid.cell) as f32, even_zone: &even_zone, red_zone: &red_zone };
    let gains = model::gains(&cells, &inputs);
    par_rows(&mut img.data, iw, |y, row| {
        let Some(py) = y.checked_sub(grid.y0).filter(|py| *py < grid.h) else { return };
        let Some(mrow) = mask.data.get(py * grid.w..) else { return };
        for (px, m) in mrow.iter().take(grid.w).enumerate() {
            if *m <= 0.0 {
                continue;
            }
            let g = grid.at(&gains, px, py);
            if let Some(p) = row.get_mut(grid.x0 + px) {
                *p = [p[0] * (1.0 + m * (g[0] - 1.0)), p[1] * (1.0 + m * (g[1] - 1.0)), p[2] * (1.0 + m * (g[2] - 1.0))];
            }
        }
    });
}

/// Face Light on one face's area.
fn face_light(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, size: f64, light: f32) {
    let Some(grid) = region(scene, face, &FACE_AREA).and_then(|r| Grid::new(r, size, LIGHT_CELLS_PER_FACE)) else { return };
    let (masks, view) = (masks_of(scene, face, &FACE_AREA), scene.view());
    let area = Plane::from_fn(grid.gw, grid.gh, |gx, gy| {
        let (x, y) = grid.centre(gx, gy);
        mask_of(view, &masks, x, y)
    });
    // inside the area, at least an edge's width from its border, full; nothing at the border
    let weight = gaussian(&area, grid.sigma(LIGHT_EDGE, size)).map(|v| smoothstep(0.5, 0.92, v));
    let k = light * FACE_LIGHT_STOPS * std::f32::consts::LN_2;
    let iw = img.width;
    par_rows(&mut img.data, iw, |y, row| {
        let Some(py) = y.checked_sub(grid.y0).filter(|py| *py < grid.h) else { return };
        for px in 0..grid.w {
            let w = grid.at(&weight, px, py);
            if w <= 0.0 {
                continue;
            }
            let m = mask_of(view, &masks, grid.x0 + px, y);
            if m <= 0.0 {
                continue;
            }
            if let Some(p) = row.get_mut(grid.x0 + px) {
                let g = (k * w * m).exp();
                *p = [p[0] * g, p[1] * g, p[2] * g];
            }
        }
    });
}
