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
pub const MAX_BYTES: usize = 128 << 20;

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

/// Most scales finished spots are kept for (the zoom's 100 %, a pinch level or two, another photo's).
pub const MAX_SCALES: usize = 6;

/// Most bytes of LaMa fills a view keeps ([`KeptFill`]); past it the oldest is forgotten.
pub const FILL_BYTES: usize = 128 << 20;

/// A LaMa spot's fill as the model gave it, at the size it saw the crop (the original's pixels), with where that crop
/// lay: a render of the spot at this scale or below takes it, resampled to its own crop, and never asks the model.
#[derive(Clone, Debug)]
pub struct KeptFill {
    key: u64,
    /// Output pixels per long edge of the render that asked for it.
    ppl: f64,
    /// That render's normalized coordinates to its pixels, and its crop's top-left there.
    to_out: lightcraft_geom::Affine,
    c0: (f64, f64),
    /// Render pixels per fill pixel, across and down, and the fill's size.
    k: (f64, f64),
    size: (usize, usize),
    out: Arc<Vec<f32>>,
}

impl KeptFill {
    /// The fill of a `cw × ch` crop at `c0` of a render whose pixels `to_norm` maps to normalized coordinates, sampled
    /// from this one between pixel centres.
    pub fn resample(&self, to_norm: &lightcraft_geom::Affine, c0: (usize, usize), (cw, ch): (usize, usize)) -> Vec<f32> {
        let (fw, fh) = self.size;
        let mut out = Vec::with_capacity(cw * ch * 3);
        let at = |x: usize, y: usize, c: usize| {
            self.out.get((y.min(fh.saturating_sub(1)) * fw + x.min(fw.saturating_sub(1))) * 3 + c).copied().unwrap_or(0.0)
        };
        // a render pixel's centre in fill pixels (between centres)
        let fill_at = |x: f64, y: f64| {
            let q = self.to_out.apply(to_norm.apply(lightcraft_geom::Point::new(c0.0 as f64 + x, c0.1 as f64 + y)));
            ((q.x - self.c0.0) / self.k.0 - 0.5, (q.y - self.c0.1) / self.k.1 - 0.5)
        };
        // fill pixels per render pixel: shrinking (a finer render's fill shown at Fit) averages the fill pixels each render
        // pixel covers, so a textured fill does not alias (review F6); otherwise one bilinear sample
        let (o, ax, ay) = (fill_at(0.5, 0.5), fill_at(1.5, 0.5), fill_at(0.5, 1.5));
        let step = |a: (f64, f64)| ((a.0 - o.0).hypot(a.1 - o.1)).clamp(1.0, 64.0);
        let (sx, sy) = (step(ax), step(ay));
        let (sx, sy) = (if sx.is_finite() { sx } else { 1.0 }, if sy.is_finite() { sy } else { 1.0 });
        let max = (fw.saturating_sub(1) as f64, fh.saturating_sub(1) as f64);
        for y in 0..ch {
            for x in 0..cw {
                let (fx, fy) = fill_at(x as f64 + 0.5, y as f64 + 0.5);
                let (fx, fy) = (if fx.is_finite() { fx } else { 0.0 }, if fy.is_finite() { fy } else { 0.0 });
                if sx > 1.0 + 1e-6 || sy > 1.0 + 1e-6 {
                    // the fill pixels whose centres fall in this pixel's footprint, [c − s/2, c + s/2) (at least the nearest)
                    let span = |c: f64, s: f64, hi: f64| {
                        let (a, b) = ((c - s / 2.0).ceil().clamp(0.0, hi), ((c + s / 2.0).ceil() - 1.0).clamp(0.0, hi));
                        if b < a { (c.round().clamp(0.0, hi) as usize, c.round().clamp(0.0, hi) as usize) } else { (a as usize, b as usize) }
                    };
                    let ((x0, x1), (y0, y1)) = (span(fx, sx, max.0), span(fy, sy, max.1));
                    let n = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f32;
                    for c in 0..3 {
                        let sum: f32 = (y0..=y1).flat_map(|yy| (x0..=x1).map(move |xx| (xx, yy))).map(|(xx, yy)| at(xx, yy, c)).sum();
                        out.push(sum / n);
                    }
                    continue;
                }
                let (fx, fy) = (fx.clamp(0.0, max.0), fy.clamp(0.0, max.1));
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
                for c in 0..3 {
                    let top = at(x0, y0, c) * (1.0 - tx) + at(x0 + 1, y0, c) * tx;
                    let bottom = at(x0, y0 + 1, c) * (1.0 - tx) + at(x0 + 1, y0 + 1, c) * tx;
                    out.push(top * (1.0 - ty) + bottom * ty);
                }
            }
        }
        out
    }

