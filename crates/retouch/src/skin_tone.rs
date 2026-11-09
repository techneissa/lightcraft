//! Skin Tone: Even Tone, Redness and Face Light.
//!
//! - **Even Tone** (0…100) evens blotchy colour in the face's Facial Skin and Body Skin: where the skin's colour
//!   over a small area (an [`EVEN_FINE`] blur) differs from its colour over a wider one ([`EVEN_BROAD`]), the
//!   difference is taken away (all of it at 100). Facial Skin and Body Skin are each measured against themselves. Mostly colour, a little brightness ([`EVEN_LUMA`]). Differences
//!   much larger than blotches (a beard, a nostril's shadow, the line where skin meets lips) fade out of the
//!   correction ([`EVEN_KEEP`]), so the face keeps its shape and its natural gradients.
//! - **Redness** (0…100) lowers red patches (cheeks, nose, acne): where the skin over a spot ([`RED_SPOT`]) is
//!   redder than the skin around it ([`RED_AROUND`]), its colour is drawn towards that skin's, up to
//!   [`RED_CUT`] of the way (from nothing at [`RED_START`] redder to all of that from [`RED_FULL`]); its other colour and its brightness stay.
//! - **Face Light** (−100…+100) lightens or darkens the face against the rest of the photo, up to
//!   [`FACE_LIGHT_STOPS`]: the face's area (Facial Skin, Eyebrows, Eye Whites, Iris, Lips, Teeth) with a soft edge
//!   that falls to nothing at the area's border ([`LIGHT_EDGE`]), so nothing outside it changes.
//!
//! Every size is a fraction of the face. Colour is measured on a grid of cells ([`CELLS_PER_FACE`] across the face)
//! from the skin's pixels only (each cell is the mask-weighted mean, each blur divides by the blurred mask), so hair,
//! eyes, lips and the background never bleed in: no halo at the mask's edge. The correction is a smooth gain per
//! cell, read back bilinearly and multiplied in, so the texture finer than a cell (pores, hair, grain) keeps its
//! contrast. It is weighted by the mask: a pixel outside the masks never changes, and a face without them is left
//! alone. A slider move samples the cached masks once per pixel of their box and blurs the grid; no model runs.

mod grid;

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, PartMask, Scene, Scope, Spec, Stage, View};
use grid::{Grid, smoothstep, union};

/// How much Face Light ±100 changes the face, in stops.
pub const FACE_LIGHT_STOPS: f32 = 0.6;
/// Cells across the face's size in the colour grid (a cell is never smaller than a pixel).
pub const CELLS_PER_FACE: f64 = 128.0;
/// Even Tone: the blotch scale, a fraction of the face (Gaussian sigma).
pub const EVEN_FINE: f64 = 0.012;
/// Even Tone: the scale of the skin's natural colour, a fraction of the face.
pub const EVEN_BROAD: f64 = 0.12;
/// Even Tone: the share of a brightness blotch taken away at 100 (colour: all of it).
pub const EVEN_LUMA: f32 = 0.35;
/// Even Tone: a difference (natural log) this large is mostly kept, as structure, not a blotch: colour, brightness.
pub const EVEN_KEEP: [f32; 2] = [0.4, 0.18];
/// Redness: the size of a red spot, a fraction of the face.
pub const RED_SPOT: f64 = 0.008;
/// Redness: the skin a spot is compared with, a fraction of the face.
pub const RED_AROUND: f64 = 0.25;
/// Redness: how much redder (natural log of red over green) than the skin around a spot must be to count at all,
/// and to count fully.
pub const RED_START: f32 = 0.02;
pub const RED_FULL: f32 = 0.1;
/// Redness: how far towards the skin around it a red spot's colour goes at 100.
pub const RED_CUT: f32 = 0.75;
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
    let Some(grid) = scene.mask_box(face, part).and_then(|r| Grid::new(r, size, CELLS_PER_FACE)) else { return };
    let (iw, masks, view) = (img.width, masks_of(scene, face, &[part]), scene.view());
    // the skin mask at every pixel of the region, sampled once
    let mut mask = Plane::new(grid.w, grid.h);
    par_rows(&mut mask.data, grid.w, |py, row| {
        for (px, m) in row.iter_mut().enumerate() {
            *m = mask_of(view, &masks, grid.x0 + px, grid.y0 + py);
        }
    });
    // each cell: the mask-weighted sum of the skin's colour, and of the mask
    let mut sums = Image::<[f32; 4]>::new(grid.gw, grid.gh);
    {
        let (pixels, mask) = (&img.data, &mask);
        par_rows(&mut sums.data, grid.gw, |gy, cells| {
            for py in grid.start(gy)..grid.start(gy + 1) {
                let (Some(row), Some(mrow)) = (pixels.get((grid.y0 + py) * iw + grid.x0..), mask.data.get(py * grid.w..)) else { continue };
                for (px, (p, m)) in row.iter().zip(mrow).take(grid.w).enumerate() {
                    if *m <= 0.0 {
                        continue;
                    }
                    if let Some(c) = cells.get_mut(grid.cell_of(px, grid.gw)) {
                        let v = p.map(|v| if v.is_finite() { v.clamp(0.0, 64.0) } else { 0.0 });
                        *c = [c[0] + m * v[0], c[1] + m * v[1], c[2] + m * v[2], c[3] + m];
                    }
                }
            }
        });
    }
    // the wide blurs run on cells COARSE times larger
    let coarse = reduce(&sums, COARSE);
    let blur = |fraction: f64| gaussian(&sums, grid.sigma(fraction, size));
    let wide = |fraction: f64| gaussian(&coarse, grid.sigma(fraction, size) / COARSE as f32);
    let none = Image::<[f32; 4]>::new(0, 0);
    let (fine, broad) = if even > 0.0 { (blur(EVEN_FINE), wide(EVEN_BROAD)) } else { (none.clone(), none.clone()) };
    let (spot, around) = if red > 0.0 { (blur(RED_SPOT), wide(RED_AROUND)) } else { (none.clone(), none.clone()) };
    let mut gains = Image::<[f32; 3]>::new(grid.gw, grid.gh);
    par_rows(&mut gains.data, grid.gw, |y, row| {
        for (x, g) in row.iter_mut().enumerate() {
            let i = y * grid.gw + x;
            let at = |g: &Image<[f32; 4]>| g.data.get(i).copied().unwrap_or([0.0; 4]);
            let up = |g: &Image<[f32; 4]>| if g.data.is_empty() { [0.0; 4] } else { enlarged(g, COARSE, x, y) };
            let fine = at(&fine);
            let spot = if spot.data.is_empty() { fine } else { at(&spot) };
            *g = cell_gain([fine, up(&broad), spot, up(&around)], even, red);
        }
    });
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

