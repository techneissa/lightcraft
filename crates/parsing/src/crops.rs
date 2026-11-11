//! Sapiens2 up close: the model is trained on pictures framed around one person, and in a whole photo of several
//! people it is unsure of those further back (dark clothes there read as background). Each person, largest face first,
//! gets a pass of their own over a box around their body. Each cell of the body map belongs to one person's box (the
//! one it is deepest inside) and takes that box's pass, or the whole photo's when the box had none.

use candle_core::{Device, Tensor};

use crate::body::BodyMap;
use crate::image::{RgbImage, sample_grid};
use crate::sapiens;
use crate::types::NormRect;
use crate::{Error, Result};

/// The most people that get a pass of their own (the others keep the whole photo's).
pub const MAX_CROPS: usize = 8;
/// Model pixels per grid cell of a box: its result is brought down to the grid's cells, so more is wasted time.
const CELL_SAMPLES: f32 = 1.5;
/// A box's long side is never smaller than this for the model.
const MIN_LONG: f32 = 384.0;
/// All the people's passes together see at most this many times the trained number of patches (eight large people
/// would otherwise take about 20 s); past it each box's pass is made smaller by the same factor.
const BUDGET: f32 = 2.0;
/// A box over this share of the photo was seen about as well by the whole photo's pass.
const WHOLE_SHARE: f32 = 0.5;
/// The share of a box's size over which its say fades at its inner edges.
const FEATHER: f32 = 0.08;

/// Sapiens2's forward: a normalised `1 × 3 × H × W` input to `1 × 29 × H × W` logits (a stand-in in tests).
pub type Forward<'a> = &'a dyn Fn(&Tensor) -> Result<Tensor>;
/// A body box around a face: this many face widths across, from this many face heights above the face's top to this
/// many below it.
const BODY_WIDE: f32 = 4.0;
const BODY_ABOVE: f32 = 0.6;
const BODY_BELOW: f32 = 8.0;
/// What the clothes parser sees of a person counts this far (a share of the body box's size) beyond the body box.
const REACH: f32 = 0.25;

/// The box around a face where its body may be, in fractions of the photo.
fn body_box(face: &NormRect, aspect: f32) -> NormRect {
    let (cx, _) = face.centre();
    let fw = face.width().max(face.height() / aspect.max(1e-3));
    let fh = face.height();
    NormRect::new(cx - BODY_WIDE * fw / 2.0, face.y0 - BODY_ABOVE * fh, cx + BODY_WIDE * fw / 2.0, face.y0 + BODY_BELOW * fh)
}

/// The people's boxes (see [`placed`]).
#[cfg(test)]
fn people_boxes(body: &BodyMap, faces: &[NormRect], img: &RgbImage) -> Vec<NormRect> {
    placed(body, faces, img).0
}

/// Where each person is: the body box around their face, grown to take in what the clothes parser saw of them close
/// by (an arm held out), or for a person without a face the clothes parser's piece; largest face first. Sapiens2
/// decides only inside these boxes: it is trained on pictures with people in them, and in a photo without any (a car
/// at night) it calls much of the picture a person. With them, for each cell of the body map, the box of the person
/// the clothes parser sees there (`None`: nobody, or a person without a box).
fn placed(body: &BodyMap, faces: &[NormRect], img: &RgbImage) -> (Vec<NormRect>, Vec<Option<usize>>) {
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
    let mut box_of = vec![None; owners.people];
    for (k, e) in extent.iter().enumerate() {
        let piece = (e.0 < e.2 && e.1 < e.3)
            .then(|| NormRect::new(e.0 as f32 / w as f32, e.1 as f32 / h as f32, e.2 as f32 / w as f32, e.3 as f32 / h as f32));
        let b = match (faces.get(k), piece) {
            (Some(f), Some(p)) => {
                let around = body_box(f, aspect);
                let reach = grow(&around, REACH);
                let near = NormRect::new(p.x0.max(reach.x0), p.y0.max(reach.y0), p.x1.min(reach.x1), p.y1.min(reach.y1));
                if near.is_valid() { around.union(&near) } else { around }
            }
            (Some(f), None) => body_box(f, aspect),
            (None, Some(p)) => grow(&p, 0.15),
            (None, None) => continue,
        };
        if let Some(b) = grow(&b, 0.05).clamped() {
            if let Some(slot) = box_of.get_mut(k) {
                *slot = Some(out.len());
            }
            out.push(b);
        }
    }
    let stands = owners.owner.iter().map(|&o| usize::from(o).checked_sub(1).and_then(|k| box_of.get(k).copied().flatten())).collect();
    (out, stands)
}

