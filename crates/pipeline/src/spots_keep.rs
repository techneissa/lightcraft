//! Spots drawn again only where an edit reaches (issue #82).
//!
//! Spots are drawn one after another on the white-balanced image, each reading the image as the earlier ones left
//! it, so every spot edit used to draw every spot again: adding the 101st spot cost a hundred times the first, and
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
    // the spots changed: what lies between the longest common start and end
    let a = old.iter().zip(new).take_while(|(o, n)| o == n).count();
    let (old_rest, new_rest) = (old.get(a..).unwrap_or_default(), new.get(a..).unwrap_or_default());
    let b = old_rest.iter().rev().zip(new_rest.iter().rev()).take_while(|(o, n)| o == n).count();
    let (old_end, new_end) = (old.len().saturating_sub(b), new.len().saturating_sub(b));
    let write = |v: &[Option<Drawn>], i: usize| v.get(i).and_then(Option::as_ref).map(|d| d.at.write);
    // the spots drawn: the new and changed ones, and each later one that reads what one drawn before it changed
    let mut changed: Vec<Rect> = (a..old_end).filter_map(|i| write(old, i)).collect();
    let mut read_by_drawn: Vec<Rect> = Vec::new();
    let mut draw = vec![false; new.len()];
    for (i, d) in new.iter().enumerate().skip(a) {
        let Some(d) = d else { continue };
        if i < new_end || changed.iter().any(|r| overlaps(r, &d.at.read)) {
            if let Some(slot) = draw.get_mut(i) {
                *slot = true;
            }
            changed.push(d.at.write);
            read_by_drawn.push(d.at.read);
        }
    }
    // the old spots taken off: those whose box holds what a drawn spot reads or what a removed one drew, and every
    // later one drawn over one taken off
    let old_of = |i: usize| i + old_end - new_end;
    let mut undone = vec![false; old.len()];
    for (k, d) in old.iter().enumerate().skip(a) {
        let Some(d) = d else { continue };
        let removed = k < old_end;
        let redrawn = k >= old_end && draw.get(k + new_end - old_end).copied().unwrap_or(false);
        let needed = read_by_drawn.iter().chain(&changed).any(|r| overlaps(r, &d.at.write));
        let over = (a..k).any(|u| undone.get(u).copied().unwrap_or(false) && write(old, u).is_some_and(|w| overlaps(&w, &d.at.write)));
        if let Some(slot) = undone.get_mut(k) {
            *slot = removed || redrawn || needed || over;
        }
    }
    let undo = (a..old.len()).rev().filter(|k| undone.get(*k).copied().unwrap_or(false)).collect();
    let steps = (a..new.len())
        .filter_map(|i| {
            if draw.get(i).copied().unwrap_or(false) {
                Some(Step::Draw(i))
            } else {
                (i >= new_end && undone.get(old_of(i)).copied().unwrap_or(false)).then(|| Step::Paste { i, old: old_of(i) })
            }
        })
        .collect();
    let same = (0..new.len()).map(|i| if i < a { Some(i) } else { (i >= new_end).then(|| old_of(i)) }).collect();
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

/// A view's kept spots ([`Kept`], [`CpuKept`]) per output size, of the photo source they were drawn from: the last
/// [`KEPT_SIZES`]. Apart from the view's stage cache, which holds two sizes: a drag's drafts would push the full size
/// out, and the full render after every drag drew every spot again.
pub struct KeptStore<T> {
    v: std::sync::Mutex<Vec<(std::sync::Weak<Rgb32f>, u64, Arc<T>)>>,
}

impl<T> Default for KeptStore<T> {
    fn default() -> Self {
        Self { v: std::sync::Mutex::new(Vec::new()) }
    }
}

impl<T> KeptStore<T> {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(std::sync::Weak<Rgb32f>, u64, Arc<T>)>> {
        self.v.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What was kept for `src` at the size `geo` names ([`Plan::geo`]).
    pub fn get(&self, src: &Arc<Rgb32f>, geo: u64) -> Option<Arc<T>> {
        self.lock().iter().find(|(s, g, _)| *g == geo && std::sync::Weak::ptr_eq(s, &Arc::downgrade(src))).map(|e| e.2.clone())
    }

    /// Keep `kept` for `src` at `geo` (`None`: forget it), dropping the oldest size past [`KEPT_SIZES`] and anything of
    /// a source that is gone.
    pub fn put(&self, src: &Arc<Rgb32f>, geo: u64, kept: Option<Arc<T>>) {
        let mut v = self.lock();
        v.retain(|(s, g, _)| s.strong_count() > 0 && !(*g == geo && std::sync::Weak::ptr_eq(s, &Arc::downgrade(src))));
        if let Some(k) = kept {
            v.push((Arc::downgrade(src), geo, k));
        }
        while v.len() > KEPT_SIZES {
            v.remove(0);
        }
    }

