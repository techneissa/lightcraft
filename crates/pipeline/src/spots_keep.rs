//! Spots drawn again only where an edit reaches (issue #82).
//!
//! Spots are drawn one after another on the white-balanced image, each reading the image as the earlier ones left
//! it, so every spot edit used to draw every spot again: adding the 101st spot cost a hundred times the first, and
//! moving one redrew all of them. A view now keeps the image before its spots ([`Kept::pre`]) and after them
//! ([`Kept::post`]). On the next render, [`redo`] compares the old spots with the new ones and finds the area an edit
//! changed (the boxes of the spots added, removed or changed); every spot that reads from that area is drawn again,
//! and what it reads joins the area, until nothing more is reached. That area is copied back from `pre` into a copy
//! of `post` and only those spots are drawn on it. Each spot drawn again then reads exactly the pixels the whole
//! sequence would have given it, so the result is the same as drawing every spot.

use std::cell::Cell;
use std::sync::Arc;

use lightcraft_develop::Spot;
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::{Plan, SourceInfo, spots};

thread_local! {
    static DRAWN: Cell<usize> = const { Cell::new(0) };
}

/// Spots drawn on this thread so far, by either renderer (a test counts the work of one edit with it).
pub fn spots_drawn() -> usize {
    DRAWN.with(Cell::get)
}

/// Count one spot drawn ([`spots_drawn`]).
pub fn note_spot() {
    DRAWN.with(|c| c.set(c.get().wrapping_add(1)));
}

/// `[x0, y0, x1, y1)` in output pixels.
pub type Rect = [usize; 4];

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Where one spot writes (its box) and every pixel its result depends on (the box, the source patch and, for an
/// automatic source, the rings the source search samples).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    pub write: Rect,
    pub read: Rect,
}

/// `spot`'s footprint in a `w × h` render of `frame`; `None` when it draws nothing there.
pub fn footprint(spot: &Spot, frame: &Frame, w: usize, h: usize, ppl: f64) -> Option<Footprint> {
    let l = spots::layout(spot, frame, w, h, ppl)?;
    let write = [l.bx, l.by, l.bx + l.bw, l.by + l.bh];
    let clamp = |x0: f64, y0: f64, x1: f64, y1: f64| -> Rect {
        let c = |v: f64, hi: usize| {
            if v.is_finite() {
                v.clamp(0.0, hi as f64) as usize
            } else if v > 0.0 {
                hi
            } else {
                0
            }
        };
        [c(x0.floor(), w), c(y0.floor(), h), c(x1.ceil(), w), c(y1.ceil(), h)]
    };
    let [x0, y0, x1, y1] = write.map(|v| v as f64);
    let read = match l.offset {
        // the patch at the offset, sampled bilinearly (a pixel more on each side)
        Some((dx, dy)) => {
            let s = clamp(x0 + dx as f64 - 2.0, y0 + dy as f64 - 2.0, x1 + dx as f64 + 2.0, y1 + dy as f64 + 2.0);
            [s[0].min(write[0]), s[1].min(write[1]), s[2].max(write[2]), s[3].max(write[3])]
        }
        // candidates up to 4.2 radii away, each scored on rings up to 1.6 radii around it (`spots::auto_source`)
        None => {
            let g = (4.2 * l.r as f64).ceil() + 2.0;
            clamp(x0 - g, y0 - g, x1 + g, y1 + g)
        }
    };
    Some(Footprint { write, read })
}

/// One spot as a render drew it: the spot and its footprint (`None`: not drawn by the Classic pass, because a
/// learned fill drew it or it misses the image).
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub spot: Spot,
    pub at: Footprint,
}

/// The [`Drawn`] entries of `spots` in a `w × h` render (`done`: the ones a learned fill drew).
pub fn entries(spots: &[Spot], done: &[bool], frame: &Frame, w: usize, h: usize, ppl: f64) -> Vec<Option<Drawn>> {
    spots
        .iter()
        .enumerate()
        .map(|(i, sp)| {
            if done.get(i).copied().unwrap_or(false) {
                return None;
            }
            footprint(sp, frame, w, h, ppl).map(|at| Drawn { spot: sp.clone(), at })
        })
        .collect()
}

/// How to turn the old spots' result into the new one's: copy `restore` back from the image before the spots,
/// then draw the spots marked in `draw`, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Redo {
    pub restore: Vec<Rect>,
    pub draw: Vec<bool>,
}

impl Redo {
    /// Spots it draws.
    pub fn drawn(&self) -> usize {
        self.draw.iter().filter(|d| **d).count()
    }
}