    fn bytes(&self) -> usize {
        self.out.len() * 4
    }
}

/// Where a render asked for a LaMa fill: its output pixels per long edge, its normalized coordinates to its pixels, its
/// crop's top-left there, and render pixels per fill pixel across and down.
#[derive(Clone, Copy, Debug)]
pub struct FillAt {
    pub ppl: f64,
    pub to_out: lightcraft_geom::Affine,
    pub c0: (usize, usize),
    pub k: (f64, f64),
}

/// The store every view shares ([`shared`]): a view let go at Fit (the zoom's) finds its spots again when it comes back,
/// and the source's [`fingerprint`] in every key keeps photos apart.
static SHARED: std::sync::LazyLock<Arc<FinalsStore>> = std::sync::LazyLock::new(Arc::default);

/// Most automatic sources kept ([`picked`]).
pub const MAX_PICKS: usize = 4096;

static PICKS: Mutex<Vec<(u64, Option<lightcraft_geom::Point>)>> = Mutex::new(Vec::new());

/// Whether `spot` copies from a source: Remove, Heal and Clone do; a Content-Aware, shaped or LaMa spot is rebuilt from
/// its surroundings and reads none.
pub fn reads_source(spot: &lightcraft_develop::Spot) -> bool {
    spot.mode != lightcraft_develop::SpotMode::Fill && spot.area.is_none() && !crate::spots_learned::is_learned(spot)
}

/// The key of `spot`'s automatic source on the source fingerprinted `source`: what [`crate::spots::pick_source`] reads
/// (the source, the settings that place it, the lens the file embeds, the spot), never the window or the scale.
pub fn pick_key(source: u64, s: &DevelopSettings, info: &SourceInfo, spot: &lightcraft_develop::Spot) -> u64 {
    crate::hash_of((
        source,
        format!("{:?}", (&s.optics, &s.geometry, &s.crop, &s.orientation, &info.lens)),
        crate::spots_keep::pre_spot_sections(s),
        format!("{spot:?}"),
    ))
}

/// The automatic source kept under `key`, else `pick`'s, kept (issue #176, review F1: every window searched each spot's
/// source again on a 512-pixel copy of the whole photo).
pub fn picked(key: u64, pick: impl FnOnce() -> Option<lightcraft_geom::Point>) -> Option<lightcraft_geom::Point> {
    if let Some((_, o)) = PICKS.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
        return *o;
    }
    let o = pick();
    let mut v = PICKS.lock().unwrap_or_else(PoisonError::into_inner);
    v.retain(|(k, _)| *k != key);
    v.push((key, o));
    if v.len() > MAX_PICKS {
        v.remove(0);
    }
    o
}

/// The store views share.
pub fn shared() -> Arc<FinalsStore> {
    SHARED.clone()
}

/// The shared store as a field whose default is it (another renderer's per-view state).
#[derive(Debug)]
pub struct Shared(pub Arc<FinalsStore>);

impl Default for Shared {
    fn default() -> Self {
        Shared(shared())
    }
}