/// Which people's boxes get a pass of their own: the first `max` of those under half the photo (a box over most of
/// it was seen about as well by the whole photo's pass).
pub fn crops(people: &[NormRect], max: usize) -> Vec<bool> {
    let mut left = max;
    people
        .iter()
        .map(|b| {
            let take = left > 0 && b.width() * b.height() < WHOLE_SHARE;
            if take {
                left -= 1;
            }
            take
        })
        .collect()
}

/// The grid cells a box covers: `x0, y0, width, height`, out to whole cells, inside the grid.
fn cells(b: &NormRect, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let gx0 = ((b.x0 * w as f32).floor().max(0.0) as usize).min(w);
    let gy0 = ((b.y0 * h as f32).floor().max(0.0) as usize).min(h);
    let gx1 = ((b.x1 * w as f32).ceil().max(0.0) as usize).min(w);
    let gy1 = ((b.y1 * h as f32).ceil().max(0.0) as usize).min(h);
    (gx0, gy0, gx1.saturating_sub(gx0), gy1.saturating_sub(gy0))
}

/// Each person's say at each cell of their box: 1 inside, fading at its inner edges, `(cell, weight)`.
fn weights(b: &NormRect, w: usize, h: usize) -> impl Iterator<Item = (usize, f32)> {
    let (gx0, gy0, cw, ch) = cells(b, w, h);
    (0..ch).flat_map(move |y| (0..cw).map(move |x| ((gy0 + y) * w + gx0 + x, fade(x, cw, gx0, w).min(fade(y, ch, gy0, h)))))
}

/// How much Sapiens2 counts at each cell of a `w × h` grid: 1 inside the people's boxes, fading to 0 toward the edge of
/// the area they cover together. Where one box meets another it does not fade: a dip there would hand a thin line
/// across both people back to the clothes parser (#124).
pub fn presence(people: &[NormRect], w: usize, h: usize) -> Vec<f32> {
    let n = w * h;
    let mut covered = vec![false; n];
    // how far each cell's fade reaches along each axis: the longest of its boxes'
    let mut reach = vec![(0.0f32, 0.0f32); n];
    for b in people {
        let (_, _, cw, ch) = cells(b, w, h);
        let r = ((FEATHER * cw as f32).max(1.0), (FEATHER * ch as f32).max(1.0));
        for (i, _) in weights(b, w, h) {
            if let (Some(c), Some(o)) = (covered.get_mut(i), reach.get_mut(i)) {
                *c = true;
                *o = (o.0.max(r.0), o.1.max(r.1));
            }
        }
    }
    let at = |i: usize| covered.get(i).copied().unwrap_or(false);
    let dx = inward(w, h, |line, k| at(line * w + k));
    let dy = inward(h, w, |line, k| at(k * w + line));
    let mut out = vec![0.0f32; n];
    for (i, o) in out.iter_mut().enumerate() {
        let (Some(&r), Some(&x), Some(&y)) = (reach.get(i), dx.get(i), dy.get((i % w) * h + i / w)) else { continue };
        if at(i) {
            *o = (x / r.0).min(y / r.1).min(1.0);
        }
    }
    out
}

