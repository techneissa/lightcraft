//! Renders that recompute only what an edit changed (issue #111).
//!
//! A view keeps, per output size, every stage of its last render on the device (the stage cache's entry: the
//! resampled source, the linear image, its planes) and now the finished output too ([`Last`]). A spot edit rewrites
//! only some boxes of the image after the spots (`spots_keep` knows which). Their [`TILE`]-pixel tiles are the linear
//! stage's dirty area; each later stage grows it by how far a change spreads through it (its reach, see [`cut`]) and
//! recomputes only its dirty tiles, from a cut of its kept input with that much margin, into a copy of its kept
//! output. The per-pixel stage then runs on the union of those tiles. Every filter is local, so the result has the
//! same bits as a whole render.
//!
//! The whole frame renders (and is kept) when anything but the spots changed ([`Rest`]), when the spot redo did not
//! start from the image the kept linear stage was made from, or for what stage 1 leaves out: masks that read the
//! image, red eye, LaMa spots and the camera's local tone map (the linear stage on the CPU).

mod cut;

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{Plan, RenderRequest, SourceInfo, local};

use crate::ctx::Buf;
use crate::render::{Cx, Planes, Prep, map};
use cut::{Part, paste};
pub(crate) use cut::{Reach, Rect};

/// Side of a tile, in output pixels.
pub const TILE: usize = 64;

thread_local! {
    static TILES: Cell<usize> = const { Cell::new(0) };
    static FRAME: RefCell<Option<FrameNote>> = const { RefCell::new(None) };
}

/// Output tiles the per-pixel stage computed on this thread so far, by renders with a view's stages (a whole render
/// counts every tile of its output): a test counts the work of one edit with it.
pub fn tiles_rendered() -> usize {
    TILES.with(Cell::get)
}

pub(crate) fn note_tiles(n: usize) {
    TILES.with(|c| c.set(c.get().wrapping_add(n)));
}

/// What the last render on this thread with a view's stages made, for whoever shows it: its output's number and, when
/// it started from an earlier output of the same view and size, that one's number and the rectangles that differ from
/// it (`[x0, y0, x1, y1)` in its pixels). Everything outside them has the earlier output's pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameNote {
    pub frame: u64,
    pub changed: Option<(u64, Vec<[usize; 4]>)>,
}

/// The [`FrameNote`] of the last render on this thread, taken (`None` when it kept nothing).
pub fn take_frame_note() -> Option<FrameNote> {
    FRAME.with(|f| f.borrow_mut().take())
}

pub(crate) fn note_frame(note: Option<FrameNote>) {
    FRAME.with(|f| *f.borrow_mut() = note);
}

static FRAMES: AtomicU64 = AtomicU64::new(1);

/// A new output number.
pub(crate) fn next_frame() -> u64 {
    FRAMES.fetch_add(1, Ordering::Relaxed)
}

/// A set of [`TILE`] × [`TILE`] tiles of a `w × h` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tiles {
    size: (usize, usize),
    cols: usize,
    on: Vec<bool>,
}

impl Tiles {
    pub(crate) fn none(size: (usize, usize)) -> Tiles {
        let cols = size.0.div_ceil(TILE);
        Tiles { size, cols, on: vec![false; cols * size.1.div_ceil(TILE)] }
    }

    pub(crate) fn all(size: (usize, usize)) -> Tiles {
        let mut t = Tiles::none(size);
        t.on.iter_mut().for_each(|v| *v = true);
        t
    }

    fn rows(&self) -> usize {
        self.on.len().checked_div(self.cols).unwrap_or(0)
    }

    /// The tiles holding any pixel of `rects`.
    pub(crate) fn of(size: (usize, usize), rects: &[Rect]) -> Tiles {
        let mut t = Tiles::none(size);
        for r in rects {
            let (x1, y1) = (r[2].min(size.0), r[3].min(size.1));
            if r[0] >= x1 || r[1] >= y1 {
                continue;
            }
            for ty in r[1] / TILE..y1.div_ceil(TILE) {
                for tx in r[0] / TILE..x1.div_ceil(TILE) {
                    if let Some(v) = t.on.get_mut(ty * t.cols + tx) {
                        *v = true;
                    }
                }
            }
        }
        t
    }

    pub(crate) fn count(&self) -> usize {
        self.on.iter().filter(|v| **v).count()
    }

    pub(crate) fn len(&self) -> usize {
        self.on.len()
    }