/// [`Finals`] of a few scales, and LaMa fills ([`KeptFill`]).
#[derive(Debug, Default)]
pub struct FinalsStore {
    v: Mutex<Vec<Arc<Finals>>>,
    fills: Mutex<Vec<KeptFill>>,
}

impl FinalsStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<Finals>>> {
        self.v.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_fills(&self) -> std::sync::MutexGuard<'_, Vec<KeptFill>> {
        self.fills.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The finals kept under `key`.
    pub fn get(&self, key: u64) -> Option<Arc<Finals>> {
        self.lock().iter().find(|f| f.key == key).cloned()
    }

    /// Forget them, and the fills.
    pub fn clear(&self) {
        self.lock().clear();
        self.lock_fills().clear();
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.lock().iter().map(|f| f.bytes).sum::<usize>() + self.lock_fills().iter().map(KeptFill::bytes).sum::<usize>()
    }

    /// The fill kept for the LaMa spot `key` from a render at `ppl` or finer.
    pub fn fill(&self, key: u64, ppl: f64) -> Option<KeptFill> {
        self.lock_fills().iter().find(|f| f.key == key && f.ppl >= ppl * (1.0 - 1e-9)).cloned()
    }

    /// Keep the fill `out` (`size`, as the model gave it) of LaMa spot `key`, asked by a render `at`.
    pub fn keep_fill(&self, key: u64, at: FillAt, size: (usize, usize), out: Arc<Vec<f32>>) {
        let FillAt { ppl, to_out, c0, k } = at;
        let f = KeptFill { key, ppl, to_out, c0: (c0.0 as f64, c0.1 as f64), k, size, out };
        let mut v = self.lock_fills();
        v.retain(|o| o.key != key);
        if f.bytes() <= FILL_BYTES {
            v.push(f);
        }
        while v.iter().map(KeptFill::bytes).sum::<usize>() > FILL_BYTES {
            v.remove(0);
        }
    }