/// Along each of `lines` lines of `len` cells, how far each covered cell is from the nearest uncovered one, less half
/// a cell (infinite toward the photo's border, which never fades); `line * len + k` order.
fn inward(len: usize, lines: usize, covered: impl Fn(usize, usize) -> bool) -> Vec<f32> {
    let mut out = vec![f32::INFINITY; len * lines];
    for line in 0..lines {
        let row = out.get_mut(line * len..(line + 1) * len).unwrap_or_default();
        let mut last = None;
        for (k, o) in row.iter_mut().enumerate() {
            if !covered(line, k) {
                last = Some(k);
            }
            *o = last.map_or(f32::INFINITY, |l| (k - l) as f32 - 0.5);
        }
        let mut next = None;
        for (k, o) in row.iter_mut().enumerate().rev() {
            if !covered(line, k) {
                next = Some(k);
            }
            *o = o.min(next.map_or(f32::INFINITY, |l| (l - k) as f32 - 0.5));
        }
    }
    out
}

/// The box each cell belongs to: the box of the person the clothes parser sees there (`stands`, per cell) when it
/// covers the cell; else the box it is deepest inside (the greatest say), on a tie the one whose middle is nearest,
/// then the smaller box (a person in front of or inside another's box); `None` outside every box. Two
/// boxes of people side by side both count fully over much of their overlap: without the first two rules the smaller
/// one's pass decided the other person's body there, cut off along a straight line (#124).
fn owners(people: &[NormRect], stands: &[Option<usize>], w: usize, h: usize) -> Vec<Option<usize>> {
    let mut owner = vec![None; w * h];
    let mut best = vec![(false, 0.0f32, f32::NEG_INFINITY, f32::INFINITY); w * h];
    for (k, b) in people.iter().enumerate() {
        let area = b.width() * b.height();
        let (mx, my) = b.centre();
        let (mx, my) = (mx * w as f32, my * h as f32);
        for (i, v) in weights(b, w, h) {
            let here = stands.get(i).copied().flatten() == Some(k);
            let (u, t) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let near = -((u - mx).powi(2) + (t - my).powi(2));
            let key = (here, v, near, area);
            if let (Some(o), Some(m)) = (owner.get_mut(i), best.get_mut(i))
                && (key.0, key.1, key.2, -key.3).partial_cmp(&(m.0, m.1, m.2, -m.3)) == Some(std::cmp::Ordering::Greater)
            {
                *o = Some(k);
                *m = key;
            }
        }
    }
    owner
}

/// `r` grown by `by` of its size on each side.
fn grow(r: &NormRect, by: f32) -> NormRect {
    let (mx, my) = (r.width() * by, r.height() * by);
    NormRect::new(r.x0 - mx, r.y0 - my, r.x1 + mx, r.y1 + my)
}

/// One Sapiens2 pass: the picture it sees (three planes of values 0 to 1, `iw × ih`, sides multiples of the patch) and
/// the grid its class probabilities are wanted on (`w × h`).
pub struct Pass {
    pub px: Vec<f32>,
    pub iw: usize,
    pub ih: usize,
    pub w: usize,
    pub h: usize,
}

/// Runs Sapiens2's passes over a photo.
pub trait Runner {
    /// The input a pass wanted at `w × h` gets (Core ML's canvas holds passes up to a size).
    fn fit(&self, w: usize, h: usize) -> (usize, usize) {
        (w, h)
    }

    /// Each pass's class probabilities on its grid (class-major), in order; fewer only when `cancelled`.
    fn probs(&self, passes: &[Pass], cancelled: &dyn Fn() -> bool) -> Result<Vec<Vec<f32>>>;
}

/// Sapiens2 on candle (or a stand-in in tests), one pass at a time.
pub struct OnCandle<'a> {
    pub net: Forward<'a>,
    pub dev: &'a Device,
}

impl Runner for OnCandle<'_> {
    fn probs(&self, passes: &[Pass], cancelled: &dyn Fn() -> bool) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(passes.len());
        for p in passes {
            if cancelled() {
                break;
            }
            let logits = (self.net)(&sapiens::input(p.px.clone(), p.iw, p.ih, self.dev)?)?;
            out.push(probs_of(&logits, p.w, p.h)?);
        }
        Ok(out)
    }
}