/// How many fine cells across one coarse cell of the wide blurs.
const COARSE: usize = 4;

/// `g` with each `f × f` block of cells summed into one.
fn reduce(g: &Image<[f32; 4]>, f: usize) -> Image<[f32; 4]> {
    let (w, h) = (g.width.div_ceil(f).max(1), g.height.div_ceil(f).max(1));
    let mut out = Image::<[f32; 4]>::new(w, h);
    for (i, v) in g.data.iter().enumerate() {
        let (x, y) = (i % g.width.max(1) / f, i / g.width.max(1) / f);
        if let Some(o) = out.data.get_mut(y * w + x) {
            *o = [o[0] + v[0], o[1] + v[1], o[2] + v[2], o[3] + v[3]];
        }
    }
    out
}

/// Coarse cells `g` (each `f` fine cells across) read at fine cell (`x`, `y`), bilinear between coarse centres.
fn enlarged(g: &Image<[f32; 4]>, f: usize, x: usize, y: usize) -> [f32; 4] {
    let axis = |p: usize, n: usize| {
        let c = ((p as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, n.saturating_sub(1) as f32);
        let i = c.floor() as usize;
        (i, (i + 1).min(n.saturating_sub(1)), c - i as f32)
    };
    let ((x0, x1, tx), (y0, y1, ty)) = (axis(x, g.width), axis(y, g.height));
    let v = |x: usize, y: usize| g.data.get(y * g.width + x).copied().unwrap_or([0.0; 4]);
    let w = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
    let p = [v(x0, y0), v(x1, y0), v(x0, y1), v(x1, y1)];
    std::array::from_fn(|k| p.iter().zip(w).map(|(p, w)| p[k] * w).sum())
}

/// A cell's mean colour as log chroma (each channel's log less the log luminance) and log luminance; `None`
/// where the cell holds no skin.
fn logs(s: [f32; 4]) -> Option<([f32; 3], f32)> {
    if s[3].is_nan() || s[3] <= 1e-9 {
        return None;
    }
    let c = [s[0] / s[3], s[1] / s[3], s[2] / s[3]].map(|v| v.max(1e-6));
    let y = (LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]).max(1e-6).ln();
    Some((c.map(|v| v.ln() - y), y))
}

/// `d`, fading out as it grows past `keep` (a blotch is corrected, structure is kept).
fn soft(d: f32, keep: f32) -> f32 {
    d * (-(d / keep) * (d / keep)).exp()
}

