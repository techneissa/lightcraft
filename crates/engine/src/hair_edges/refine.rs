//! Fine edges (fork, issue #240): a Subject, Background or Entire Person selection, once its model (and edge model)
//! has decided what is selected, snapped to the photo at a finer size and stored as fine grids (`lightcraft-edgerefine`).
//!
//! The models see the photo at about 1000 px (BiRefNet) or 2048 px (people parsing) and give 1024² or coarser grids:
//! 6 px cells on a 24 MP photo, so at 1:1 an edge is a soft blur and thin strands are lost. Here the selection's
//! coverage is drawn at [`WORK_EDGE`] px (the parse's own picture for people), its edge snapped to the photo's colour
//! edges by a guided filter, and kept as one 1024² grid plus up to four finer ones over the edge (Subject,
//! Background), or as each person's Entire Person plane at the picture's size (people; Select People draws its grids
//! from it). Marked [`SUFFIX`]: such grids are not filtered again by a render (`lightcraft_pipeline::masks`), so a
//! preview, a zoomed view and an export sample the same stored edge. Runs once per selection on the worker that made
//! it; dragging a slider never runs it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::SegMask;
use lightcraft_edgerefine::{Alpha, Rgb, radius_for, sigmoid, snap, store};
use lightcraft_parsing::{NormRect, Parsing, Part, Plane, RgbImage};

/// What a selection whose edge was refined here adds to its model id (`SegMask::model`).
pub const SUFFIX: &str = "+refined";
/// Changes when the refinement changes, so people parses refined the old way are made again.
pub const VERSION: &str = "refined1";
/// The long side of the picture a Subject or Background selection is refined at.
pub const WORK_EDGE: usize = 3072;
/// The side of the whole-photo grid a refined Subject or Background keeps.
const COARSE_SIDE: usize = 1024;

/// A Subject selection `base` (a grid over the whole photo) refined on the photo `rgb` (`w × h`, the oriented,
/// uncropped photo at the working size): the grid over the whole photo and the finer grids over its edge, all marked
/// `<base model>+refined`. `None` when it can't be refined (sizes that don't add up, a damaged grid, stopped): the
/// caller keeps `base`.
pub fn subject(base: &SegMask, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Option<(SegMask, Vec<SegMask>)> {
    let img = Rgb { data: rgb, w, h };
    if !img.is_valid() || base.rect.is_some() {
        return None;
    }
    let side = base.side as usize;
    let logits = base.logits()?;
    let at = |x: usize, y: usize| logits.get(y.min(side - 1) * side + x.min(side - 1)).copied().unwrap_or(-16.0);
    // the stored grid's coverage at each pixel, as a render samples it (bilinear logits, cell centres at (i + 0.5) / side)
    let mut alpha = Alpha::from_fn(w, h, |fx, fy| {
        let (gx, gy) = ((fx * side as f32 - 0.5).clamp(0.0, (side - 1) as f32), (fy * side as f32 - 0.5).clamp(0.0, (side - 1) as f32));
        let (x0, y0) = (gx as usize, gy as usize);
        let (tx, ty) = (gx - x0 as f32, gy - y0 as f32);
        let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
        let bot = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
        sigmoid(top * (1.0 - ty) + bot * ty)
    });
    let cell = (w.max(h) as f32 / side.max(1) as f32).max(1.0);
    let stop = || cancel.load(Ordering::Relaxed);
    snap(&img, &mut alpha, radius_for(cell), &stop).ok()?;
    let (coarse, tiles) = store(&alpha, COARSE_SIDE)?;
    let id = format!("{}{SUFFIX}", base.model.as_deref().unwrap_or("sam3"));
    let main = SegMask::from_logits(coarse.side, &coarse.logits).made_by(&id);
    let detail = tiles.iter().map(|t| SegMask::from_logits_in(t.side, &t.logits, t.rect).made_by(&id)).collect();
    Some((main, detail))
}

/// Every person's Entire Person edge in `p` snapped to `img` (the picture `p` was parsed from): each person's coverage
/// over their box at the picture's size, filtered, and kept as their one Entire Person plane at that size. A person
/// never grows into another one. Marks the parse's version; `Err` when stopped.
pub fn people(mut p: Parsing, img: &RgbImage, cancel: &Arc<AtomicBool>) -> Result<Parsing, String> {
    let (w, h) = (img.width, img.height);
    let ok = w > 0 && h > 0 && w.checked_mul(h).and_then(|n| n.checked_mul(3)) == Some(img.data.len());
    if ok && !p.people.is_empty() {
        let before = p.people.clone();
        for (k, person) in p.people.iter_mut().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let Some(rect) = super::people::entire_rect(person) else { continue };
            let others: Vec<&lightcraft_parsing::Person> = before.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, q)| q).collect();
            if let Some(plane) = person_plane(person, &others, rect, img, cancel) {
                person.parts.insert(Part::EntirePerson, vec![plane]);
            }
        }
    }
    p.model_version = format!("{}+{VERSION}", p.model_version);
    Ok(p)
}