    /// Add what a render found (`reach` of every spot, and the finished pixels of those it drew exactly) to what is kept
    /// under `key` (pixels of one key are the same whichever render made them); the oldest of more than [`MAX_SCALES`]
    /// keys, or past [`MAX_BYTES`], is forgotten.
    fn merge(&self, key: u64, reach: &[Option<Reach>], found: Vec<Option<Arc<Vec<[f32; 3]>>>>) {
        let mut g = self.lock();
        let old = g.iter().position(|f| f.key == key && f.entries.len() == reach.len()).map(|i| g.remove(i));
        g.retain(|f| f.key != key);
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
        if bytes <= MAX_BYTES {
            g.push(Arc::new(Finals { key, entries, bytes }));
        }
        while g.len() > MAX_SCALES || g.iter().map(|f| f.bytes).sum::<usize>() > MAX_BYTES {
            g.remove(0);
        }
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
    /// What a LaMa spot's fill depends on besides the spots: the source and the geometry, never the scale ([`fill_key`]).
    pub fill_base: u64,
}

/// [`At::fill_base`]: the source's [`fingerprint`], the settings that place it (optics, geometry, crop, orientation, the
/// sections' eyes), and the original's size.
pub fn fill_base(source: u64, s: &DevelopSettings, native_long: usize) -> u64 {
    crate::hash_of((source, format!("{:?}", (&s.optics, &s.geometry, &s.crop, &s.orientation)), crate::spots_keep::pre_spot_sections(s), native_long))
}

/// A fingerprint of `src`: its size and a grid of 64 of its pixels.
pub fn fingerprint(src: &Rgb32f) -> u64 {
    let (w, h) = (src.width, src.height);
    let grid: Vec<[u32; 3]> = (0..64)
        .filter_map(|i| src.data.get((i / 8 * h / 8 + h / 16).min(h.saturating_sub(1)) * w + (i % 8 * w / 8 + w / 16).min(w.saturating_sub(1))))
        .map(|p| p.map(f32::to_bits))
        .collect();
    crate::hash_of(((w, h), grid))
}

/// The key of learned spot `i`'s fill: [`At::fill_base`] and the learned spots up to it (each is drawn on what the earlier
/// ones left).
pub fn fill_key(base: u64, spots: &[lightcraft_develop::Spot], i: usize) -> u64 {
    let learned: Vec<String> =
        spots.iter().take(i.saturating_add(1)).filter(|sp| crate::spots_learned::is_learned(sp)).map(|sp| format!("{sp:?}")).collect();
    crate::hash_of((base, learned))
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

/// Which spots a render of `work` (pixels of the whole frame) drew exactly as the whole frame does: each spot that drew
/// (`drawn`) whose reach lies inside it, unless it reads or writes where a spot that is not exact writes (in either order: an earlier
/// spot changed what it read, a later one drew over its box).
pub fn exact(reach: &[Option<Reach>], work: &Rect, drawn: &[bool]) -> Vec<bool> {
    // (each pass is quadratic: past this many spots none is kept, and windows draw them as before)
    if reach.len() > MAX_SPOTS {
        return vec![false; reach.len()];
    }
    // (a spot that did not draw, a LaMa spot whose fill has not come, is not exact either: its neighbours read or drew
    // over pixels the fill will change, review F3)
    let mut ok: Vec<bool> = reach
        .iter()
        .enumerate()
        .map(|(i, r)| r.is_some_and(|r| inside(&r.read, work) && inside(&r.write, work)) && drawn.get(i).copied().unwrap_or(false))
        .collect();
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
    let ok = exact(&at.reach, &[ox, oy, ox + w, oy + h], drawn);
    at.reach
        .iter()
        .enumerate()
        .filter(|(i, _)| ok.get(*i).copied().unwrap_or(false))
        .filter_map(|(i, r)| r.map(|r| (i, [r.write[0] - ox, r.write[1] - oy, r.write[2] - ox, r.write[3] - oy])))
        .collect()
}

/// Keep `found` (spot, its finished pixels) for the view, at `at`'s key.
pub fn keep(at: &At, found: Vec<(usize, Vec<[f32; 3]>)>) {
    // (only a window says which spots it drew exactly: a whole frame keeping an empty list would make every window of
    // its scale paste nothing, review F2)
    let Some(store) = at.store.as_ref().filter(|_| at.origin.is_some() && !at.reach.is_empty()) else { return };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Review F6: a fine render's fill shown four times smaller averages its pixels; one bilinear sample of a one-pixel
    /// checkerboard gave black or white.
    #[test]
    fn a_fill_shrunk_averages_its_pixels() {
        let (fw, fh) = (64, 64);
        let out: Vec<f32> = (0..fw * fh).flat_map(|i| [((i % fw + i / fw) % 2) as f32; 3]).collect();
        // the fill at 1:1 with a render at its top-left; a render four times smaller maps 1 pixel to 4 of the fill's
        let fill = KeptFill {
            key: 0,
            ppl: 4.0,
            // (every render pixel's centre on a fill pixel's: one sample there is black)
            to_out: lightcraft_geom::Affine::translate(lightcraft_geom::Vec2::new(0.5, 0.5)) * lightcraft_geom::Affine::scale(4.0, 4.0),
            c0: (0.0, 0.0),
            k: (1.0, 1.0),
            size: (fw, fh),
            out: Arc::new(out),
        };
        let small = fill.resample(&lightcraft_geom::Affine::IDENTITY, (0, 0), (16, 16));
        let worst = small.iter().map(|v| (v - 0.5).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.01, "a checkerboard shrunk is grey, not aliased: {worst} off");
        // at 1:1 it is the fill itself
        let same = KeptFill { to_out: lightcraft_geom::Affine::IDENTITY, ..fill.clone() };
        assert_eq!(same.resample(&lightcraft_geom::Affine::IDENTITY, (0, 0), (fw, fh)), *fill.out);
    }
}