/// The work that turns the result of `old` into that of `new` (same image before the spots), or `None` when it is
/// every new spot (draw them all from the image before the spots).
pub fn redo(old: &[Option<Drawn>], new: &[Option<Drawn>]) -> Option<Redo> {
    // the spots changed: what lies between the longest common start and end
    let a = old.iter().zip(new).take_while(|(o, n)| o == n).count();
    let (old_rest, new_rest) = (old.get(a..).unwrap_or_default(), new.get(a..).unwrap_or_default());
    let b = old_rest.iter().rev().zip(new_rest.iter().rev()).take_while(|(o, n)| o == n).count();
    let changed =
        |v: &[Option<Drawn>]| v.get(..v.len().saturating_sub(b)).unwrap_or_default().iter().flatten().map(|d| d.at.write).collect::<Vec<_>>();
    let mut area = changed(old_rest);
    area.extend(changed(new_rest));
    let mut draw = vec![false; new.len()];
    // Every spot that reads the area is drawn again, and its box joins the area (it is copied back from before the
    // spots, then drawn). What it reads outside the area holds the old result, which is what it needs unless a later
    // spot drew there: then that joins the area too. Each round adds a spot.
    for _ in 0..=new.len() {
        let mut grew = false;
        for (i, d) in new.iter().enumerate() {
            let Some(d) = d else { continue };
            if draw.get(i).copied().unwrap_or(true) || !area.iter().any(|r| overlaps(r, &d.at.read)) {
                continue;
            }
            if let Some(slot) = draw.get_mut(i) {
                *slot = true;
            }
            area.push(d.at.write);
            let later = new.get(i + 1..).unwrap_or_default();
            if later.iter().flatten().any(|k| overlaps(&k.at.write, &d.at.read)) {
                area.push(d.at.read);
            }
            grew = true;
        }
        if !grew {
            break;
        }
    }
    // (drawing every one is cheaper from the image before the spots)
    let all = new.iter().zip(&draw).all(|(d, r)| d.is_none() || *r);
    (!all).then_some(Redo { restore: area, draw })
}

/// What a view keeps of its last render's spots: the image before them, after them, and how each was drawn.
pub struct Kept<I> {
    /// [`key`] of the image before the spots.
    pub key: u64,
    pub pre: Arc<I>,
    pub post: Arc<I>,
    pub drawn: Vec<Option<Drawn>>,
    /// The spots a learned fill drew (part of `pre`).
    pub done: Vec<bool>,
}

/// Key of the image the Classic spots are drawn on: everything [`Plan::pre_nr_key`] holds but those spots and the
/// red eye corrections (drawn after them). Learned spots are drawn before the white balance and so are part of it.
pub fn key(info: &SourceInfo, p: &Plan<'_>) -> u64 {
    let s = &*p.settings;
    let (wb_t, wb_tint) = crate::local::effective_wb(info, s);
    let learned: Vec<(usize, String)> =
        s.spots.iter().enumerate().filter(|(_, sp)| crate::spots_learned::is_learned(sp)).map(|(i, sp)| (i, format!("{sp:?}"))).collect();
    crate::hash_of((
        p.geo,
        (p.w, p.h),
        p.px_per_long.to_bits(),
        [wb_t, wb_tint, info.as_shot_temp, info.as_shot_tint].map(f64::to_bits),
        format!("{:?}", s.optics),
        crate::local_tone::enabled(info, s),
        p.src_long,
        p.draft && !learned.is_empty(),
        (!learned.is_empty()).then_some(p.native_long),
        learned,
    ))
}

/// The pixels of `rects` copied from `from` into `to` (images of the same size).
pub fn restore(to: &mut Rgb32f, from: &Rgb32f, rects: &[Rect]) {
    let w = to.width.min(from.width);
    for r in rects {
        let (x0, x1) = (r[0].min(w), r[2].min(w));
        for y in r[1]..r[3].min(to.height).min(from.height) {
            let (a, b) = (y * w + x0, y * w + x1);
            if let (Some(d), Some(s)) = (to.data.get_mut(a..b), from.data.get(a..b)) {
                d.copy_from_slice(s);
            }
        }
    }
}

/// [`crate::lin_cpu`] on `img` (the resampled source), reusing `kept` (the view's last spots) when its image before the
/// spots is this one: then only the spots an edit reaches are drawn. Returns what to keep for the next render
/// (`None` without spots).
pub fn lin_cpu(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>, kept: Option<&Kept<Rgb32f>>) -> Option<Arc<Kept<Rgb32f>>> {
    let s = &*p.settings;
    if s.spots.is_empty() {
        crate::lin_cpu(img, info, p);
        return None;
    }
    let key = key(info, p);
    let hit = kept.filter(|k| k.key == key && (k.pre.width, k.pre.height) == (img.width, img.height));
    let (pre, done) = match hit {
        Some(k) => (k.pre.clone(), k.done.clone()),
        None => {
            let done = crate::lin_cpu_pre(img, info, p);
            (Arc::new(img.clone()), done)
        }
    };
    let new = entries(&s.spots, &done, &p.frame, img.width, img.height, p.px_per_long);
    let plan = hit.and_then(|k| redo(&k.drawn, &new).map(|r| (k, r)));
    let mut work = match &plan {
        Some((k, r)) => {
            let mut work = (*k.post).clone();
            restore(&mut work, &pre, &r.restore);
            let skip: Vec<bool> = r.draw.iter().enumerate().map(|(i, d)| !d || done.get(i).copied().unwrap_or(false)).collect();
            spots::apply_except(&mut work, &s.spots, &skip, &p.frame, p.px_per_long);
            work
        }
        None => {
            let mut work = (*pre).clone();
            spots::apply_except(&mut work, &s.spots, &done, &p.frame, p.px_per_long);
            work
        }
    };
    let post = Arc::new(work.clone());
    crate::redeye::apply(&mut work, &p.eyes);
    *img = work;
    Some(Arc::new(Kept { key, pre, post, drawn: new, done }))
}

#[cfg(test)]
#[path = "tests_spots_keep.rs"]
mod tests;
