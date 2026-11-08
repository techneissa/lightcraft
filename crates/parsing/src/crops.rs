//! Sapiens2 up close: the model is trained on pictures framed around one person, and in a whole photo of several
//! people it is unsure of those further back (dark clothes there read as background). Each person, largest face first,
//! gets a pass of their own over a box around their body, and the box's result replaces the whole photo's there (with
//! a soft edge where it meets it).

use candle_core::Device;

use crate::body::BodyMap;
use crate::image::{RgbImage, sample_grid};
use crate::sapiens::{self, Sapiens2};
use crate::types::NormRect;
use crate::{Error, Result};

/// The most people that get a pass of their own (the others keep the whole photo's).
pub const MAX_CROPS: usize = 8;
/// Model pixels per grid cell of a box: its result is brought down to the grid's cells, so more is wasted time.
const CELL_SAMPLES: f32 = 1.5;
/// A box's long side is never smaller than this for the model.
const MIN_LONG: f32 = 384.0;
/// A box over this share of the photo was seen about as well by the whole photo's pass.
const WHOLE_SHARE: f32 = 0.5;
/// The share of a box's size over which its result fades into the whole photo's.
const FEATHER: f32 = 0.08;
/// How much the whole photo's pass counts where a box covers it.
const WHOLE_WEIGHT: f32 = 1e-3;
/// A body box around a face: this many face widths across, from this many face heights above the face's top to this
/// many below it.
const BODY_WIDE: f32 = 4.0;
const BODY_ABOVE: f32 = 0.6;
const BODY_BELOW: f32 = 8.0;

/// The box around a face where its body may be, in fractions of the photo.
fn body_box(face: &NormRect, aspect: f32) -> NormRect {
    let (cx, _) = face.centre();
    let fw = face.width().max(face.height() / aspect.max(1e-3));
    let fh = face.height();
    NormRect::new(cx - BODY_WIDE * fw / 2.0, face.y0 - BODY_ABOVE * fh, cx + BODY_WIDE * fw / 2.0, face.y0 + BODY_BELOW * fh)
}

/// The boxes to look at closer: each person's body (where the clothes parser's person area is, held to the body box
/// around their face), largest face first, at most `max`; boxes that cover most of the photo are left out.
pub fn boxes(body: &BodyMap, faces: &[NormRect], img: &RgbImage, max: usize) -> Vec<NormRect> {
    let (w, h) = (body.w, body.h);
    let owners = crate::people::assign(&body.person(), &body.skin_or_hair(), w, h, faces);
    let mut extent = vec![(usize::MAX, usize::MAX, 0usize, 0usize); owners.people];
    for (i, &o) in owners.owner.iter().enumerate() {
        if let Some(b) = usize::from(o).checked_sub(1).and_then(|k| extent.get_mut(k)) {
            let (x, y) = (i % w, i / w);
            *b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1));
        }
    }
    // a face's box is a fraction of the photo; its pixels are wider or taller by the photo's shape
    let aspect = img.width.max(1) as f32 / img.height.max(1) as f32;
    let mut out = Vec::new();
    for (k, e) in extent.iter().enumerate().take(max) {
        let piece = (e.0 < e.2 && e.1 < e.3)
            .then(|| NormRect::new(e.0 as f32 / w as f32, e.1 as f32 / h as f32, e.2 as f32 / w as f32, e.3 as f32 / h as f32));
        let b = match (faces.get(k), piece) {
            (Some(f), Some(p)) => {
                let around = body_box(f, aspect);
                let held = NormRect::new(p.x0.max(around.x0), p.y0.max(around.y0), p.x1.min(around.x1), p.y1.min(around.y1));
                // the box always holds the face itself
                if held.is_valid() { held.union(f) } else { around }
            }
            (Some(f), None) => body_box(f, aspect),
            (None, Some(p)) => p,
            (None, None) => continue,
        };
        let (mx, my) = (b.width() * 0.05, b.height() * 0.05);
        let Some(b) = NormRect::new(b.x0 - mx, b.y0 - my, b.x1 + mx, b.y1 + my).clamped() else { continue };
        if b.width() * b.height() < WHOLE_SHARE {
            out.push(b);
        }
    }
    out
}