/// The class probabilities (class-major) on a `w × h` grid of Sapiens2's logits `1 × 29 × H × W`.
pub(crate) fn probs_of(logits: &Tensor, w: usize, h: usize) -> Result<Vec<f32>> {
    let down = crate::nn::resize(logits, h, w, false)?;
    let probs = candle_nn::ops::softmax(&down, 1)?.flatten_all()?.to_vec1::<f32>()?;
    if probs.len() != sapiens::CLASSES * w * h {
        return Err(Error::Model(format!("Sapiens2 gave {} values, not {}", probs.len(), sapiens::CLASSES * w * h)));
    }
    Ok(probs)
}

/// The clothes parser's `map` with Sapiens2's say inside the people's boxes (`faces`: the people's faces, largest
/// first). The first [`MAX_CROPS`] people get a pass of their own; a cell takes the pass of the box it belongs to, or
/// the whole photo's pass when that box had none (a portrait filling the frame, the ninth person), which runs only
/// then. The passes run together (`run`: Core ML packs several in a call).
pub fn parts_map(run: &dyn Runner, map: BodyMap, img: &RgbImage, faces: &[NormRect], cancelled: &dyn Fn() -> bool) -> Result<BodyMap> {
    parts_map_with(run, map, img, faces, cancelled, MAX_CROPS)
}

/// [`parts_map`] with at most `max` passes of their own.
fn parts_map_with(run: &dyn Runner, map: BodyMap, img: &RgbImage, faces: &[NormRect], cancelled: &dyn Fn() -> bool, max: usize) -> Result<BodyMap> {
    let (people, stands) = placed(&map, faces, img);
    if people.is_empty() {
        return Ok(map);
    }
    let (w, h) = (map.w, map.h);
    let cropped = crops(&people, max);
    let owner = owners(&people, &stands, w, h);
    let uncropped = |k: &usize| !cropped.get(*k).copied().unwrap_or(false);
    let whole = owner.iter().flatten().any(uncropped);
    let mine: Vec<(usize, &NormRect)> = people.iter().enumerate().filter(|(k, _)| !uncropped(k)).collect();
    let samples = samples(mine.iter().map(|(_, b)| cells(b, w, h)), CELL_SAMPLES);
    let mut passes = Vec::with_capacity(mine.len() + 1);
    if whole {
        passes.push(crate::fuse::whole_pass(run, img, w, h));
    }
    let mut boxes = Vec::with_capacity(mine.len());
    for (k, b) in mine {
        let at = cells(b, w, h);
        if at.2 < 2 || at.3 < 2 {
            continue;
        }
        passes.push(box_pass(run, img, (w, h), samples, at));
        boxes.push((k, at));
    }
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let out = run.probs(&passes, cancelled)?;
    if cancelled() {
        return Err(Error::Cancelled);
    }
    if out.len() != passes.len() {
        return Err(Error::Model(format!("Sapiens2 gave {} passes, not {}", out.len(), passes.len())));
    }
    let mut out = out.into_iter();
    let n = w * h;
    let mut probs = if whole { out.next().unwrap_or_default() } else { vec![0.0; sapiens::CLASSES * n] };
    if probs.len() != sapiens::CLASSES * n {
        return Err(Error::Model(format!("Sapiens2 gave {} values, not {}", probs.len(), sapiens::CLASSES * n)));
    }
    for ((k, at), local) in boxes.into_iter().zip(out) {
        place(&mut probs, &local, (w, h), at, |i| owner.get(i).copied().flatten() == Some(k));
    }
    Ok(crate::fuse::fuse(&map, &probs, &presence(&people, w, h), faces))
}

/// Write a box's class probabilities `local` (over its grid cells `at`: `x0, y0, width, height`) into `probs`
/// (class-major on the `w × h` grid) at the cells where `mine(cell)`.
fn place(probs: &mut [f32], local: &[f32], (w, h): (usize, usize), at: (usize, usize, usize, usize), mine: impl Fn(usize) -> bool) {
    let n = w * h;
    let (gx0, gy0, cw, ch) = at;
    for y in 0..ch {
        for x in 0..cw {
            let i = (gy0 + y) * w + gx0 + x;
            if !mine(i) {
                continue;
            }
            for c in 0..sapiens::CLASSES {
                if let (Some(o), Some(&p)) = (probs.get_mut(c * n + i), local.get(c * cw * ch + y * cw + x)) {
                    *o = p;
                }
            }
        }
    }
}

