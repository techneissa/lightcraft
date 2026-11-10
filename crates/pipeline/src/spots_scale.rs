//! Spots finished once per scale, for every window of it (issue #176).
//!
//! A zoomed-in render is a window of the whole frame at that scale. Spots read pixels around themselves (their
//! source, the rings an automatic source searches, the surroundings a fill rebuilds from), so a window used to grow
//! until it held every spot touching it, their reads, and every spot those touch in turn: with spots spread over the
//! photo, 100 % rendered nearly the whole photo at full size, and every pan or zoom drew every spot again, its
//! Content-Aware fill and its LaMa fill included.
//!
//! A view now keeps each spot's finished pixels ([`Finals`]): its box after every spot drew, in pixels of the whole
//! frame at the render's scale, under a key of everything those pixels depend on ([`key`]: the whole frame and its
//! size, the settings before the spots and the spots themselves) and nothing about the window. A spot's pixels are
//! kept only when the render that drew them held everything they depend on ([`exact`]). A later window of the same
//! scale whose spots are all kept ([`Finals::covers`]) does not grow for them and draws none: it pastes their pixels
//! over its own image after the white balance ([`paste`]). Any change before or to the spots changes the key.
//!
//! What the zoom needs from it: full-size spot results a window can be cut from, keyed by the frame and scale, so
//! a pan or a zoom between windows of one scale starts no fill and no source search.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::DevelopSettings;
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::spots_keep::Rect;
use crate::{PixelWindow, SourceInfo};

/// Most bytes of finished spot pixels a view keeps; past it nothing is kept and windows draw their spots.
pub const MAX_BYTES: usize = 256 << 20;

/// Most spots whose pixels are kept.
pub const MAX_SPOTS: usize = 2000;

/// One spot at a scale: where it writes and everything its pixels depend on, in pixels of the whole frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub write: Rect,
    pub read: Rect,
}

#[derive(Clone, Debug)]
enum Entry {
    /// Draws nothing at this scale.
    Nothing,
    /// Not kept yet: a window it touches draws its spots.
    Unknown(Rect),
    /// Its box after every spot drew, row by row.
    Exact(Rect, Arc<Vec<[f32; 3]>>),
}

/// Each spot's finished pixels at one scale, under [`key`].
#[derive(Debug)]
pub struct Finals {
    key: u64,
    entries: Vec<Entry>,
    bytes: usize,
}

impl Finals {
    /// Whether every spot that writes into `work` (pixels of the whole frame) is kept.
    pub fn covers(&self, work: &Rect) -> bool {
        self.entries.iter().all(|e| match e {
            Entry::Unknown(r) => !overlaps(r, work),
            _ => true,
        })
    }

    /// Spots kept.
    pub fn kept(&self) -> usize {
        self.entries.iter().filter(|e| matches!(e, Entry::Exact(..))).count()
    }
}

/// A view's [`Finals`] (one scale at a time: the zoom's).
#[derive(Debug, Default)]
pub struct FinalsStore {
    v: Mutex<Option<Arc<Finals>>>,
}

impl FinalsStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arc<Finals>>> {
        self.v.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The finals kept under `key`.
    pub fn get(&self, key: u64) -> Option<Arc<Finals>> {
        self.lock().clone().filter(|f| f.key == key)
    }

    /// Forget them.
    pub fn clear(&self) {
        *self.lock() = None;
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.lock().as_ref().map_or(0, |f| f.bytes)
    }

    /// Add what a render found (`reach` of every spot, and the finished pixels of those it drew exactly) to what is kept
    /// under `key` (pixels of one key are the same whichever render made them); another key replaces it.
    fn merge(&self, key: u64, reach: &[Option<Reach>], found: Vec<Option<Arc<Vec<[f32; 3]>>>>) {
        let mut g = self.lock();
        let old = g.clone().filter(|f| f.key == key && f.entries.len() == reach.len());
        let entries: Vec<Entry> = reach
            .iter()
            .zip(found)
            .enumerate()
            .map(|(i, (r, px))| match (r, px) {
                (None, _) => Entry::Nothing,
                (Some(r), Some(px)) => Entry::Exact(r.write, px),
                (Some(r), None) => match old.as_ref().and_then(|o| o.entries.get(i)) {
                    Some(e @ Entry::Exact(..)) => e.clone(),
                    _ => Entry::Unknown(r.write),
                },
            })
            .collect();
        let bytes = entries.iter().map(|e| if let Entry::Exact(_, px) = e { px.len() * 12 } else { 0 }).sum();
        *g = (bytes <= MAX_BYTES).then(|| Arc::new(Finals { key, entries, bytes }));
    }
}