/// Run Sapiens2 over each box and put its result into `probs` (the whole photo's class probabilities on the body
/// map's `grid` of `w × h` cells, class-major), blending where boxes overlap and fading at their edges.
pub fn refine(
    net: &Sapiens2,
    img: &RgbImage,
    grid: (usize, usize),
    probs: &mut [f32],
    boxes: &[NormRect],
    dev: &Device,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let (w, h) = grid;
    let n = w * h;
    if boxes.is_empty() || probs.len() != sapiens::CLASSES * n {
        return Ok(());
    }
    let mut acc: Vec<f32> = probs.iter().map(|p| p * WHOLE_WEIGHT).collect();
    let mut weight = vec![WHOLE_WEIGHT; n];
    for b in boxes {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        // the box, out to whole grid cells
        let gx0 = ((b.x0 * w as f32).floor().max(0.0) as usize).min(w);
        let gy0 = ((b.y0 * h as f32).floor().max(0.0) as usize).min(h);
        let gx1 = ((b.x1 * w as f32).ceil().max(0.0) as usize).min(w);
        let gy1 = ((b.y1 * h as f32).ceil().max(0.0) as usize).min(h);
        let (cw, ch) = (gx1.saturating_sub(gx0), gy1.saturating_sub(gy0));
        if cw < 2 || ch < 2 {
            continue;
        }
        let local = pass(net, img, (gx0, gy0, cw, ch), (w, h), dev)?;
        for y in 0..ch {
            for x in 0..cw {
                let wt = fade(x, cw, gx0, w).min(fade(y, ch, gy0, h));
                let i = (gy0 + y) * w + gx0 + x;
                if let Some(t) = weight.get_mut(i) {
                    *t += wt;
                }
                for c in 0..sapiens::CLASSES {
                    if let (Some(a), Some(&p)) = (acc.get_mut(c * n + i), local.get(c * cw * ch + y * cw + x)) {
                        *a += wt * p;
                    }
                }
            }
        }
    }
    for (i, p) in probs.iter_mut().enumerate() {
        if let (Some(&a), Some(&t)) = (acc.get(i), weight.get(i % n.max(1))) {
            *p = a / t;
        }
    }
    Ok(())
}

/// How much a box's result counts at cell `at` of its `len` cells along one axis (the box starts at cell `start` of
/// the grid's `total`): rising from its edges over [`FEATHER`] of its length, except at an edge on the photo's border.
fn fade(at: usize, len: usize, start: usize, total: usize) -> f32 {
    let lo = if start == 0 { f32::INFINITY } else { at as f32 + 0.5 };
    let hi = if start + len >= total { f32::INFINITY } else { len.saturating_sub(at) as f32 - 0.5 };
    (lo.min(hi) / (FEATHER * len as f32).max(1.0)).min(1.0)
}

/// Sapiens2's class probabilities over the grid cells `cells` (`x0, y0, width, height`) of a `grid` covering the
/// photo, from a pass over that part of the photo alone.
fn pass(net: &Sapiens2, img: &RgbImage, cells: (usize, usize, usize, usize), grid: (usize, usize), dev: &Device) -> Result<Vec<f32>> {
    let (gx0, gy0, cw, ch) = cells;
    let (sx, sy) = (img.width as f32 / grid.0 as f32, img.height as f32 / grid.1 as f32);
    let (px0, py0, pw, ph) = (gx0 as f32 * sx, gy0 as f32 * sy, cw as f32 * sx, ch as f32 * sy);
    let (iw, ih) = input_size(cw as f32 * CELL_SAMPLES, ch as f32 * CELL_SAMPLES);
    let (ux, uy) = (pw / iw as f32, ph / ih as f32);
    let px = sample_grid(img, iw, ih, ux.max(uy), [0.485, 0.456, 0.406], |u, v| (px0 + u * ux, py0 + v * uy));
    let logits = net.forward(&sapiens::input(px, iw, ih, dev)?)?;
    let down = crate::nn::resize(&logits, ch, cw, false)?;
    let probs = candle_nn::ops::softmax(&down, 1)?.flatten_all()?.to_vec1::<f32>()?;
    if probs.len() != sapiens::CLASSES * cw * ch {
        return Err(Error::Model(format!("Sapiens2 gave {} values, not {}", probs.len(), sapiens::CLASSES * cw * ch)));
    }
    Ok(probs)
}