    /// Forget everything.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Each kept state, for counting its bytes.
    pub fn each(&self, mut f: impl FnMut(&T)) {
        self.lock().iter().for_each(|e| f(&e.2));
    }
}

/// Key of the image the Classic spots are drawn on: everything [`Plan::pre_nr_key`] holds but those spots and the
/// red eye corrections (drawn after them). The LaMa spots are drawn before the white balance and so are part of it,
/// with which of them got their fill (`done`).
pub fn key(info: &SourceInfo, p: &Plan<'_>, done: &[bool]) -> u64 {
    let s = &*p.settings;
    let (wb_t, wb_tint) = crate::local::effective_wb(info, s);
    let learned: Vec<(usize, String)> =
        s.spots.iter().enumerate().filter(|(_, sp)| crate::spots_learned::is_learned(sp)).map(|(i, sp)| (i, format!("{sp:?}"))).collect();
    crate::hash_of((
        learned_key(p),
        [wb_t, wb_tint, info.as_shot_temp, info.as_shot_tint].map(f64::to_bits),
        format!("{:?}", s.optics),
        crate::local_tone::enabled(info, s),
        learned,
        done,
    ))
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
/// balance) and the Classic ones (after it), each a layer of its own.
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
/// drawn on) for only the spots an edit reaches. Otherwise `prepare` makes `img` the layer's image and every spot is
/// drawn. `draw` draws spot `i` and says whether it drew (a spot that did not is tried again next time). `img` ends as
/// the layer's result.
fn layer(
    img: &mut Rgb32f,
    key: u64,
    kept: Option<&CpuLayer>,
    mut new: Vec<Option<Drawn>>,
    prepare: impl FnOnce(&mut Rgb32f),
    mut draw: impl FnMut(&mut Rgb32f, usize) -> bool,
) -> CpuLayer {
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
    let plan = hit.map(|k| (k, redo(&k.drawn, &new))).filter(|(k, r)| k.has_patches(r));
    if crate::profiling() {
        let why = plan.as_ref().map_or_else(|| "drew all".to_string(), |(_, r)| format!("drew {} (took off {})", r.drawn(), r.undo.len()));
        eprintln!("  spots: {} of {} in {}×{}: {why}", new.iter().flatten().count(), new.len(), img.width, img.height);
    }
    match plan {
        Some((k, r)) => {
            *img = (*k.post).clone();
            // (a spot not drawn again keeps its patches: it is the old one, reading the same pixels)
            for (slot, old) in patches.iter_mut().zip(&r.same) {
                *slot = old.and_then(|o| k.patches.get(o).cloned().flatten());
            }
            for u in &r.undo {
                if let (Some(Some(d)), Some(Some(pt))) = (k.drawn.get(*u), k.patches.get(*u)) {
                    paste(img, &d.at.write, &pt.before);
                }
            }
            for step in &r.steps {
                match *step {
                    Step::Draw(i) => draw_one(img, i, &mut new, &mut patches),
                    Step::Paste { i, old } => {
                        if let (Some(Some(d)), Some(Some(pt))) = (new.get(i), k.patches.get(old)) {
                            paste(img, &d.at.write, &pt.after);
                        }
                    }
                }
            }
        }
        None => {
            prepare(img);
            for i in 0..new.len() {
                draw_one(img, i, &mut new, &mut patches);
            }
        }
    }
    Kept { key, post: Arc::new(img.clone()), drawn: new, patches, done: Vec::new() }
}

/// [`crate::lin_cpu`] on `img` (the resampled source), reusing `kept` (the view's last spots): only the spots an edit
/// reaches are drawn, LaMa and Classic alike, and the white balance runs only when the image the Classic spots are
/// drawn on changed. Returns what to keep for the next render (`None` without spots).
pub fn lin_cpu(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>, kept: Option<&CpuKept>) -> Option<Arc<CpuKept>> {
    let s = &*p.settings;
    if s.spots.is_empty() {
        crate::lin_cpu(img, info, p);
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
    let learned = lnew.iter().any(Option::is_some).then(|| {
        let provider = crate::learned_fill::current();
        let at = crate::spots_learned::Where { frame: &p.frame, ppl, reference_long: crate::native_long(info, p.src_long) as f64, draft: p.draft };
        let run = crate::learned_fill::FillRun::new();
        let kept = kept.and_then(|k| k.learned.as_ref());
        layer(
            img,
            learned_key(p),
            kept,
            lnew,
            |_| {},
            |img, i| {
                let (Some(pr), Some(sp)) = (provider.as_ref(), s.spots.get(i)) else { return false };
                crate::spots_learned::apply_one(img, sp, &at, &**pr, &run)
            },
        )
    });
    let done: Vec<bool> = learned.as_ref().map(|k| k.drawn.iter().map(Option::is_some).collect()).unwrap_or_default();
    // 2. the Classic spots (and the LaMa ones that got no fill), after the white balance
    let cnew = entries(&s.spots, &done, &p.frame, w, h, ppl);
    let prepare = |img: &mut Rgb32f| {
        // (`crate::lin_cpu`'s order between its LaMa and Classic spots)
        crate::local_tone::apply(img, info, p);
        crate::local::white_balance(img, info, s);
        crate::optics::defringe(img, s, ppl / crate::optics::DEFRINGE_REF_LONG);
    };
    let classic = layer(img, key(info, p, &done), kept.map(|k| &k.classic), cnew, prepare, |img, i| {
        if let Some(sp) = s.spots.get(i) {
            spots::apply(img, std::slice::from_ref(sp), &p.frame, ppl);
        }
        true
    });
    crate::redeye::apply(img, &p.eyes);
    Some(Arc::new(CpuKept { learned, classic }))
}

#[cfg(test)]
#[path = "tests_spots_keep.rs"]
mod tests;