/// The pass over the grid cells `at` (`x0, y0, width, height`) of a `grid` over `img`, at `samples` model pixels a
/// cell: that part of the photo alone, its probabilities wanted on those cells.
fn box_pass(run: &dyn Runner, img: &RgbImage, grid: (usize, usize), samples: f32, at: (usize, usize, usize, usize)) -> Pass {
    let (gx0, gy0, cw, ch) = at;
    let (sx, sy) = (img.width as f32 / grid.0 as f32, img.height as f32 / grid.1 as f32);
    let (px0, py0, pw, ph) = (gx0 as f32 * sx, gy0 as f32 * sy, cw as f32 * sx, ch as f32 * sy);
    let (iw, ih) = input_size(cw as f32 * samples, ch as f32 * samples);
    let (iw, ih) = run.fit(iw, ih);
    let (ux, uy) = (pw / iw as f32, ph / ih as f32);
    let px = sample_grid(img, iw, ih, ux.max(uy), [0.485, 0.456, 0.406], |u, v| (px0 + u * ux, py0 + v * uy));
    Pass { px, iw, ih, w: cw, h: ch }
}

/// How much a box's result counts at cell `at` of its `len` cells along one axis (the box starts at cell `start` of
/// the grid's `total`): rising from its edges over [`FEATHER`] of its length, except at an edge on the photo's border.
fn fade(at: usize, len: usize, start: usize, total: usize) -> f32 {
    let lo = if start == 0 { f32::INFINITY } else { at as f32 + 0.5 };
    let hi = if start + len >= total { f32::INFINITY } else { len.saturating_sub(at) as f32 - 0.5 };
    (lo.min(hi) / (FEATHER * len as f32).max(1.0)).min(1.0)
}

