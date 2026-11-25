//! Spots drawn again only where an edit reaches (issue #82).
//!
//! Spots are drawn one after another on the resampled source, before the white balance (issue #184: Temperature and
//! Tint then draw none of them again; [`spots_after_wb`] restores the old order, after it), each reading the image as
//! the earlier ones left it, so every spot edit used to draw every spot again: adding the 101st spot cost a hundred times the first, and
//! moving one redrew all of them. A view now keeps the image after its spots ([`Kept::post`]) and, for each spot, its
//! box as it was before the spot drew and after ([`Patch`]). On the next render [`redo`] compares the old spots with
//! the new ones. Only the spots added or changed are drawn, and the later spots whose box or source reads pixels a
//! drawn spot changed; earlier spots are never drawn again. Before that, the later spots that overlap what changes
//! are taken off in reverse order (each spot's box goes back to how it was before it), and afterwards every one of
//! them not drawn again is put back from its own result. Each spot drawn then reads exactly the pixels the whole
//! sequence would have given it, so the result is the same as drawing every spot. A change before the spots (white
//! balance, profile, anything in [`key`]) draws them all again; a change after them (tone, masks, effects) reuses
//! the whole stage and draws none.

use std::cell::Cell;
use std::sync::Arc;

use lightcraft_develop::Spot;
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::{Plan, SourceInfo, spots};

thread_local! {
    static DRAWN: Cell<usize> = const { Cell::new(0) };
    static AFTER_WB_HERE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Draw the Classic spots after the white balance, defringe and the camera's local tone mapping, as before issue #184
/// (Settings ▸ Performance ▸ Draw Spots After White Balance, `app.spotOrder`). Off: they are drawn on the resampled source,
/// with the LaMa spots, so Temperature and Tint draw none again. Process-wide, like the hot pixel limit.
static AFTER_WB: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether renders planned now draw the Classic spots after the white balance ([`set_spots_after_wb`]); a plan keeps
/// the answer ([`Plan::spots_after_wb`]), so one render never mixes the orders.
pub fn spots_after_wb() -> bool {
    AFTER_WB_HERE.with(Cell::get).unwrap_or_else(|| AFTER_WB.load(std::sync::atomic::Ordering::Relaxed))
}

/// Set [`spots_after_wb`]; true when it changed.
pub fn set_spots_after_wb(on: bool) -> bool {
    AFTER_WB.swap(on, std::sync::atomic::Ordering::Relaxed) != on
}

/// Test hook: renders planned on this thread take `on` as [`spots_after_wb`] (`None`: the process's), so a test of one
/// order does not change another test's renders.
pub fn spots_after_wb_here(on: Option<bool>) {
    AFTER_WB_HERE.with(|c| c.set(on));
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
            // (a hidden spot draws nothing: `DevelopSettings::effective` already leaves it out of a render)
            if done.get(i).copied().unwrap_or(false) || sp.hidden {
                return None;
            }
            footprint(sp, frame, w, h, ppl).map(|at| Drawn { spot: sp.clone(), at })
        })
        .collect()
}

/// One step of turning the old spots' result into the new one's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Draw new spot `i`.
    Draw(usize),
    /// Put new spot `i` back from the result old spot `old` gave (the same spot, reading the same pixels).
    Paste { i: usize, old: usize },
}

/// The work that turns the result of `old` into that of `new` (same image before the spots): put the boxes of the
/// old spots in `undo` back as they were before each drew (in that order: last first), then run `steps`.
#[derive(Clone, Debug, PartialEq)]
pub struct Redo {
    pub undo: Vec<usize>,
    pub steps: Vec<Step>,
    /// For each new spot, the old spot it is (unchanged), if any.
    pub same: Vec<Option<usize>>,
}

impl Redo {
    /// Spots it draws.
    pub fn drawn(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s, Step::Draw(_))).count()
    }
}

/// [`Redo`] from the result of `old` to that of `new` (both drawn on the same image).
pub fn redo(old: &[Option<Drawn>], new: &[Option<Drawn>]) -> Redo {
    redo_under(old, new, &[])
}