/// What a plan knows of its view's kept spots: the store, the key of its scale and, for a window that pastes them,
/// the finals and where the window lies in the whole frame.
#[derive(Clone, Debug, Default)]
pub struct At {
    pub store: Option<Arc<FinalsStore>>,
    pub key: u64,
    /// Every spot's reach at this scale (empty without a store).
    pub reach: Arc<Vec<Option<Reach>>>,
    /// The rendered window's top-left in the whole frame (`None`: not a window, nothing is kept or pasted).
    pub origin: Option<(usize, usize)>,
    /// The finals this window pastes instead of drawing its spots.
    pub paste: Option<Arc<Finals>>,
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn inside(a: &Rect, b: &Rect) -> bool {
    a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3]
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (a, b) => a.or(b),
    }
}

/// The window as a [`Rect`].
pub fn rect(w: &PixelWindow) -> Rect {
    [w.x, w.y, w.x.saturating_add(w.w), w.y.saturating_add(w.h)]
}

/// Key of the spots' finished pixels at a scale: the whole frame and its size, what the image before the spots
/// depends on (as `spots_keep::key` and [`crate::Plan::pre_retouch_key`] hold it) and the spots, never the window.
pub fn key(info: &SourceInfo, s: &DevelopSettings, frame: &Frame, (w, h): (usize, usize), ppl: f64, longs: (usize, usize), draft: bool) -> u64 {
    let (wb_t, wb_tint) = crate::local::effective_wb(info, s);
    let learned = s.spots.iter().any(crate::spots_learned::is_learned);
    crate::hash_of((
        format!("{frame:?}"),
        (w, h),
        ppl.to_bits(),
        longs,
        [wb_t, wb_tint, info.as_shot_temp, info.as_shot_tint].map(f64::to_bits),
        format!("{:?}", s.optics),
        crate::spots_keep::pre_spot_sections(s),
        crate::local_tone::enabled(info, s),
        format!("{:?}", s.spots),
        // a draft may show a LaMa spot with the Classic fill; a full render must not take it
        draft && learned,
    ))
}

/// Each spot's reach in the whole `w × h` frame at `ppl`: its Classic and LaMa boxes, and what they read, grown by
/// defringe's radius (the image the spots read is defringed first).
pub fn reach(s: &DevelopSettings, frame: &Frame, w: usize, h: usize, ppl: f64) -> Vec<Option<Reach>> {
    let halo = crate::optics::defringe_params(s, ppl / crate::optics::DEFRINGE_REF_LONG).map_or(0, |d| d.radius.saturating_add(1));
    s.spots
        .iter()
        .map(|sp| {
            if sp.hidden {
                return None;
            }
            let classic = crate::spots_keep::footprint(sp, frame, w, h, ppl);
            let learned = crate::spots_learned::footprint(sp, frame, w, h, ppl);
            let write = union(classic.map(|f| f.write), learned.map(|l| l.0))?;
            let read = union(Some(write), union(classic.map(|f| f.read), learned.map(|l| l.1)))?;
            let read = [read[0].saturating_sub(halo), read[1].saturating_sub(halo), (read[2] + halo).min(w), (read[3] + halo).min(h)];
            Some(Reach { write, read })
        })
        .collect()
}

/// `work` grown, as [`crate::spots::window_for_reads`] grows a window, until it holds the whole reach of every spot
/// whose reach touches it, so each of them is drawn exactly and can be kept; `work` itself when that passes `max`
/// pixels along an axis.
pub fn grow(reach: &[Option<Reach>], work: &PixelWindow, max: usize) -> PixelWindow {
    let mut r = rect(work);
    for _ in 0..=reach.len().min(MAX_SPOTS) {
        let mut g = r;
        for x in reach.iter().flatten() {
            if overlaps(&x.read, &r) || overlaps(&x.write, &r) {
                g = [g[0].min(x.read[0]), g[1].min(x.read[1]), g[2].max(x.read[2]), g[3].max(x.read[3])];
            }
        }
        if g == r {
            break;
        }
        if g[2] - g[0] > max || g[3] - g[1] > max {
            return *work;
        }
        r = g;
    }
    PixelWindow { x: r[0], y: r[1], w: r[2] - r[0], h: r[3] - r[1] }
}

