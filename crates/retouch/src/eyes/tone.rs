//! Whites and Iris: per-pixel changes on the Eye Whites and Iris masks.

use lightcraft_raster::{Image, Rgb32f};

use super::geom::{Eye, smoothstep};
use super::grid::{blur, luma, sample};
use crate::{Part, Scene};

/// How much brighter the whites get at 100 (a factor: about 0.4 of a stop).
const WHITES_GAIN: f32 = 0.32;
/// Extra lift, at 100, for the darker spots of the whites (veins, blotches), toward the whites' level.
const WHITES_EVEN: f32 = 0.35;
/// How much of the whites' colour goes at 100 (veins and a yellow cast fade to a faint grey).
const WHITES_DESAT: f32 = 0.7;
/// The grid (across, down) the whites' level is measured on, over the Eye Whites mask's area.
const LEVEL_GRID: (usize, usize) = (96, 32);
/// The whites' level: this share of their pixels is darker.
const WHITES_LEVEL: f32 = 0.9;
/// A pixel of the whites' mask this much darker than their level (a lash, the lid's edge) is left alone; one at
/// the upper share is wholly changed.
const WHITES_GATE: (f32, f32) = (0.3, 0.6);
/// How much brighter the iris gets at 100.
const IRIS_GAIN: f32 = 0.45;
/// How strongly the iris's texture is sharpened at 100.
const IRIS_SHARP: f32 = 0.9;
/// Texture details under this share of their surroundings are noise and are left as they are.
const IRIS_CORE: f32 = 0.04;
/// The texture's scale, in half eye widths.
const IRIS_SIGMA: f64 = 0.05;
/// Most a texture detail darkens (a halo around a catchlight) or brightens, as a share of its surroundings.
const IRIS_DETAIL: (f32, f32) = (-0.25, 0.5);

/// The pixels of face `face`'s mask of `part`, with the mask's value at each: index, mask.
fn masked(img: &Rgb32f, scene: &Scene<'_>, face: usize, part: Part) -> Vec<(usize, f32)> {
    let Some(m) = scene.faces.get(face).and_then(|f| f.face.mask(part)) else { return Vec::new() };
    let Some([x0, y0, x1, y1]) = scene.mask_box(face, part) else { return Vec::new() };
    let view = scene.view();
    let mut out = Vec::new();
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
            let v = m.sample(nx, ny);
            if v > 0.0 {
                out.push((y * img.width + x, v));
            }
        }
    }
    out
}

/// Whites, `amount` 0…1.
pub fn whites(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, amount: f32) {
    let px = masked(img, scene, face, Part::EyeWhites);
    let Some(mask) = scene.faces.get(face).and_then(|f| f.face.mask(Part::EyeWhites)) else { return };
    // the whites' level, from a fixed grid over the whole mask on the photo: the same whatever part of the face a
    // render (a zoomed loupe) holds, and at every size
    let a = mask.area;
    let mut ys = Vec::with_capacity(LEVEL_GRID.0 * LEVEL_GRID.1);
    for j in 0..LEVEL_GRID.1 {
        for i in 0..LEVEL_GRID.0 {
            let (nx, ny) = (a.x0 + (i as f64 + 0.5) / LEVEL_GRID.0 as f64 * a.width(), a.y0 + (j as f64 + 0.5) / LEVEL_GRID.1 as f64 * a.height());
            if mask.sample(nx, ny) < 0.5 {
                continue;
            }
            if let Some(y) = scene.view().to_px(nx, ny).and_then(|(x, y)| sample(img, x, y)).map(luma).filter(|y| y.is_finite()) {
                ys.push(y);
            }
        }
    }
    if ys.is_empty() {
        return;
    }
    let last = ys.len() - 1;
    let k = ((last as f32 * WHITES_LEVEL) as usize).min(last);
    let (_, level, _) = ys.select_nth_unstable_by(k, f32::total_cmp);
    let level = *level;
    if level.is_nan() || level <= 1e-6 {
        return;
    }
    for (i, m) in px {
        let Some(p) = img.data.get_mut(i) else { continue };
        let y = luma(*p);
        let w = amount * m * smoothstep(WHITES_GATE.0 as f64 * level as f64, WHITES_GATE.1 as f64 * level as f64, y as f64) as f32;
        if w.is_nan() || w <= 0.0 {
            continue;
        }
        let even = (1.0 - y / level).clamp(0.0, 1.0);
        let gain = 1.0 + w * (WHITES_GAIN + WHITES_EVEN * even);
        let lit = p.map(|c| c * gain);
        let ly = luma(lit);
        let keep = 1.0 - WHITES_DESAT * w;
        *p = lit.map(|c| ly + (c - ly) * keep);
    }
}

/// Iris, `amount` 0…1: brighter and sharper. `half` sets the texture's scale (a half eye width, pixels).
pub fn iris(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, eyes: &[Eye], half: f64, amount: f32) {
    let Some([x0, y0, x1, y1]) = scene.mask_box(face, Part::Iris) else { return };
    let px = masked(img, scene, face, Part::Iris);
    if px.is_empty() {
        return;
    }
    // the luminance around the iris, blurred to the texture's scale
    let sigma = IRIS_SIGMA * half;
    let pad = (3.0 * sigma).ceil() as usize + 1;
    let (bx0, by0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    let (bx1, by1) = ((x1 + pad).min(img.width), (y1 + pad).min(img.height));
    let (bw, bh) = (bx1.saturating_sub(bx0), by1.saturating_sub(by0));
    let lum = Image::<f32>::from_fn(bw, bh, |x, y| img.data.get((by0 + y) * img.width + bx0 + x).map_or(0.0, |p| luma(*p)));
    let soft = blur(&lum, sigma);
    let irises: Vec<_> = eyes.iter().filter_map(|e| e.iris).collect();
    let w_img = img.width.max(1);
    for (i, m) in px {
        let (x, y) = (i % w_img, i / w_img);
        // cut to the mesh's iris circles, when there are some
        let disc = if irises.is_empty() {
            1.0
        } else {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            irises
                .iter()
                .map(|((cx, cy), r)| 1.0 - smoothstep(0.92 * r, 1.12 * r, ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt()))
                .fold(0.0, f64::max)
        };
        let w = amount * m * disc as f32;
        if w.is_nan() || w <= 0.0 || x < bx0 || y < by0 {
            continue;
        }
        let j = (y - by0) * bw + (x - bx0);
        let (Some(&l), Some(&s)) = (lum.data.get(j), soft.data.get(j)) else { continue };
        let raw = if s > 1e-6 { (l - s) / s } else { 0.0 };
        // soft coring: the noise's share goes, what is above it is kept
        let detail = (raw.signum() * (raw.abs() - IRIS_CORE).max(0.0)).clamp(IRIS_DETAIL.0, IRIS_DETAIL.1);
        let gain = (1.0 + IRIS_GAIN * w) * (1.0 + IRIS_SHARP * w * detail);
        if let Some(p) = img.data.get_mut(i)
            && gain.is_finite()
        {
            *p = p.map(|c| c * gain);
        }
    }
}