/// [`Redo`] from the result of `old` to that of `new`, drawn on an image that changed in the boxes `under` since `old`
/// was drawn (issue #372: a LaMa spot added under the Classic ones). The old spots that wrote there are taken off, the
/// caller puts the new image back in `under` after [`Redo::undo`], and every spot that reads it is drawn again.
pub fn redo_under(old: &[Option<Drawn>], new: &[Option<Drawn>], under: &[Rect]) -> Redo {
    // the spots changed: what lies between the longest common start and end
    let a = old.iter().zip(new).take_while(|(o, n)| o == n).count();
    let (old_rest, new_rest) = (old.get(a..).unwrap_or_default(), new.get(a..).unwrap_or_default());
    let b = old_rest.iter().rev().zip(new_rest.iter().rev()).take_while(|(o, n)| o == n).count();
    let (old_end, new_end) = (old.len().saturating_sub(b), new.len().saturating_sub(b));
    let write = |v: &[Option<Drawn>], i: usize| v.get(i).and_then(Option::as_ref).map(|d| d.at.write);
    // the old spot a new one is (before the change, the same index; after it, the same distance from the end)
    let old_of = |i: usize| if i < a { Some(i) } else { (i >= new_end).then(|| i + old_end - new_end) };
    // with nothing changed under the spots, none before the first change is drawn or taken off (`from`)
    let from = if under.is_empty() { a } else { 0 };
    // the spots drawn: the new and changed ones, and each later one that reads what one drawn before it changed (or
    // what changed under them)
    let mut changed: Vec<Rect> = under.iter().copied().chain((a..old_end).filter_map(|i| write(old, i))).collect();
    let mut read_by_drawn: Vec<Rect> = Vec::new();
    let mut draw = vec![false; new.len()];
    for (i, d) in new.iter().enumerate().skip(from) {
        let Some(d) = d else { continue };
        if (a..new_end).contains(&i) || changed.iter().any(|r| overlaps(r, &d.at.read)) {
            if let Some(slot) = draw.get_mut(i) {
                *slot = true;
            }
            changed.push(d.at.write);
            read_by_drawn.push(d.at.read);
        }
    }
    // the old spots taken off: those drawn again or removed, those after the first change whose box holds what a drawn
    // spot reads or what a removed one drew, those whose box holds what changed under them, and every later one drawn
    // over one taken off
    let new_of = |k: usize| if k < a { Some(k) } else { (k >= old_end).then(|| k + new_end - old_end) };
    let mut undone = vec![false; old.len()];
    for (k, d) in old.iter().enumerate().skip(from) {
        let Some(d) = d else { continue };
        let removed = (a..old_end).contains(&k);
        let redrawn = new_of(k).is_some_and(|i| draw.get(i).copied().unwrap_or(false));
        let needed = k >= a && read_by_drawn.iter().chain(&changed).any(|r| overlaps(r, &d.at.write));
        let beneath = under.iter().any(|r| overlaps(r, &d.at.write));
        let over = (from..k).any(|u| undone.get(u).copied().unwrap_or(false) && write(old, u).is_some_and(|w| overlaps(&w, &d.at.write)));
        if let Some(slot) = undone.get_mut(k) {
            *slot = removed || redrawn || needed || beneath || over;
        }
    }
    let undo = (from..old.len()).rev().filter(|k| undone.get(*k).copied().unwrap_or(false)).collect();
    let steps = (from..new.len())
        .filter_map(|i| {
            if draw.get(i).copied().unwrap_or(false) {
                Some(Step::Draw(i))
            } else {
                old_of(i).filter(|o| undone.get(*o).copied().unwrap_or(false)).map(|old| Step::Paste { i, old })
            }
        })
        .collect();
    let same = (0..new.len()).map(old_of).collect();
    Redo { undo, steps, same }
}

/// One spot's box before it drew and after, row by row (`I`: the pixels, on the host or the device).
pub struct Patch<P> {
    pub before: P,
    pub after: P,
}

/// What a view keeps of its last render's spots: the image after them, how each was drawn and each one's [`Patch`].
pub struct Kept<I, P> {
    /// [`key`] of the image before the spots.
    pub key: u64,
    pub post: Arc<I>,
    pub drawn: Vec<Option<Drawn>>,
    pub patches: Vec<Option<Arc<Patch<P>>>>,
    /// The spots a learned fill drew (part of the image before the spots).
    pub done: Vec<bool>,
}

impl<I, P> Kept<I, P> {
    /// The patches of the spots `redo` puts back: all there (else the whole stage is drawn again).
    pub fn has_patches(&self, r: &Redo) -> bool {
        let old = r.steps.iter().filter_map(|s| match s {
            Step::Paste { old, .. } => Some(*old),
            Step::Draw(_) => None,
        });
        r.undo.iter().copied().chain(old).all(|k| self.patches.get(k).is_some_and(Option::is_some))
    }
}