/// Model pixels per grid cell for boxes of these `cells`: `wanted`, or less when their passes together would see more
/// than [`BUDGET`] times the trained number of patches.
fn samples(cells: impl Iterator<Item = (usize, usize, usize, usize)>, wanted: f32) -> f32 {
    let trained = (sapiens::TRAIN_H * sapiens::TRAIN_W) as f32;
    let total: f32 = cells.map(|(_, _, cw, ch)| (cw * ch) as f32 * wanted * wanted).map(|a| a.min(trained)).sum();
    if total > BUDGET * trained { wanted * (BUDGET * trained / total).sqrt() } else { wanted }
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
    fn each_face_gets_a_body_box_and_a_whole_photo_portrait_no_pass_of_its_own() {
        let img = RgbImage::new(200, 100, vec![0; 200 * 100 * 3]).unwrap();
        // no person area at all: the boxes come from the faces
        let empty = BodyMap { w: 20, h: 10, probs: [vec![1.0; 200], vec![0.0; 17 * 200]].concat() };
        let faces = [NormRect::new(0.1, 0.1, 0.15, 0.2), NormRect::new(0.6, 0.2, 0.65, 0.3)];
        let people = people_boxes(&empty, &faces, &img);
        assert_eq!(people.len(), 2);
        assert!(people[0].x0 < 0.1 && people[0].x1 > 0.15 && people[0].y1 > 0.9, "{:?}", people[0]);
        assert_eq!(crops(&people, 1), vec![true, false]);
        // a face as big as the photo: its body box covers most of it, so the whole photo's pass is enough
        let big = people_boxes(&empty, &[NormRect::new(0.3, 0.1, 0.7, 0.9)], &img);
        assert_eq!(big.len(), 1);
        assert_eq!(crops(&big, MAX_CROPS), vec![false]);
        // no faces and no person area: nowhere for Sapiens2 to decide
        assert!(people_boxes(&empty, &[], &img).is_empty());
        assert!(presence(&[], 20, 10).iter().all(|&v| v == 0.0));
    }

    #[test]
    fn presence_is_whole_inside_a_box_and_nothing_outside() {
        let p = presence(&[NormRect::new(0.0, 0.0, 0.5, 1.0)], 20, 10);
        assert_eq!(p[5 * 20 + 2], 1.0);
        assert_eq!(p[5 * 20 + 15], 0.0);
        assert!(p[5 * 20 + 9] < 1.0, "it fades toward its inner edge");
    }

    #[test]
    fn a_box_fades_in_from_its_inner_edges_only() {
        assert_eq!(fade(0, 40, 0, 100), 1.0, "an edge on the photo's border does not fade");
        assert_eq!(fade(39, 40, 60, 100), 1.0);
        assert!(fade(0, 40, 10, 100) < 0.2 && fade(39, 40, 10, 100) < 0.2);
        assert_eq!(fade(20, 40, 10, 100), 1.0, "the middle is the box's alone");
        assert!(fade(1, 40, 10, 100) > fade(0, 40, 10, 100));
    }

    // ---- placement, with a stand-in for the network (no model needed) ----

    use crate::sapiens::class;

    const GRID: (usize, usize) = (40, 24);

    /// A black 200 × 120 photo (5 px per grid cell) with `patches` (cell rectangles `x0, y0, x1, y1`) in `colour`.
    fn photo(patches: &[((usize, usize, usize, usize), [u8; 3])]) -> RgbImage {
        let (w, h) = (200usize, 120usize);
        let mut d = vec![0u8; w * h * 3];
        for &((x0, y0, x1, y1), c) in patches {
            for y in y0 * 5..y1 * 5 {
                for x in x0 * 5..x1 * 5 {
                    d[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
                }
            }
        }
        RgbImage::new(w, h, d).unwrap()
    }

    /// The clothes parser seeing nothing (the boxes come from the faces alone).
    fn nothing() -> BodyMap {
        let n = GRID.0 * GRID.1;
        BodyMap { w: GRID.0, h: GRID.1, probs: [vec![1.0; n], vec![0.0; (crate::body::CLASSES - 1) * n]].concat() }
    }

    /// Logits with `class(r, g, b)` (the normalised input's channels) winning at each input pixel.
    fn logits(x: &Tensor, class: &dyn Fn(usize, usize, f32, f32, f32) -> usize) -> Result<Tensor> {
        let (_, _, h, w) = x.dims4()?;
        let v = x.flatten_all()?.to_vec1::<f32>()?;
        let n = w * h;
        let mut out = vec![0.0f32; sapiens::CLASSES * n];
        for i in 0..n {
            out[class(w, h, v[i], v[n + i], v[2 * n + i]) * n + i] = 10.0;
        }
        Ok(Tensor::from_vec(out, (1, sapiens::CLASSES, h, w), x.device())?)
    }

    /// Red pixels are upper clothing, blue ones lower clothing, the rest background.
    fn by_colour(_: usize, _: usize, r: f32, _: f32, b: f32) -> usize {
        if r > 1.0 {
            class::UPPER_CLOTHING
        } else if b > 1.0 {
            class::LOWER_CLOTHING
        } else {
            class::BACKGROUND
        }
    }

    fn atr(m: &BodyMap, c: usize, x: usize, y: usize) -> f32 {
        m.probs[c * m.w * m.h + y * m.w + x]
    }

    const UPPER: usize = 4;
    const PANTS: usize = 6;
    const HAIR: usize = 2;

    #[test]
    fn a_pass_lands_on_the_cells_it_saw_at_the_edge_and_in_the_middle() {
        // a person at the top-left corner, their shirt in cells x 2..5, y 7..10; one in the middle, cells x 21..24,
        // y 15..18
        let img = photo(&[((2, 7, 5, 10), [255, 0, 0]), ((21, 15, 24, 18), [255, 0, 0])]);
        let faces = [NormRect::new(0.0, 0.0, 0.1, 0.15), NormRect::new(0.5, 0.35, 0.58, 0.48)];
        let net = |x: &Tensor| logits(x, &by_colour);
        let m = parts_map(&OnCandle { net: &net, dev: &Device::Cpu }, nothing(), &img, &faces, &|| false).unwrap();
        for (x0, y0, x1, y1) in [(2, 7, 5, 10), (21, 15, 24, 18)] {
            for y in y0..y1 {
                for x in x0..x1 {
                    assert!(atr(&m, UPPER, x, y) > 0.95, "({x}, {y}): {}", atr(&m, UPPER, x, y));
                }
            }
            // one cell off to each side is not the shirt
            for (x, y) in [(x0 - 1, y0), (x1, y0), (x0, y0 - 1), (x0, y1)] {
                assert!(atr(&m, UPPER, x, y) < 0.05, "({x}, {y}) is next to the shirt: {}", atr(&m, UPPER, x, y));
            }
        }
    }

    #[test]
    fn where_two_boxes_overlap_a_cell_takes_one_pass_not_a_mix() {
        // two people side by side: a red half and a blue half; each pass calls everything it sees by its main colour
        let img = photo(&[((0, 0, 20, 24), [255, 0, 0]), ((20, 0, 40, 24), [0, 0, 255])]);
        let faces = [NormRect::new(0.38, 0.1, 0.46, 0.22), NormRect::new(0.54, 0.1, 0.62, 0.22)];
        let net = |x: &Tensor| {
            let mean = |c: usize| x.narrow(1, c, 1).and_then(|t| t.mean_all()).and_then(|t| t.to_scalar::<f32>());
            let main = if mean(0)? > mean(2)? { class::UPPER_CLOTHING } else { class::LOWER_CLOTHING };
            logits(x, &|_, _, _, _, _| main)
        };
        let m = parts_map(&OnCandle { net: &net, dev: &Device::Cpu }, nothing(), &img, &faces, &|| false).unwrap();
        let people = people_boxes(&nothing(), &faces, &img);
        assert!(people[0].x1 > people[1].x0, "the boxes overlap: {people:?}");
        let overlap = ((people[1].x0 * 40.0).ceil() as usize + (people[0].x1 * 40.0).floor() as usize) / 2;
        for y in 6..18 {
            let (top, pants) = (atr(&m, UPPER, overlap, y), atr(&m, PANTS, overlap, y));
            assert!(top > 0.95 || pants > 0.95, "cell ({overlap}, {y}) mixes the two passes: top {top}, trousers {pants}");
        }
        assert!(atr(&m, UPPER, 14, 12) > 0.95 && atr(&m, PANTS, 25, 12) > 0.95, "each person's own pass");
    }

    #[test]
    fn where_two_boxes_meet_the_mask_runs_on_without_a_seam() {
        // two people side by side whose boxes just meet; both shirts are red, and the clothes parser sees nothing, so
        // a dip in Sapiens2's say at the boxes' shared edge would show as a line of background (#124)
        let img = photo(&[((0, 0, 40, 24), [255, 0, 0])]);
        let faces = [NormRect::new(0.23, 0.1, 0.33, 0.2), NormRect::new(0.67, 0.1, 0.77, 0.2)];
        let people = people_boxes(&nothing(), &faces, &img);
        let (a, b) = (cells(&people[0], 40, 24), cells(&people[1], 40, 24));
        assert!(a.0 + a.2 >= b.0 && a.0 + a.2 <= b.0 + 1, "the boxes meet with at most a cell in common: {a:?} {b:?}");
        let net = |x: &Tensor| logits(x, &by_colour);
        let m = parts_map(&OnCandle { net: &net, dev: &Device::Cpu }, nothing(), &img, &faces, &|| false).unwrap();
        for y in 6..18 {
            for x in 11..29 {
                assert!(atr(&m, UPPER, x, y) > 0.95, "({x}, {y}) breaks the shirt: {}", atr(&m, UPPER, x, y));
            }
        }
        let p = presence(&people, 40, 24);
        assert!(p[12 * 40 + a.0 + a.2 - 1] == 1.0 && p[12 * 40 + b.0] == 1.0, "no fade where one box meets another");
        assert!(p[12 * 40 + a.0] < 1.0 && p[12 * 40 + b.0 + b.2 - 1] < 1.0, "the outer edges still fade");
    }

    #[test]
    fn a_person_without_a_pass_inside_anothers_box_keeps_the_whole_photo_pass() {
        let img = photo(&[]);
        // a large face, and a small one inside its body box; only the first gets a pass
        let faces = [NormRect::new(0.42, 0.05, 0.5, 0.15), NormRect::new(0.45, 0.5, 0.48, 0.55)];
        let whole = crate::fuse::input_size(img.width, img.height);
        let net = |x: &Tensor| {
            let (_, _, h, w) = x.dims4()?;
            // the whole photo's pass says hair, a person's pass upper clothing
            let c = if (w, h) == whole { class::HAIR } else { class::UPPER_CLOTHING };
            logits(x, &|_, _, _, _, _| c)
        };
        let m = parts_map_with(&OnCandle { net: &net, dev: &Device::Cpu }, nothing(), &img, &faces, &|| false, 1).unwrap();
        let people = people_boxes(&nothing(), &faces, &img);
        assert_eq!(crops(&people, 1), vec![true, false]);
        let small = people[1];
        let (cx, cy) = small.centre();
        let (x, y) = ((cx * 40.0) as usize, (cy * 24.0) as usize);
        assert!(atr(&m, HAIR, x, y) > 0.95, "the second person's cells take the whole photo's pass: hair {}", atr(&m, HAIR, x, y));
        assert!(atr(&m, UPPER, 20, 2) > 0.95, "the first person's own pass");
    }

    #[test]
    fn many_large_people_share_a_budget_and_few_keep_their_size() {
        let trained = (sapiens::TRAIN_H * sapiens::TRAIN_W) as f32;
        // three boxes of 200 × 500 cells: 1.5 samples a cell makes each 0.29 of the trained size, under the budget
        assert_eq!(samples([(0, 0, 200, 500); 3].into_iter(), 1.5), 1.5);
        // eight: 2.3 times the trained size, so each is made smaller to fit twice it
        let s = samples([(0, 0, 200, 500); 8].into_iter(), 1.5);
        assert!(s < 1.5);
        let total = 8.0 * 200.0 * 500.0 * s * s;
        assert!((total / trained - BUDGET).abs() < 0.01, "{}", total / trained);
        assert_eq!(samples(std::iter::empty(), 1.5), 1.5);
    }

    #[test]
    fn the_smaller_box_owns_a_cell_both_cover_fully() {
        let big = NormRect::new(0.0, 0.0, 1.0, 1.0);
        let small = NormRect::new(0.3, 0.3, 0.7, 0.7);
        let o = owners(&[big, small], &[], 10, 10);
        assert_eq!(o[5 * 10 + 5], Some(1));
        assert_eq!(o[0], Some(0));
        assert_eq!(owners(&[], &[], 2, 2), vec![None; 4]);
        assert_eq!(crops(&[small, big, small, small], 2), vec![true, false, true, false], "the big one never counts toward the cap");
    }

    #[test]
    fn where_two_boxes_count_fully_a_person_keeps_their_own_pass() {
        // two people side by side whose boxes cover most of the photo between them; the second box is smaller
        let (a, b) = (NormRect::new(0.0, 0.0, 0.8, 1.0), NormRect::new(0.3, 0.0, 0.95, 1.0));
        let (w, h) = (20, 10);
        let o = owners(&[a, b], &[], w, h);
        // the middle of both boxes: each side goes to the box whose middle is nearer, not all to the smaller box
        assert_eq!(o[5 * w + 8], Some(0), "x 0.43 is nearer the first box's middle");
        assert_eq!(o[5 * w + 14], Some(1), "x 0.73 is nearer the second's");
        // where the clothes parser sees the first person, their own pass decides
        let mut stands = vec![None; w * h];
        stands[5 * w + 14] = Some(0);
        assert_eq!(owners(&[a, b], &stands, w, h)[5 * w + 14], Some(0));
        // a person seen outside their own box does not pull the cell out of the other's
        stands[5 * w + 18] = Some(0);
        assert_eq!(owners(&[a, b], &stands, w, h)[5 * w + 18], Some(1));
    }
}