/// One person's refined Entire Person plane over `r` at the picture's pixels.
fn person_plane(
    person: &lightcraft_parsing::Person,
    others: &[&lightcraft_parsing::Person],
    r: NormRect,
    img: &RgbImage,
    cancel: &Arc<AtomicBool>,
) -> Option<Plane> {
    let (w, h) = (img.width, img.height);
    let x0 = ((r.x0 * w as f32).floor().max(0.0) as usize).min(w - 1);
    let y0 = ((r.y0 * h as f32).floor().max(0.0) as usize).min(h - 1);
    let x1 = ((r.x1 * w as f32).ceil() as usize).clamp(x0 + 1, w);
    let y1 = ((r.y1 * h as f32).ceil() as usize).clamp(y0 + 1, h);
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut crop = Vec::with_capacity(cw * ch * 3);
    for y in y0..y1 {
        crop.extend_from_slice(img.data.get((y * w + x0) * 3..(y * w + x1) * 3)?);
    }
    let col = |i: usize| ((x0 + i) as f32 + 0.5) / w as f32;
    let row = |j: usize| ((y0 + j) as f32 + 0.5) / h as f32;
    let old = Alpha::from_fn(cw, ch, |fx, fy| {
        let (i, j) = ((fx * cw as f32) as usize, (fy * ch as f32) as usize);
        person.sample(Part::EntirePerson, col(i), row(j))
    });
    let mut alpha = old.clone();
    // (the parse's own cells: its planes are at most as fine as the picture)
    let cell = person
        .parts
        .get(&Part::EntirePerson)
        .into_iter()
        .flatten()
        .filter(|q| q.is_valid())
        .map(|q| (q.rect.width() * w as f32 / q.width as f32).max(q.rect.height() * h as f32 / q.height as f32))
        .fold(1.0f32, f32::max);
    let stop = || cancel.load(Ordering::Relaxed);
    snap(&Rgb { data: &crop, w: cw, h: ch }, &mut alpha, radius_for(cell), &stop).ok()?;
    // (never into another person: where someone else is, this person may only lose)
    for (k, (v, o)) in alpha.data.iter_mut().zip(&old.data).enumerate() {
        let (i, j) = (k % cw, k / cw);
        if *v > *o && lightcraft_parsing::joined(others.iter().copied(), Part::EntirePerson, col(i), row(j)) > 0.5 {
            *v = *o;
        }
    }
    let rect = NormRect::new(x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32);
    // (a parse file keeps planes of at most 1536 cells a side)
    let (pw, ph, data) = shrink(&alpha, lightcraft_parsing::cache::MAX_PLANE_SIDE as usize);
    Some(Plane { rect, width: pw, height: ph, data, split: false })
}

/// `a` area-averaged down so neither side is over `most` (as it is when it fits).
fn shrink(a: &Alpha, most: usize) -> (usize, usize, Vec<f32>) {
    let big = a.w.max(a.h);
    if big <= most || most == 0 {
        return (a.w, a.h, a.data.clone());
    }
    let (tw, th) = ((a.w * most).div_ceil(big).max(1), (a.h * most).div_ceil(big).max(1));
    let mut out = Vec::with_capacity(tw * th);
    for j in 0..th {
        let (ya, yb) = (j * a.h / th, ((j + 1) * a.h / th).max(j * a.h / th + 1).min(a.h));
        for i in 0..tw {
            let (xa, xb) = (i * a.w / tw, ((i + 1) * a.w / tw).max(i * a.w / tw + 1).min(a.w));
            let mut s = 0.0f32;
            for y in ya..yb {
                s += a.data.get(y * a.w + xa..y * a.w + xb).map_or(0.0, |r| r.iter().sum());
            }
            out.push(s / ((xb - xa) * (yb - ya)).max(1) as f32);
        }
    }
    (tw, th, out)
}