    /// The tiles holding a pixel within `reach` of one of these.
    pub(crate) fn grow(&self, reach: Reach) -> Tiles {
        let (kx, ky) = (reach.0.div_ceil(TILE), reach.1.div_ceil(TILE));
        let (cols, rows) = (self.cols, self.rows());
        // across, then down (a square grows separably)
        let mut across = vec![false; self.on.len()];
        for y in 0..rows {
            for x in 0..cols {
                if self.on.get(y * cols + x).copied().unwrap_or(false) {
                    for v in across.iter_mut().skip(y * cols + x.saturating_sub(kx)).take((x + kx).min(cols - 1) + 1 - x.saturating_sub(kx)) {
                        *v = true;
                    }
                }
            }
        }
        let mut on = vec![false; self.on.len()];
        for y in 0..rows {
            for x in 0..cols {
                if across.get(y * cols + x).copied().unwrap_or(false) {
                    for yy in y.saturating_sub(ky)..=(y + ky).min(rows - 1) {
                        if let Some(v) = on.get_mut(yy * cols + x) {
                            *v = true;
                        }
                    }
                }
            }
        }
        Tiles { size: self.size, cols, on }
    }

    pub(crate) fn union(&mut self, other: &Tiles) {
        for (a, b) in self.on.iter_mut().zip(&other.on) {
            *a |= *b;
        }
    }

    /// The tiles as few rectangles (in pixels, inside the output): each row's runs, a run that continues one of the row
    /// above with the same columns joining it.
    pub(crate) fn rects(&self) -> Vec<Rect> {
        let (w, h) = self.size;
        let mut done: Vec<Rect> = Vec::new();
        let mut open: Vec<Rect> = Vec::new();
        for ty in 0..self.rows() {
            let mut runs = Vec::new();
            let mut tx = 0;
            while tx < self.cols {
                if self.on.get(ty * self.cols + tx).copied().unwrap_or(false) {
                    let start = tx;
                    while tx < self.cols && self.on.get(ty * self.cols + tx).copied().unwrap_or(false) {
                        tx += 1;
                    }
                    runs.push([start * TILE, ty * TILE, (tx * TILE).min(w), ((ty + 1) * TILE).min(h)]);
                } else {
                    tx += 1;
                }
            }
            let mut next = Vec::new();
            for r in runs {
                match open.iter().position(|o| o[0] == r[0] && o[2] == r[2]) {
                    Some(i) => {
                        let mut o = open.swap_remove(i);
                        o[3] = r[3];
                        next.push(o);
                    }
                    None => next.push(r),
                }
            }
            done.append(&mut open);
            open = next;
        }
        done.append(&mut open);
        done
    }
}

/// Everything a render depends on but the spots: when it is the same, a render differs from the last one only where
/// its spots drew differently.
#[derive(Debug, PartialEq)]
pub(crate) struct Rest {
    settings: DevelopSettings,
    /// The source info without its large tables, which [`Self::tables`] names by address.
    info: SourceInfo,
    tables: [usize; 3],
    plan: String,
}

impl Rest {
    pub(crate) fn of(info: &SourceInfo, plan: &Plan<'_>, req: &RenderRequest) -> Rest {
        let mut settings = (*plan.settings).clone();
        settings.spots.clear();
        let mut light = info.clone();
        light.mattes = None;
        light.local_tone = None;
        let tables = [
            info.mattes.as_ref().map_or(0, |m| Arc::as_ptr(m) as usize),
            info.local_tone.as_ref().map_or(0, |m| Arc::as_ptr(m) as usize),
            plan.mattes.as_ref().map_or(0, |m| Arc::as_ptr(m) as usize),
        ];
        let p = plan;
        let plan = format!(
            "{req:?} {:?} {:?} {:?} {:?} {} {} {} {:?} {} {} {} {}",
            p.eyes,
            p.recolor,
            p.fixed_air.map(f32::to_bits),
            p.keep,
            p.w,
            p.h,
            p.geo,
            p.px_per_long.to_bits(),
            p.detail_scale.to_bits(),
            p.src_long,
            p.native_long,
            p.draft
        );
        Rest { settings, info: light, tables, plan }
    }
}

/// What a view keeps of its last finished render at one size, for the next one to start from.
#[derive(Clone)]
pub(crate) struct Last {
    pub(crate) rest: Arc<Rest>,
    pub(crate) lin_key: u64,
    /// The finished output (packed RGBA8) and its number ([`FrameNote`]).
    pub(crate) out: Arc<Buf>,
    pub(crate) frame: u64,
    pub(crate) air: u32,
}

/// Why a render does or does not start from the view's last one (`LIGHTCRAFT_PROFILE`).
pub(crate) fn why(rest: Option<&Rest>, last: Option<&Last>) -> &'static str {
    match (rest, last) {
        (None, _) => "not eligible",
        (_, None) => "no last render",
        (Some(r), Some(l)) if *r != *l.rest => "more than the spots changed",
        _ => "from the last render",
    }
}

/// What `spots_keep` redrew: the image it started from and the boxes it rewrote.
pub(crate) struct Redone {
    pub(crate) from: Weak<Buf>,
    pub(crate) rects: Vec<Rect>,
}