/// The model's input for a box wanted at `w × h` (its grid cells times [`CELL_SAMPLES`]): the box's shape, the long
/// side at least [`MIN_LONG`] (the model needs a person some pixels tall), never past the trained number of patches,
/// sides in multiples of the patch.
fn input_size(w: f32, h: f32) -> (usize, usize) {
    let area = (sapiens::TRAIN_H * sapiens::TRAIN_W) as f32;
    let (pw, ph) = (if w.is_finite() { w.max(1.0) } else { 1.0 }, if h.is_finite() { h.max(1.0) } else { 1.0 });
    let s = (MIN_LONG / pw.max(ph)).max(1.0).min((area / (pw * ph)).sqrt());
    let p = sapiens::PATCH as f32;
    let side = |v: f32| (((v * s) / p).round().clamp(2.0, 256.0) * p) as usize;
    let (mut w, mut h) = (side(pw), side(ph));
    while w * h > (area * 1.05) as usize && w > 32 && h > 32 {
        if w >= h { w -= sapiens::PATCH } else { h -= sapiens::PATCH }
    }
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_is_at_most_the_trained_size_and_small_ones_are_enlarged() {
        assert_eq!(input_size(768.0, 1024.0), (768, 1024));
        let (w, h) = input_size(3000.0, 4000.0);
        assert!(w * h <= 1024 * 768 * 105 / 100 && w % 16 == 0 && h % 16 == 0);
        assert_eq!(input_size(100.0, 200.0), (192, 384), "a small box grows to the shortest long side");
        assert_eq!(input_size(300.0, 500.0), (304, 496), "a box big enough keeps its size");
        let (w, h) = input_size(0.0, f32::NAN);
        assert!(w >= 32 && h >= 32);
    }

    #[test]
    fn each_face_gets_a_body_box_and_a_whole_photo_portrait_gets_none() {
        let img = RgbImage::new(200, 100, vec![0; 200 * 100 * 3]).unwrap();
        // no person area at all: the boxes come from the faces
        let empty = BodyMap { w: 20, h: 10, probs: [vec![1.0; 200], vec![0.0; 17 * 200]].concat() };
        let faces = [NormRect::new(0.1, 0.1, 0.15, 0.2), NormRect::new(0.6, 0.2, 0.65, 0.3)];
        let b = boxes(&empty, &faces, &img, MAX_CROPS);
        assert_eq!(b.len(), 2);
        assert!(b[0].x0 < 0.1 && b[0].x1 > 0.15 && b[0].y1 > 0.9, "{:?}", b[0]);
        assert!(boxes(&empty, &faces, &img, 1).len() == 1);
        // a face as big as the photo: its body box covers most of it, so the whole photo's pass is enough
        assert!(boxes(&empty, &[NormRect::new(0.3, 0.1, 0.7, 0.9)], &img, MAX_CROPS).is_empty());
    }

    #[test]
    fn a_box_fades_in_from_its_inner_edges_only() {
        assert_eq!(fade(0, 40, 0, 100), 1.0, "an edge on the photo's border does not fade");
        assert_eq!(fade(39, 40, 60, 100), 1.0);
        assert!(fade(0, 40, 10, 100) < 0.2 && fade(39, 40, 10, 100) < 0.2);
        assert_eq!(fade(20, 40, 10, 100), 1.0, "the middle is the box's alone");
        assert!(fade(1, 40, 10, 100) > fade(0, 40, 10, 100));
    }
}