/// Most output sizes a view keeps spots for: the full-size render and the two draft sizes a drag goes through, so
/// the full render after a drag still finds its spots.
pub const KEPT_SIZES: usize = 3;

/// Most bytes a view's kept spots take (its image after the spots and each spot's boxes, every size together). Past it
/// the oldest size is forgotten, and a size that alone needs more is not kept: those renders draw every spot again.
pub const KEPT_BYTES: usize = 384 << 20;

type KeptEntry<T> = (std::sync::Weak<Rgb32f>, u64, Arc<T>, usize);

/// A view's kept spots ([`Kept`], [`CpuKept`]) per output size, of the photo source they were drawn from: the last
/// [`KEPT_SIZES`], in at most [`KEPT_BYTES`]. Apart from the view's stage cache, which holds two sizes: a drag's drafts
/// would push the full size out, and the full render after every drag drew every spot again.
pub struct KeptStore<T> {
    v: std::sync::Mutex<Vec<KeptEntry<T>>>,
    cap: usize,
}

impl<T> Default for KeptStore<T> {
    fn default() -> Self {
        Self::with_cap(KEPT_BYTES)
    }
}

impl<T> KeptStore<T> {
    /// A store that keeps at most `cap` bytes.
    pub fn with_cap(cap: usize) -> Self {
        Self { v: std::sync::Mutex::new(Vec::new()), cap }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<KeptEntry<T>>> {
        self.v.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What was kept for `src` at the size `geo` names ([`Plan::geo`]).
    pub fn get(&self, src: &Arc<Rgb32f>, geo: u64) -> Option<Arc<T>> {
        self.lock().iter().find(|(s, g, ..)| *g == geo && std::sync::Weak::ptr_eq(s, &Arc::downgrade(src))).map(|e| e.2.clone())
    }

    /// Keep `kept` (`bytes` large) for `src` at `geo` (`None`: forget it), dropping anything of a source that is gone and
    /// the oldest sizes past [`KEPT_SIZES`] or the byte cap. One that alone is over the cap is not kept.
    pub fn put(&self, src: &Arc<Rgb32f>, geo: u64, kept: Option<Arc<T>>, bytes: usize) {
        let mut v = self.lock();
        v.retain(|(s, g, ..)| s.strong_count() > 0 && !(*g == geo && std::sync::Weak::ptr_eq(s, &Arc::downgrade(src))));
        if let Some(k) = kept.filter(|_| bytes <= self.cap) {
            v.push((Arc::downgrade(src), geo, k, bytes));
        }
        while v.len() > KEPT_SIZES || v.iter().map(|e| e.3).sum::<usize>() > self.cap {
            v.remove(0);
        }
    }

    /// Forget everything.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.lock().iter().map(|e| e.3).sum()
    }
}

/// Which section toggles (eyes) the image before the spots depends on, beyond what [`DevelopSettings::effective`]
/// already resets (Light, Color and Detail come back to their defaults when off, so their values say it): Optics switches
/// defringe (and with Geometry the frame, part of [`Plan::geo`]) off where it is applied.
///
/// [`DevelopSettings::effective`]: lightcraft_develop::DevelopSettings::effective
pub fn pre_spot_sections(s: &lightcraft_develop::DevelopSettings) -> [bool; 2] {
    ["optics", "geometry"].map(|section| s.section_enabled(section))
}

/// Key of the image the Classic spots are drawn on, apart from the LaMa spots drawn on it first (the boxes they changed
/// are handed to the Classic layer instead, issue #372): the resampled source, and when the Classic spots are drawn
/// after the white balance ([`Plan::spots_after_wb`]), everything [`Plan::pre_nr_key`] holds before them too (white
/// balance, defringe, the local tone mapping). Red eye is drawn after them either way.
pub fn key(info: &SourceInfo, p: &Plan<'_>) -> u64 {
    let s = &*p.settings;
    crate::hash_of((learned_key(p), before_spots(info, s, p.spots_after_wb), format!("{:?}", s.optics), pre_spot_sections(s)))
}

/// What the image the Classic spots read holds beyond the resampled source: in the old order ([`spots_after_wb`]), the
/// white balance and whether the local tone mapping ran (`None` in the new one, where neither has run yet).
pub(crate) fn before_spots(info: &SourceInfo, s: &lightcraft_develop::DevelopSettings, after_wb: bool) -> Option<([u64; 4], bool)> {
    after_wb.then(|| {
        let (wb_t, wb_tint) = crate::local::effective_wb(info, s);
        ([wb_t, wb_tint, info.as_shot_temp, info.as_shot_tint].map(f64::to_bits), crate::local_tone::enabled(info, s))
    })
}

/// Key of the image the LaMa spots are drawn on (the resampled source) and of the size their fill sees it at.
fn learned_key(p: &Plan<'_>) -> u64 {
    crate::hash_of((p.geo, (p.w, p.h), p.px_per_long.to_bits(), p.src_long, p.native_long))
}

/// The pixels of `img` in `r`, row by row.
pub fn cut(img: &Rgb32f, r: &Rect) -> Vec<[f32; 3]> {
    let (x0, x1) = (r[0].min(img.width), r[2].min(img.width));
    let mut out = Vec::with_capacity((x1 - x0) * r[3].saturating_sub(r[1]));
    for y in r[1]..r[3].min(img.height) {
        out.extend_from_slice(img.data.get(y * img.width + x0..y * img.width + x1).unwrap_or_default());
    }
    out
}

/// `patch` ([`cut`] from `r`) put back into `img`.
pub fn paste(img: &mut Rgb32f, r: &Rect, patch: &[[f32; 3]]) {
    let (x0, x1) = (r[0].min(img.width), r[2].min(img.width));
    let w = img.width;
    for (row, y) in (r[1]..r[3].min(img.height)).enumerate() {
        let (d, s) = (img.data.get_mut(y * w + x0..y * w + x1), patch.get(row * (x1 - x0)..(row + 1) * (x1 - x0)));
        if let (Some(d), Some(s)) = (d, s) {
            d.copy_from_slice(s);
        }
    }
}

type CpuPatch = Option<Arc<Patch<Vec<[f32; 3]>>>>;
type CpuLayer = Kept<Rgb32f, Vec<[f32; 3]>>;

/// What the CPU renderer keeps of a view's spots: the LaMa spots (drawn on the resampled source, before the white
/// balance) and the Classic ones (after them, before or after the white balance: [`spots_after_wb`]), each a layer of
/// its own.
pub struct CpuKept {
    pub learned: Option<CpuLayer>,
    pub classic: CpuLayer,
}

impl CpuKept {
    /// Bytes of the images and patches held.
    pub fn bytes(&self) -> usize {
        let layer = |k: &CpuLayer| (k.post.data.len() + k.patches.iter().flatten().map(|p| p.before.len() + p.after.len()).sum::<usize>()) * 12;
        self.learned.as_ref().map_or(0, layer) + layer(&self.classic)
    }
}

/// One layer of spots drawn in order on `img`, reusing `kept` (the layer's last result, `key` naming the image it was
/// drawn on) for only the spots an edit reaches. `under`: the boxes where `img` differs from the image `kept` was drawn
/// on (`None`: not known, every spot is drawn). Otherwise `prepare` makes `img` the layer's image and every spot is
/// drawn. `draw` draws spot `i` and says whether it drew (a spot that did not is tried again next time). `img` ends as
/// the layer's result. Also says which boxes of the result changed from `kept`'s (`None`: it was drawn whole).
fn layer(
    img: &mut Rgb32f,
    (key, under): (u64, Option<&[Rect]>),
    kept: Option<&CpuLayer>,
    mut new: Vec<Option<Drawn>>,
    prepare: impl FnOnce(&mut Rgb32f),
    mut draw: impl FnMut(&mut Rgb32f, usize) -> bool,
) -> (CpuLayer, Option<Vec<Rect>>) {
    let mut patches: Vec<CpuPatch> = vec![None; new.len()];
    let mut draw_one = |img: &mut Rgb32f, i: usize, new: &mut [Option<Drawn>], patches: &mut [CpuPatch]| {
        let Some(r) = new.get(i).and_then(Option::as_ref).map(|d| d.at.write) else { return };
        let before = cut(img, &r);
        if draw(img, i) {
            if let Some(slot) = patches.get_mut(i) {
                *slot = Some(Arc::new(Patch { before, after: cut(img, &r) }));
            }
        } else if let Some(slot) = new.get_mut(i) {
            *slot = None;
        }
    };
    let hit = kept.filter(|k| k.key == key && (k.post.width, k.post.height) == (img.width, img.height));
    let plan = hit.zip(under).map(|(k, u)| (k, redo_under(&k.drawn, &new, u), u)).filter(|(k, r, _)| k.has_patches(r));
    if crate::profiling() {
        let why = plan.as_ref().map_or_else(|| "drew all".to_string(), |(_, r, _)| format!("drew {} (took off {})", r.drawn(), r.undo.len()));
        eprintln!("  spots: {} of {} in {}×{}: {why}", new.iter().flatten().count(), new.len(), img.width, img.height);
    }
    let rewrote = match plan {
        Some((k, r, under)) => {
            // (the image under the spots where it changed, put back once the spots drawn there are taken off)
            let fresh: Vec<(Rect, Vec<[f32; 3]>)> = under.iter().map(|u| (*u, cut(img, u))).collect();
            *img = (*k.post).clone();
            // (a spot not drawn again keeps its patches: it is the old one, reading the same pixels)
            for (slot, old) in patches.iter_mut().zip(&r.same) {
                *slot = old.and_then(|o| k.patches.get(o).cloned().flatten());
            }
            let mut rects: Vec<Rect> = under.to_vec();
            for u in &r.undo {
                if let (Some(Some(d)), Some(Some(pt))) = (k.drawn.get(*u), k.patches.get(*u)) {
                    paste(img, &d.at.write, &pt.before);
                    rects.push(d.at.write);
                }
            }
            for (u, px) in &fresh {
                paste(img, u, px);
            }
            for step in &r.steps {
                let (Step::Draw(i) | Step::Paste { i, .. }) = *step;
                // (a spot that did not draw left its box as it was)
                let at = new.get(i).and_then(Option::as_ref).map(|d| d.at.write);
                match *step {
                    Step::Draw(i) => draw_one(img, i, &mut new, &mut patches),
                    Step::Paste { i, old } => {
                        if let (Some(Some(d)), Some(Some(pt))) = (new.get(i), k.patches.get(old)) {
                            paste(img, &d.at.write, &pt.after);
                        }
                    }
                }
                if let Some(r) = at.filter(|_| new.get(i).is_some_and(Option::is_some)) {
                    rects.push(r);
                }
            }
            Some(rects)
        }
        None => {
            prepare(img);
            for i in 0..new.len() {
                draw_one(img, i, &mut new, &mut patches);
            }
            None
        }
    };
    (Kept { key, post: Arc::new(img.clone()), drawn: new, patches, done: Vec::new() }, rewrote)
}

/// [`crate::lin_cpu`] on `img` (the resampled source), reusing `kept` (the view's last spots): only the spots an edit
/// reaches are drawn, LaMa and Classic alike, and the white balance runs only when the image the Classic spots are
/// drawn on changed. Returns what to keep for the next render (`None` without spots).
pub fn lin_cpu(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>, kept: Option<&CpuKept>) -> Option<Arc<CpuKept>> {
    let kept = lin_cpu_pre_retouch(img, info, p, kept);
    // (`crate::lin_cpu`'s order: portrait retouching after red eye, fork `fork/retouch-foundation.md`)
    crate::retouch_step::apply(img, info, p);
    kept
}

/// [`lin_cpu`] up to the portrait retouching ([`crate::lin_cpu_pre_retouch`]).
pub fn lin_cpu_pre_retouch(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>, kept: Option<&CpuKept>) -> Option<Arc<CpuKept>> {
    let s = &*p.settings;
    if s.spots.is_empty() {
        crate::lin_cpu_pre_retouch(img, info, p);
        return None;
    }
    crate::retouch_step::note_pre_retouch_run();
    // a window whose spots are all finished at this scale pastes them (issue #176, `spots_scale`)
    if let (Some(f), Some(origin)) = (&p.spots_at.paste, p.spots_at.origin) {
        if p.spots_after_wb {
            before_classic(img, info, p);
        }
        crate::spots_scale::paste(img, f, origin);
        crate::spots_scale::note_pasted();
        if !p.spots_after_wb {
            before_classic(img, info, p);
        }
        crate::redeye::apply(img, &p.eyes);
        return None;
    }
    let (w, h, ppl) = (img.width, img.height, p.px_per_long);
    // 1. LaMa spots, on the resampled source: each asks its fill (and the fill's cache) only when it is drawn again
    let lnew: Vec<Option<Drawn>> = s
        .spots
        .iter()
        .map(|sp| {
            crate::spots_learned::footprint(sp, &p.frame, w, h, ppl).map(|(write, read)| Drawn { spot: sp.clone(), at: Footprint { write, read } })
        })
        .collect();
    let kept_learned = kept.and_then(|k| k.learned.as_ref());
    let boxes = |k: &CpuLayer| k.drawn.iter().flatten().map(|d| d.at.write).collect::<Vec<Rect>>();
    let learned = lnew.iter().any(Option::is_some).then(|| {
        let provider = crate::learned_fill::current();
        let at = crate::spots_learned::Where { frame: &p.frame, ppl, reference_long: crate::native_long(info, p.src_long) as f64, draft: p.draft };
        let run = crate::learned_fill::FillRun::new();
        layer(
            img,
            // (the image under the LaMa spots is the resampled source, which the key names)
            (learned_key(p), Some(&[])),
            kept_learned,
            lnew,
            |_| {},
            |img, i| {
                let (Some(pr), Some(sp)) = (provider.as_ref(), s.spots.get(i)) else { return false };
                // (the view's fills, for every scale and window: issue #176)
                let base = p.spots_at.fill_base;
                let kept = p.spots_at.store.as_deref().filter(|_| base != 0).map(|st| (st, crate::spots_scale::fill_key(base, &s.spots, i)));
                crate::spots_learned::apply_one_kept(img, sp, &at, &**pr, &run, kept)
            },
        )
    });
    // what changed under the Classic spots since the kept ones were drawn (issue #372: a LaMa spot added, moved or
    // removed redraws only the Classic spots it reaches): the boxes the LaMa layer redrew; with no LaMa layer before or
    // now, every box the one there is drew; `None` (draw them all) when the LaMa layer was drawn whole over another
    let under = match (&learned, kept_learned) {
        (Some((_, Some(rewrote))), _) => Some(rewrote.clone()),
        (Some((k, None)), None) => Some(boxes(k)),
        (Some((_, None)), Some(_)) => None,
        (None, old) => Some(old.map(boxes).unwrap_or_default()),
    };
    // (drawn after the white balance and defringe, what changed under them reaches further: drawn again whole)
    let under = under.filter(|u| !p.spots_after_wb || u.is_empty());
    let learned = learned.map(|(k, _)| k);
    let done: Vec<bool> = learned.as_ref().map(|k| k.drawn.iter().map(Option::is_some).collect()).unwrap_or_default();
    // 2. the Classic spots (and the LaMa ones that got no fill): on what the LaMa spots left, or after the white balance
    let cnew = entries(&s.spots, &done, &p.frame, w, h, ppl);
    let prepare = |img: &mut Rgb32f| {
        if p.spots_after_wb {
            before_classic(img, info, p);
        }
    };
    let (classic, _) = layer(img, (key(info, p), under.as_deref()), kept.map(|k| &k.classic), cnew, prepare, |img, i| {
        if let Some(sp) = s.spots.get(i) {
            spots::apply(img, std::slice::from_ref(sp), &p.frame, ppl);
        }
        true
    });
    // (a window keeps the spots it drew exactly, for the next window of its scale: a LaMa spot counts once its fill came;
    // a draft keeps none while one has no fill, as a later spot may read its Classic fill, issue #372)
    let unfilled =
        |(i, sp): (usize, &Spot)| crate::spots_learned::footprint(sp, &p.frame, w, h, ppl).is_some() && !done.get(i).copied().unwrap_or(false);
    if p.spots_at.store.is_some() && !(p.draft && s.spots.iter().enumerate().any(unfilled)) {
        let drawn: Vec<bool> =
            s.spots.iter().enumerate().map(|(i, sp)| !crate::spots_learned::is_learned(sp) || done.get(i).copied().unwrap_or(false)).collect();
        crate::spots_scale::capture(&p.spots_at, img, &drawn);
    }
    if !p.spots_after_wb {
        before_classic(img, info, p);
    }
    crate::redeye::apply(img, &p.eyes);
    Some(Arc::new(CpuKept { learned, classic }))
}

/// What runs between the LaMa spots and the red eye besides the Classic spots: the camera's local tone mapping, the
/// white balance and defringe (`crate::lin_cpu`'s order). The Classic spots come before it ([`spots_after_wb`] off) or
/// after.
pub(crate) fn before_classic(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>) {
    let s = &*p.settings;
    crate::local_tone::apply(img, info, p);
    crate::local::white_balance(img, info, s);
    crate::optics::defringe(img, s, p.px_per_long / crate::optics::DEFRINGE_REF_LONG);
}

#[cfg(test)]
#[path = "tests_spots_keep.rs"]
mod tests;