/// The gain per channel of one cell, from its blurred sums `[fine, broad, spot, around]`.
fn cell_gain(s: [[f32; 4]; 4], even: f32, red: f32) -> [f32; 3] {
    let [fine, broad, spot, around] = s;
    let mut chroma = [0.0f32; 3];
    let mut luma = 0.0f32;
    if even > 0.0
        && let (Some((cf, lf)), Some((cb, lb))) = (logs(fine), logs(broad))
    {
        for (k, d) in chroma.iter_mut().enumerate() {
            *d = even * soft(cb[k] - cf[k], EVEN_KEEP[0]);
        }
        luma = even * EVEN_LUMA * soft(lb - lf, EVEN_KEEP[1]);
    }
    let Some((cs, _)) = logs(spot) else { return [1.0; 3] };
    if red > 0.0
        && let Some((ca, _)) = logs(around)
    {
        // the spot as Even Tone leaves it
        let cs: [f32; 3] = std::array::from_fn(|k| cs[k] + chroma[k]);
        let redder = (cs[0] - cs[1]) - (ca[0] - ca[1]);
        // a spot redder than its skin is drawn towards that skin's colour (all three channels, so the hue stays a
        // skin hue), the more the redder it is
        let g = red * RED_CUT * smoothstep(RED_START, RED_FULL, redder);
        for (k, d) in chroma.iter_mut().enumerate() {
            *d -= g * (cs[k] - ca[k]);
        }
    }
    // shift the chroma, then put the spot's luminance back, so only `luma` changes brightness
    let spot_c = [spot[0], spot[1], spot[2]].map(|v| v / spot[3].max(1e-9));
    let shifted: [f32; 3] = std::array::from_fn(|k| spot_c[k] * chroma[k].clamp(-0.7, 0.7).exp());
    let lum = |c: [f32; 3]| LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    let keep = if lum(shifted) > 1e-9 { lum(spot_c) / lum(shifted) } else { 1.0 };
    let gain: [f32; 3] = std::array::from_fn(|k| chroma[k].clamp(-0.7, 0.7).exp() * keep * luma.clamp(-0.5, 0.5).exp());
    if gain.iter().all(|g| g.is_finite()) { gain } else { [1.0; 3] }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(c: [f32; 3]) -> [f32; 4] {
        [c[0], c[1], c[2], 1.0]
    }

    #[test]
    fn a_blotch_moves_to_the_skin_around_it_and_keeps_its_brightness() {
        let skin = [0.40, 0.28, 0.20];
        let blotch = [0.44, 0.27, 0.20];
        let g = cell_gain([cell(blotch), cell(skin), cell(blotch), cell(skin)], 1.0, 0.0);
        let out: [f32; 3] = std::array::from_fn(|k| blotch[k] * g[k]);
        // redder than before, less red than the blotch: closer to the skin's colour
        let ratio = |c: [f32; 3]| c[0] / c[1];
        assert!(ratio(out) < ratio(blotch) && (ratio(out) - ratio(skin)).abs() < (ratio(blotch) - ratio(skin)).abs() * 0.3);
        // no change where there is none, and none at 0
        assert_eq!(cell_gain([cell(skin); 4], 1.0, 1.0), [1.0; 3]);
        assert_eq!(cell_gain([cell(blotch), cell(skin), cell(blotch), cell(skin)], 0.0, 0.0), [1.0; 3]);
    }

    #[test]
    fn redness_lowers_red_spots_only() {
        let skin = [0.40, 0.28, 0.20];
        let red = [0.48, 0.25, 0.19];
        let pale = [0.36, 0.30, 0.22];
        let g = cell_gain([cell(red), cell(red), cell(red), cell(skin)], 0.0, 1.0);
        assert!(red[0] * g[0] / (red[1] * g[1]) < red[0] / red[1] * 0.9, "{g:?}");
        // a spot less red than the skin around it is not touched
        let g = cell_gain([cell(pale), cell(pale), cell(pale), cell(skin)], 0.0, 1.0);
        assert!(g.iter().all(|v| (v - 1.0).abs() < 1e-6), "{g:?}");
        // brightness is kept
        let g = cell_gain([cell(red), cell(red), cell(red), cell(skin)], 0.0, 1.0);
        let lum = |c: [f32; 3]| LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
        assert!((lum(std::array::from_fn(|k| red[k] * g[k])) - lum(red)).abs() < 1e-4);
    }

    #[test]
    fn large_differences_are_structure_and_mostly_kept() {
        let skin = [0.40, 0.28, 0.20];
        let beard = [0.05, 0.04, 0.035];
        let g = cell_gain([cell(beard), cell(skin), cell(beard), cell(skin)], 1.0, 0.0);
        let lum = |c: [f32; 3]| LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
        let out: [f32; 3] = std::array::from_fn(|k| beard[k] * g[k]);
        assert!((lum(out) / lum(beard) - 1.0).abs() < 0.02, "{g:?}");
    }

    #[test]
    fn empty_cells_change_nothing() {
        assert_eq!(cell_gain([[0.0; 4]; 4], 1.0, 1.0), [1.0; 3]);
        assert_eq!(cell_gain([[f32::NAN; 4]; 4], 1.0, 1.0), [1.0; 3]);
    }
}