/// Which spots a render of `work` (pixels of the whole frame) drew exactly as the whole frame does: each spot whose
/// reach lies inside it, unless it reads or writes where a spot that is not exact writes (in either order: an earlier
/// spot changed what it read, a later one drew over its box).
pub fn exact(reach: &[Option<Reach>], work: &Rect) -> Vec<bool> {
    // (each pass is quadratic: past this many spots none is kept, and windows draw them as before)
    if reach.len() > MAX_SPOTS {
        return vec![false; reach.len()];
    }
    let mut ok: Vec<bool> = reach.iter().map(|r| r.is_some_and(|r| inside(&r.read, work) && inside(&r.write, work))).collect();
    loop {
        let mut changed = false;
        for (i, r) in reach.iter().enumerate() {
            let Some(r) = r else { continue };
            if !ok.get(i).copied().unwrap_or(false) {
                continue;
            }
            let tainted = reach.iter().enumerate().any(|(j, o)| {
                j != i && !ok.get(j).copied().unwrap_or(true) && o.is_some_and(|o| overlaps(&o.write, &r.read) || overlaps(&o.write, &r.write))
            });
            if tainted && let Some(slot) = ok.get_mut(i) {
                *slot = false;
                changed = true;
            }
        }
        if !changed {
            return ok;
        }
    }
}

/// After a window's spots drew on `img` (its pixels, the window at `at.origin`): keep the finished pixels of those
/// drawn exactly (`drawn`: the spots that drew).
pub fn capture(at: &At, img: &Rgb32f, drawn: &[bool]) {
    let found = exact_boxes(at, img.width, img.height, drawn).into_iter().map(|(i, r)| (i, crate::spots_keep::cut(img, &r))).collect();
    keep(at, found);
}

/// The kept spots' pixels inside a `w × h` window at `origin` of the whole frame: each box cut to the window, in the
/// window's pixels, in the order the spots drew (boxes overlap only where they hold the same finished pixels).
pub fn pieces(finals: &Finals, (ox, oy): (usize, usize), w: usize, h: usize) -> Vec<(Rect, Vec<[f32; 3]>)> {
    let work = [ox, oy, ox.saturating_add(w), oy.saturating_add(h)];
    finals
        .entries
        .iter()
        .filter_map(|e| {
            let Entry::Exact(r, px) = e else { return None };
            if !overlaps(r, &work) {
                return None;
            }
            let (x0, y0, x1, y1) = (r[0].max(work[0]), r[1].max(work[1]), r[2].min(work[2]), r[3].min(work[3]));
            let rw = r[2] - r[0];
            let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0));
            for y in y0..y1 {
                let from = (y - r[1]) * rw + (x0 - r[0]);
                out.extend_from_slice(px.get(from..from + (x1 - x0))?);
            }
            Some(([x0 - ox, y0 - oy, x1 - ox, y1 - oy], out))
        })
        .collect()
}

/// Paste the kept spots that write into the window `img` (at `origin` in the whole frame).
pub fn paste(img: &mut Rgb32f, finals: &Finals, origin: (usize, usize)) {
    for (r, px) in pieces(finals, origin, img.width, img.height) {
        crate::spots_keep::paste(img, &r, &px);
    }
}

/// The spots a window of `w × h` at `at.origin` drew exactly, with their boxes in the window's pixels (`drawn`: the
/// spots that drew; a LaMa spot whose fill was not ready did not).
pub fn exact_boxes(at: &At, w: usize, h: usize, drawn: &[bool]) -> Vec<(usize, Rect)> {
    let Some((ox, oy)) = at.origin.filter(|_| at.store.is_some()) else { return Vec::new() };
    let ok = exact(&at.reach, &[ox, oy, ox + w, oy + h]);
    at.reach
        .iter()
        .enumerate()
        .filter(|(i, _)| ok.get(*i).copied().unwrap_or(false) && drawn.get(*i).copied().unwrap_or(false))
        .filter_map(|(i, r)| r.map(|r| (i, [r.write[0] - ox, r.write[1] - oy, r.write[2] - ox, r.write[3] - oy])))
        .collect()
}

/// Keep `found` (spot, its finished pixels) for the view, at `at`'s key.
pub fn keep(at: &At, found: Vec<(usize, Vec<[f32; 3]>)>) {
    let Some(store) = &at.store else { return };
    let mut px: Vec<Option<Arc<Vec<[f32; 3]>>>> = vec![None; at.reach.len()];
    for (i, p) in found {
        if let Some(slot) = px.get_mut(i) {
            *slot = Some(Arc::new(p));
        }
    }
    store.merge(at.key, &at.reach, px);
}

thread_local! {
    static PASTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Windows on this thread that pasted their spots instead of drawing them (a test counts them).
pub fn windows_pasted() -> usize {
    PASTED.with(std::cell::Cell::get)
}

/// Count one window that pasted its spots ([`windows_pasted`]).
pub fn note_pasted() {
    PASTED.with(|c| c.set(c.get().wrapping_add(1)));
}