/// Whether a render may start from the view's last one: no fault asked for, nothing on the CPU before the planes
/// (LaMa spots, the camera's local tone map), no red eye, and no visible mask that reads the image.
pub(crate) fn eligible(info: &SourceInfo, plan: &Plan<'_>) -> bool {
    let s = &*plan.settings;
    !lightcraft_pipeline::local_tone::enabled(info, s)
        && !s.spots.iter().any(lightcraft_pipeline::spots_learned::is_learned)
        && plan.eyes.is_empty()
        && !s.masks.iter().filter(|m| m.visible && !m.components.is_empty()).any(|m| lightcraft_pipeline::masks::reads(m).0)
}

/// The kept linear image `old` with the tiles `spots` (where the image after the spots changed) and as far as noise
/// reduction spreads them made again from `post` (the image after the spots, whole); with the tiles that changed.
pub(crate) fn linear(
    cx: &mut Cx<'_>,
    post: &Buf,
    old: &Buf,
    plan: &Plan<'_>,
    spots: &Tiles,
    noise: Option<&lightcraft_pipeline::nrc_joint::JointNoise>,
) -> Option<(Buf, Tiles)> {
    let whole = (plan.w, plan.h);
    let t = spots.grow(cut::denoise_reach(plan));
    let out = cx.copy(old);
    let part = Part::whole(post, whole, 3);
    for r in t.rects() {
        let v = cut::denoise(cx, &part, plan, noise, &r)?;
        paste(cx, &v, &r, 3, &out, plan.w);
    }
    Some((out, t))
}

/// A copy of the plane `old` (`nc` channels) with the tiles of `t` made again by `f` (one rectangle at a time).
fn update(cx: &mut Cx<'_>, old: &Buf, t: &Tiles, nc: usize, w: usize, mut f: impl FnMut(&mut Cx<'_>, &Rect) -> Option<Buf>) -> Option<Arc<Buf>> {
    let out = cx.copy(old);
    for r in t.rects() {
        let v = f(cx, &r)?;
        paste(cx, &v, &r, nc, &out, w);
    }
    Some(Arc::new(out))
}

