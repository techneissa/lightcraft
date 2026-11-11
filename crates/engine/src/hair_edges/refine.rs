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

/// Every person's Entire Person edge in `p` snapped to `img` (the picture `p` was parsed from). The people are
/// snapped together: their joined coverage (one body a parse splits into pieces, a person and their hands found as
/// people of their own, adds up again) has its thin seams closed and its edge snapped at the picture's size, and each
/// person keeps their share of it over their box, as their one Entire Person plane. A seam between two pieces of one
/// body therefore never opens into a cut. Marks the parse's version; `Err` when stopped.
pub fn people(mut p: Parsing, img: &RgbImage, cancel: &Arc<AtomicBool>) -> Result<Parsing, String> {
    let (w, h) = (img.width, img.height);
    let ok = w > 0 && h > 0 && w.checked_mul(h).and_then(|n| n.checked_mul(3)) == Some(img.data.len());
    if ok && !p.people.is_empty() {
        let col = |i: usize| (i as f32 + 0.5) / w as f32;
        let row = |j: usize| (j as f32 + 0.5) / h as f32;
        let people = p.people.clone();
        let union = Alpha::from_fn(w, h, |fx, fy| lightcraft_parsing::joined(people.iter(), Part::EntirePerson, fx, fy));
        // (the parse's own cells, the coarsest of anyone's planes: its edge is uncertain over about one of them)
        let cell = people
            .iter()
            .flat_map(|q| q.parts.get(&Part::EntirePerson).into_iter().flatten())
            .filter(|q| q.is_valid())
            .map(|q| (q.rect.width() * w as f32 / q.width as f32).max(q.rect.height() * h as f32 / q.height as f32))
            .fold(1.0f32, f32::max);
        let mut snapped = union.clone();
        lightcraft_edgerefine::close_seams(&mut snapped, (cell.ceil() as usize + 1).clamp(2, 4));
        let stop = || cancel.load(Ordering::Relaxed);
        snap(&Rgb { data: &img.data, w, h }, &mut snapped, radius_for(cell), &stop).map_err(|_| "cancelled".to_string())?;
        for (k, person) in p.people.iter_mut().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let Some(rect) = super::people::entire_rect(person) else { continue };
            let Some(q) = people.get(k) else { continue };
            let x0 = ((rect.x0 * w as f32).floor().max(0.0) as usize).min(w - 1);
            let y0 = ((rect.y0 * h as f32).floor().max(0.0) as usize).min(h - 1);
            let x1 = ((rect.x1 * w as f32).ceil() as usize).clamp(x0 + 1, w);
            let y1 = ((rect.y1 * h as f32).ceil() as usize).clamp(y0 + 1, h);
            let (cw, ch) = (x1 - x0, y1 - y0);
            let mut share = Vec::with_capacity(cw * ch);
            for j in y0..y1 {
                for i in x0..x1 {
                    let mine = q.sample(Part::EntirePerson, col(i), row(j));
                    let (u, s) = (union.data.get(j * w + i).copied().unwrap_or(0.0), snapped.data.get(j * w + i).copied().unwrap_or(0.0));
                    // (their part of the snapped union: as much as they held of the union; what the union gained where
                    // nobody was goes to the person whose box it is in, when it is no one else's)
                    let v = if u > 1e-3 {
                        s * (mine / u).min(1.0)
                    } else if people
                        .iter()
                        .enumerate()
                        .all(|(o, r)| o == k || r.bounds.is_none_or(|b| !(b.x0..=b.x1).contains(&col(i)) || !(b.y0..=b.y1).contains(&row(j))))
                    {
                        s
                    } else {
                        0.0
                    };
                    share.push(v.clamp(0.0, 1.0));
                }
            }
            let a = Alpha { w: cw, h: ch, data: share };
            let r = NormRect::new(x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32);
            // (a parse file keeps planes of at most 1536 cells a side; split as the parse's were, so they add up again)
            let (pw, ph, data) = shrink(&a, lightcraft_parsing::cache::MAX_PLANE_SIDE as usize);
            person.parts.insert(Part::EntirePerson, vec![Plane { rect: r, width: pw, height: ph, data, split: was_split(q) }]);
        }
    }
    p.model_version = format!("{}+{VERSION}", p.model_version);
    Ok(p)
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

/// Whether a person's Entire Person planes were all cut from the whole photo's body map (they add up with other
/// people's at a shared line, `lightcraft_parsing::Plane::split`).
pub(super) fn was_split(person: &lightcraft_parsing::Person) -> bool {
    person.parts.get(&Part::EntirePerson).is_some_and(|planes| !planes.is_empty() && planes.iter().all(|q| q.split))
}