/// The planes of the linear image `lin`, from `old` (the planes of the image it was made from, which differed only in
/// the tiles `t_lin`): each plane made again where it changed. With the planes' tiles that changed (all of them when
/// dehaze's airlight moved). `None` when `old` lacks a plane this render needs: render the planes whole.
pub(crate) fn planes(cx: &mut Cx<'_>, lin: &Buf, plan: &Plan<'_>, req: &RenderRequest, old: &Planes, t_lin: &Tiles) -> Option<(Planes, Prep, Tiles)> {
    let (w, h) = (plan.w, plan.h);
    let whole = (w, h);
    let sig = local::plane_sigmas(&plan.settings, plan.px_per_long, plan.detail_scale, req.quality);
    let lin_p = Part::whole(lin, whole, 3);
    let log_l = update(cx, old.log_l.as_ref()?, t_lin, 1, w, |cx, r| cut::pointwise(cx, "log_lum_k", &lin_p, 1, r))?;
    let lp = Part::whole(&log_l, whole, 1);
    let mut t_out = t_lin.clone();
    let mut planes = Planes { key: plan.lin_key, log_l: Some(log_l.clone()), ..Default::default() };
    // a plane kept under `key`, made again where `reach` takes the change
    let mut redo = |cx: &mut Cx<'_>, kept: Option<&Arc<Buf>>, reach: Reach, nc: usize, f: &mut dyn FnMut(&mut Cx<'_>, &Rect) -> Option<Buf>| {
        let t = t_lin.grow(reach);
        let b = update(cx, kept?, &t, nc, w, f)?;
        t_out.union(&t);
        Some(b)
    };
    let base = match sig.base {
        Some(sg) => {
            let key = local::base_key(&plan.settings, sg, sig.base_detail);
            let kept = old.base.as_ref().filter(|(k, _)| *k == key).map(|(_, b)| b);
            let reach = cut::tone_base_reach(whole, sg, sig.base_detail);
            let b = redo(cx, kept, reach, 1, &mut |cx, r| cut::tone_base(cx, &lp, whole, sg, sig.base_detail, r))?;
            planes.base = Some((key, b.clone()));
            b
        }
        None => log_l.clone(),
    };
    let same = |slot: &Option<(u32, Arc<Buf>)>, sg: f32| slot.as_ref().filter(|(k, _)| *k == sg.to_bits()).map(|(_, b)| b.clone());
    let mut plane = |cx: &mut Cx<'_>,
                     slot: &Option<(u32, Arc<Buf>)>,
                     sg: Option<f32>,
                     reach: &dyn Fn(f32) -> Reach,
                     f: &dyn Fn(&mut Cx<'_>, f32, &Rect) -> Option<Buf>| {
        match sg {
            None => Some(None),
            Some(sg) => {
                let b = redo(cx, same(slot, sg).as_ref(), reach(sg), 1, &mut |cx, r| f(cx, sg, r))?;
                Some(Some((sg.to_bits(), b)))
            }
        }
    };
    planes.clarity = plane(cx, &old.clarity, sig.clarity, &|sg| cut::fast_reach(whole, sg), &|cx, sg, r| {
        cut::guided_fast(cx, &lp, whole, sg, local::CLARITY_EPS, r)
    })?;
    let blur = |cx: &mut Cx<'_>, sg: f32, r: &Rect| cut::gaussian(cx, &lp, whole, sg, r);
    planes.texture = plane(cx, &old.texture, sig.texture, &|sg| cut::gauss_reach(sg, whole), &blur)?;
    planes.sharpen = plane(cx, &old.sharpen, sig.sharpen, &|sg| cut::gauss_reach(sg, whole), &blur)?;
    // dehaze: the dark channel of the image, blurred, and its airlight from the whole plane
    let (dark, air) = match sig.dark {
        None => (None, 1.0),
        Some(sg) => {
            let kept = old.dark.as_ref().filter(|(k, ..)| *k == sg.to_bits()).map(|(_, b, _)| b);
            let reach = cut::gauss_reach(sg, whole);
            let d = redo(cx, kept, reach, 1, &mut |cx, r| {
                let i = cut::grow(r, reach, whole);
                let d0 = cut::pointwise(cx, "dark_k", &lin_p, 1, &i)?;
                cut::gaussian(cx, &Part { buf: &d0, at: i, nc: 1 }, whole, sg, r)
            })?;
            let n = w * h;
            let m = n.div_ceil(local::AIRLIGHT_STEP);
            let sub = cx.gpu.buffer(m);
            map(cx, "subsample", m, &[local::AIRLIGHT_STEP as u32], [Some(&d), None, None], &sub);
            let air = local::airlight_of(cx.read(&sub, m));
            cx.flush();
            planes.dark = Some((sg.to_bits(), d.clone(), air));
            (Some(d), air)
        }
    };
    let chroma = match sig.chroma {
        None => None,
        Some(sg) => {
            let reach = cut::gauss_reach(sg, whole);
            let b = redo(cx, same(&old.chroma, sg).as_ref(), reach, 3, &mut |cx, r| {
                let i = cut::grow(r, reach, whole);
                let c = cut::pointwise(cx, "chroma_k", &lin_p, 3, &i)?;
                cut::gaussian(cx, &Part { buf: &c, at: i, nc: 3 }, whole, sg, r)
            })?;
            planes.chroma = Some((sg.to_bits(), b.clone()));
            Some(b)
        }
    };
    let prep = Prep {
        log_l,
        base,
        clarity: planes.clarity.as_ref().map(|p| p.1.clone()),
        texture: planes.texture.as_ref().map(|p| p.1.clone()),
        sharpen: planes.sharpen.as_ref().map(|p| p.1.clone()),
        dark,
        chroma,
        air,
    };
    Some((planes, prep, t_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_grow_by_their_reach_and_cover_as_rectangles() {
        let size = (1000, 600);
        let t = Tiles::of(size, &[[130, 70, 140, 75]]);
        assert_eq!(t.count(), 1);
        assert_eq!(t.rects(), vec![[128, 64, 192, 128]]);
        let g = t.grow((64, 1));
        assert_eq!(g.count(), 9, "a tile each way");
        assert_eq!(g.rects(), vec![[64, 0, 256, 192]]);
        // at the edge the last tile is cut to the image
        let e = Tiles::of(size, &[[990, 590, 1000, 600]]).grow((200, 0));
        assert_eq!(e.rects(), vec![[704, 576, 1000, 600]]);
        // two areas apart stay apart
        let mut two = Tiles::of(size, &[[0, 0, 10, 10]]);
        two.union(&Tiles::of(size, &[[900, 500, 910, 510]]));
        assert_eq!(two.rects().len(), 2);
        assert_eq!(Tiles::all(size).rects(), vec![[0, 0, 1000, 600]]);
        assert!(Tiles::none(size).rects().is_empty());
    }

    #[test]
    fn rectangles_cover_exactly_the_tiles() {
        let size = (640, 448);
        let mut t = Tiles::of(size, &[[0, 0, 200, 70], [150, 60, 400, 300], [600, 400, 640, 448]]);
        t = t.grow((10, 70));
        let mut covered = Tiles::none(size);
        let mut area = 0;
        for r in t.rects() {
            area += cut::area(&r);
            covered.union(&Tiles::of(size, &[r]));
        }
        assert_eq!(covered, t);
        let tiles: usize = t.rects().iter().map(cut::area).sum();
        assert_eq!(area, tiles, "no rectangle overlaps another");
    }
}
