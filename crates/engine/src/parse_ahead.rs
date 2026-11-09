//! Fork feature (`fork/parse-ahead.md`): people parsing ahead of time, so opening a photo or applying a portrait
//! preset finds its part masks ready instead of waiting seconds for the models.
//!
//! A low-priority queue ([`queue`]) parses, one photo at a time on a worker thread ([`worker`]): the photos around the
//! open one in the filmstrip's order, photos imported since the library opened that have faces, and, while the user is
//! idle, every other photo with faces, newest first. Parses go into people parsing's own disk cache, the same file a
//! later request reads.
//!
//! Never in the way:
//! - The frontend's per-frame pump (`parsing.ahead.pump`) gives the pace: nothing new while the user drags, types or
//!   scrolls, or while the photo on screen is being made ([`crate::prep::foreground_busy`]); a model run going then is
//!   stopped (unless someone waits for it) and run again later. Another parse (a request) always goes first.
//! - One photo at a time, so the GPU turn is given up between photos.
//! - Nothing in Low Power Mode or on battery below [`power::MIN_BATTERY`] %; nothing when the setting is off
//!   (Settings ▸ Performance, on by default).
//! - One quiet row in the activity stack, "Finding people parts ahead", with a count; its ✕ stops the run and empties
//!   the queue until another photo is opened.
//!
//! Cache: the parse files keep people parsing's size limit, the least recently used going first (a lookup marks a
//! file as used). The idle sweep stops at [`IDLE_FILL`] of the limit, so it never pushes out parses for its own. A
//! photo already parsed for the current model set is skipped without being rendered: an index beside the cache
//! ([`index`]) files each parse's key by what the parse depends on ([`identity`]).

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lightcraft_catalog::{Photo, PhotoId};
use lightcraft_meta::RegionKind;
use lightcraft_preview::Hasher128;
use serde_json::{Value, json};

use crate::Session;
use crate::activity::{Cancel, TaskGuard};
use crate::people_parsing::{PARSE_EDGE, QuietRun};

pub(crate) mod index;
pub mod power;
pub(crate) mod queue;
mod worker;

use index::{Index, Listing};
use queue::Why;

/// The share of the cache's size limit the idle sweep fills at most.
pub const IDLE_FILL: f64 = 0.9;
/// How often the queue is rebuilt after the catalog changed (the photos around the open one: at once).
const REBUILD_EVERY: Duration = Duration::from_secs(3);

/// How hard parsing ahead may work, from what the user is doing (the frontend says, as for the face scan).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Start nothing; stop a model run going.
    Pause,
    /// The user is around: the photos near the open one and new imports.
    Light,
    /// The user is idle: everything.
    Normal,
}

impl Pace {
    /// `pause`, `light`, `normal` or `full` (as `normal`); anything else is `light`.
    pub fn parse(name: Option<&str>) -> Pace {
        match name {
            Some("pause") => Pace::Pause,
            Some("normal" | "full") => Pace::Normal,
            _ => Pace::Light,
        }
    }

    fn allows(self, why: Why) -> bool {
        match self {
            Pace::Pause => false,
            Pace::Light => why != Why::Idle,
            Pace::Normal => true,
        }
    }
}

/// What parsing ahead has done since the app started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Photos parsed (a model ran).
    pub parsed: u64,
    /// Photos found parsed already when their turn came (a request had parsed them).
    pub cached: u64,
    /// Photos skipped without a render: their parse was in the cache.
    pub skipped: u64,
    pub failed: u64,
}

struct InFlight {
    id: PhotoId,
    why: Why,
    cancel: Arc<AtomicBool>,
    run: QuietRun,
}

/// What the queue was built from.
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    library: usize,
    /// Photos in the catalog: an import is queued at once.
    photos: usize,
    revision: u64,
    version: String,
    yunet: bool,
    cache: Option<PathBuf>,
}

/// Parsing ahead, in a session (`Session::parse_ahead`).
pub struct ParseAhead {
    enabled: bool,
    pub(crate) power: power::PowerWatch,
    worker: Option<worker::Worker>,
    hold: Arc<AtomicBool>,
    queue: VecDeque<(Why, PhotoId)>,
    /// The photos around the open one the queue was built with.
    near: Vec<PhotoId>,
    /// New imports and the idle sweep, in order, with their identities (rebuilt when the catalog changes; the photos
    /// around the open one are put in front of it whenever the open photo changes).
    rest: Vec<(Why, PhotoId, u128)>,
    stamp: Option<Stamp>,
    built_at: Option<Instant>,
    /// The library opening, and the photos it had when parsing ahead first looked (the rest are new imports).
    known: Option<(usize, HashSet<PhotoId>)>,
    /// Photos done since the app started (parsed or failed), by identity: a photo that changes is done again.
    tried: HashSet<u128>,
    in_flight: Option<InFlight>,
    index: Option<(PathBuf, Index, Listing)>,
    row: Option<TaskGuard>,
    row_cancel: Arc<AtomicBool>,
    /// Photos done since the row appeared.
    row_done: u64,
    /// ✕ was pressed while this photo was open: nothing until another one is.
    stopped_at: Option<Option<PhotoId>>,
    counts: Counts,
    state: &'static str,
}

impl Default for ParseAhead {
    fn default() -> Self {
        ParseAhead {
            enabled: true,
            power: power::PowerWatch::default(),
            worker: None,
            hold: Arc::new(AtomicBool::new(false)),
            queue: VecDeque::new(),
            near: Vec::new(),
            rest: Vec::new(),
            stamp: None,
            built_at: None,
            known: None,
            tried: HashSet::new(),
            in_flight: None,
            index: None,
            row: None,
            row_cancel: Arc::new(AtomicBool::new(false)),
            row_done: 0,
            stopped_at: None,
            counts: Counts::default(),
            state: "idle",
        }
    }
}

impl std::fmt::Debug for ParseAhead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseAhead").field("enabled", &self.enabled).field("queued", &self.queue.len()).field("state", &self.state).finish()
    }
}

impl Drop for ParseAhead {
    fn drop(&mut self) {
        self.drop_in_flight();
        if let Some((_, ix, listing)) = self.index.as_mut() {
            ix.save(listing, true);
        }
    }
}

impl ParseAhead {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Photos waiting.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Whether a photo is being parsed now.
    pub fn working(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Stop the photo going (its model run too) without counting it.
    fn drop_in_flight(&mut self) {
        if let Some(f) = &self.in_flight {
            f.cancel.store(true, Ordering::SeqCst);
            f.run.stop();
        }
    }

    /// Stop everything: the photo going, the queue and the row.
    fn stand_still(&mut self, state: &'static str) {
        self.drop_in_flight();
        self.queue.clear();
        self.near.clear();
        self.rest.clear();
        self.stamp = None;
        self.row = None;
        self.row_done = 0;
        self.state = state;
    }
}

/// What the parse of `p` depends on, as far as the catalog tells: its file, the render that makes its picture, its
/// faces (the regions and whether YuNet finds them) and the model set's version. Two photos, or one photo before and
/// after a change, with the same identity are parsed under the same key.
pub(crate) fn identity(p: &Photo, yunet: bool, version: &str) -> u128 {
    let mut h = Hasher128::new();
    h.str("parse-ahead/1")
        .str(&crate::media::content_key(p))
        .u64(crate::media::RENDER_CACHE_VERSION)
        .u64(PARSE_EDGE as u64)
        .str(version)
        .u64(u64::from(yunet))
        .str(&format!("{:?}", p.embedded_lens));
    for r in p.meta.regions.iter().filter(|r| r.kind == RegionKind::Face) {
        h.u64(r.rect.x0.to_bits()).u64(r.rect.y0.to_bits()).u64(r.rect.x1.to_bits()).u64(r.rect.y1.to_bits());
        h.str(r.name.as_deref().unwrap_or("\u{0}"));
    }
    h.finish().0
}

fn has_faces(p: &Photo) -> bool {
    p.meta.regions.iter().any(|r| r.kind == RegionKind::Face)
}

fn parseable(p: &Photo) -> bool {
    p.in_library() && !p.is_video()
}

/// The newest-first date of a photo: when it was taken, else when it was imported.
fn taken(p: &Photo) -> String {
    p.captured.clone().unwrap_or_else(|| p.imported.clone())
}

/// The photos around the open one worth parsing: those with faces, and those the face scan has not looked at yet.
fn near_photos(s: &mut Session, backward: bool) -> Vec<PhotoId> {
    let Some(at) = s.active() else { return Vec::new() };
    let view = s.visible_cloned();
    queue::near(&view, at, backward)
        .into_iter()
        .filter(|id| s.catalog.photo(*id).is_some_and(|p| parseable(p) && (has_faces(p) || !s.faces.scanned.contains(id.0))))
        .collect()
}

/// Rebuild the photos after the open one's neighbours: new imports with faces and (while the cache has room) every
/// other photo with faces, minus those whose parse is in the cache. Reads the cache folder and hashes each photo with
/// faces: done when the catalog changed, not when the user steps to another photo.
fn rebuild_rest(s: &mut Session, stamp: &Stamp) {
    if s.parse_ahead.known.as_ref().is_none_or(|(lib, _)| *lib != stamp.library) {
        let ids = s.catalog.photos().map(|p| p.id).collect();
        s.parse_ahead.known = Some((stamp.library, ids));
    }
    let room = match &mut s.parse_ahead.index {
        Some((dir, _, listing)) => {
            *listing = Listing::read(dir);
            (listing.bytes as f64) < crate::people_parsing::cache_budget() as f64 * IDLE_FILL
        }
        None => false,
    };
    let (mut imported, mut idle) = (Vec::new(), Vec::new());
    let known = s.parse_ahead.known.as_ref().map(|k| &k.1);
    for p in s.catalog.photos().filter(|p| parseable(p) && has_faces(p)) {
        if known.is_some_and(|k| !k.contains(&p.id)) {
            imported.push((p.id, p.imported.clone()));
        } else if room {
            idle.push((p.id, taken(p)));
        }
    }
    let order = queue::order(&[], imported, idle);
    let mut rest = Vec::with_capacity(order.len());
    for (why, id) in order {
        let Some(p) = s.catalog.photo(id) else { continue };
        let ident = identity(p, stamp.yunet, &stamp.version);
        if !skip(&mut s.parse_ahead, ident) {
            rest.push((why, id, ident));
        }
    }
    s.parse_ahead.rest = rest;
    s.parse_ahead.built_at = Some(Instant::now());
}

/// Whether the photo with `ident` needs no parse: done since the app started, or its parse is in the cache (then it
/// counts as skipped, once).
fn skip(ahead: &mut ParseAhead, ident: u128) -> bool {
    if ahead.tried.contains(&ident) {
        return true;
    }
    let cached = ahead.index.as_ref().is_some_and(|(_, ix, listing)| ix.get(ident).is_some_and(|k| listing.keys.contains(k)));
    if cached {
        ahead.tried.insert(ident);
        ahead.counts.skipped += 1;
    }
    cached
}

/// The queue: the photos around the open one, then the rest, each once, minus what is done and the photo going.
fn requeue(s: &mut Session, near: Vec<PhotoId>, stamp: &Stamp) {
    let going = s.parse_ahead.in_flight.as_ref().map(|f| f.id);
    let mut queue = VecDeque::with_capacity(near.len() + s.parse_ahead.rest.len());
    for id in &near {
        let Some(p) = s.catalog.photo(*id) else { continue };
        let ident = identity(p, stamp.yunet, &stamp.version);
        if Some(*id) != going && !skip(&mut s.parse_ahead, ident) {
            queue.push_back((Why::Near, *id));
        }
    }
    let ahead = &mut s.parse_ahead;
    ahead.rest.retain(|(_, _, ident)| !ahead.tried.contains(ident));
    queue.extend(ahead.rest.iter().filter(|(_, id, _)| !near.contains(id) && Some(*id) != going).map(|(why, id, _)| (*why, *id)));
    ahead.queue = queue;
    ahead.near = near;
}

/// Take in what the worker finished.
fn collect(s: &mut Session) {
    let ahead = &mut s.parse_ahead;
    let Some(w) = &ahead.worker else { return };
    for f in w.finished() {
        if ahead.in_flight.as_ref().is_some_and(|x| x.id == f.id) {
            ahead.in_flight = None;
        }
        match f.ended {
            worker::Ended::Parsed { key, cached } => {
                ahead.tried.insert(f.identity);
                if cached {
                    ahead.counts.cached += 1;
                } else {
                    ahead.counts.parsed += 1;
                }
                ahead.row_done += 1;
                if let (Some(key), Some((dir, ix, listing))) = (key, ahead.index.as_mut()) {
                    listing.add(dir, &key);
                    ix.insert(f.identity, key);
                    // the cache filled up: the idle sweep ends here
                    if (listing.bytes as f64) >= crate::people_parsing::cache_budget() as f64 * IDLE_FILL {
                        ahead.queue.retain(|(why, _)| *why != Why::Idle);
                        ahead.rest.retain(|(why, _, _)| *why != Why::Idle);
                    }
                }
            }
            worker::Ended::Failed(e) => {
                log::info!("parse ahead: photo {}: {e}", f.id.0);
                ahead.tried.insert(f.identity);
                ahead.counts.failed += 1;
                ahead.row_done += 1;
            }
            // (not done: the next build queues it again)
            worker::Ended::Cancelled => ahead.stamp = None,
        }
    }
}

/// What holds the work back now, before the pace: the setting, the power, no models, nowhere to keep parses.
fn standing_still(s: &Session) -> Option<&'static str> {
    if !s.parse_ahead.enabled {
        return Some("off");
    }
    if let Some(why) = s.parse_ahead.power.now().holds_back() {
        return Some(why);
    }
    if !s.parsing.can_parse() {
        return Some("noModels");
    }
    if s.parsing.cache_dir().is_none() {
        return Some("noCache");
    }
    None
}

/// One pump: take in finished photos, follow the pace, rebuild the queue when what it was built from changed, and
/// hand the worker the next photo. Cheap when there is nothing to do.
pub(crate) fn pump(s: &mut Session, pace: Pace, backward: bool) {
    collect(s);
    if s.parse_ahead.row_cancel.swap(false, Ordering::SeqCst) {
        let at = s.active();
        s.parse_ahead.stand_still("stopped");
        s.parse_ahead.stopped_at = Some(at);
    }
    if let Some(why) = standing_still(s) {
        s.parse_ahead.stand_still(why);
        return;
    }
    if let Some(at) = s.parse_ahead.stopped_at {
        if s.active() == at {
            s.parse_ahead.state = "stopped";
            return;
        }
        s.parse_ahead.stopped_at = None;
    }
    // the index lives beside the parses
    open_index(s);
    let cache = s.parsing.cache_dir();
    let mine = s.parse_ahead.in_flight.as_ref().and_then(|f| f.run.key());
    let held = pace == Pace::Pause || crate::prep::foreground_busy() || s.parsing.others_running(mine.as_deref());
    s.parse_ahead.hold.store(held, Ordering::SeqCst);
    if held {
        // the model run going gives way, unless a request waits for it
        if let Some(f) = &s.parse_ahead.in_flight
            && f.run.running()
            && !mine.as_deref().is_some_and(|k| s.parsing.waited_on(k))
        {
            f.run.stop();
        }
    }
    let near = near_photos(s, backward);
    let yunet = crate::cmd::face_detect::detector(s).is_ok();
    let stamp = Stamp {
        library: Arc::as_ptr(&s.library_identity) as usize,
        photos: s.catalog.len(),
        revision: s.catalog.revision,
        version: s.parsing.ahead_version(),
        yunet,
        cache: cache.clone(),
    };
    let catalog_changed = match &s.parse_ahead.stamp {
        None => true,
        Some(old) => {
            old.library != stamp.library
                || old.photos != stamp.photos
                || old.version != stamp.version
                || old.yunet != stamp.yunet
                || old.cache != stamp.cache
                || (old.revision != stamp.revision && s.parse_ahead.built_at.is_none_or(|t| t.elapsed() >= REBUILD_EVERY))
        }
    };
    if catalog_changed {
        rebuild_rest(s, &stamp);
    }
    if catalog_changed {
        requeue(s, near, &stamp);
        s.parse_ahead.stamp = Some(stamp);
    } else if near != s.parse_ahead.near {
        // (the stamp stays: a catalog change waiting for its rebuild is still seen)
        requeue(s, near, &stamp);
    }
    if !held && s.parse_ahead.in_flight.is_none() {
        submit_next(s, pace);
    }
    let ahead = &mut s.parse_ahead;
    if let Some((_, ix, listing)) = ahead.index.as_mut() {
        ix.save(listing, false);
    }
    let left = ahead.queue.iter().filter(|(why, _)| pace == Pace::Pause || pace.allows(*why)).count();
    let busy = ahead.in_flight.is_some() || left > 0;
    ahead.state = if !busy {
        "done"
    } else if held {
        "paused"
    } else {
        "working"
    };
    if !busy {
        ahead.row = None;
        ahead.row_done = 0;
        return;
    }
    let row = ahead.row.get_or_insert_with(|| s.activity.start("parseAhead", "Finding people parts ahead", Cancel::Flag(ahead.row_cancel.clone())));
    let total = ahead.row_done + left as u64 + u64::from(ahead.in_flight.is_some());
    row.progress(ahead.row_done, total);
}

/// Hand the worker the first photo the pace allows.
fn submit_next(s: &mut Session, pace: Pace) {
    let yunet = crate::cmd::face_detect::detector(s).is_ok();
    let version = s.parsing.ahead_version();
    while let Some(pos) = s.parse_ahead.queue.iter().position(|(why, _)| pace.allows(*why)) {
        let Some((why, id)) = s.parse_ahead.queue.remove(pos) else { return };
        let Some(photo) = s.catalog.photo(id).cloned() else { continue };
        let ident = identity(&photo, yunet, &version);
        let prepare = match crate::cmd::parsing::prepare_job(s, id) {
            Ok(p) => p,
            Err(e) => {
                log::info!("parse ahead: photo {}: {e}", id.0);
                s.parse_ahead.tried.insert(ident);
                continue;
            }
        };
        let ahead = &mut s.parse_ahead;
        if ahead.worker.is_none() {
            match worker::Worker::start(s.parsing.clone(), ahead.hold.clone()) {
                Ok(w) => ahead.worker = Some(w),
                Err(e) => {
                    log::warn!("parse ahead: {e}");
                    ahead.queue.push_front((why, id));
                    return;
                }
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let run = QuietRun::default();
        let task = worker::Task { id, identity: ident, prepare, cancel: cancel.clone(), run: run.clone() };
        if ahead.worker.as_ref().is_some_and(|w| w.submit(task)) {
            ahead.in_flight = Some(InFlight { id, why, cancel, run });
        } else {
            // the thread has gone: a new one next time
            ahead.worker = None;
            ahead.queue.push_front((why, id));
        }
        return;
    }
}

/// Open the index beside the cache in use (saving the one of another folder first).
fn open_index(s: &mut Session) {
    let cache = s.parsing.cache_dir();
    if s.parse_ahead.index.as_ref().map(|i| &i.0) != cache.as_ref() {
        if let Some((_, ix, listing)) = s.parse_ahead.index.as_mut() {
            ix.save(listing, true);
        }
        s.parse_ahead.index = cache.as_ref().map(|d| (d.clone(), Index::open(d), Listing::read(d)));
    }
}

/// The parse of photo `id`, when one was filed for it as it is now (by parsing ahead or an earlier request): read from
/// the cache without rendering the photo. `None`: not known (or made by another model version); parse as usual.
pub(crate) fn ready(s: &mut Session, id: PhotoId) -> Option<Arc<lightcraft_parsing::Parsing>> {
    open_index(s);
    let yunet = crate::cmd::face_detect::detector(s).is_ok();
    let version = s.parsing.ahead_version();
    let ident = identity(s.catalog.photo(id)?, yunet, &version);
    let key = s.parse_ahead.index.as_ref()?.1.get(ident)?.to_string();
    // (a parse made another way, under a version this one does not name, is not taken for it)
    s.parsing.cached(&key).filter(|p| p.model_version == version)
}

/// A request parsed photo `id` under `key`: the next request finds it without a render.
pub(crate) fn remember(s: &mut Session, id: PhotoId, key: String) {
    open_index(s);
    let yunet = crate::cmd::face_detect::detector(s).is_ok();
    let version = s.parsing.ahead_version();
    let Some(ident) = s.catalog.photo(id).map(|p| identity(p, yunet, &version)) else { return };
    if let Some((dir, ix, listing)) = s.parse_ahead.index.as_mut() {
        listing.add(dir, &key);
        ix.insert(ident, key);
        ix.save(listing, false);
    }
}

impl Session {
    /// Turn parsing ahead on or off (the desktop app's setting; on by default). Off stops the photo going.
    pub fn set_parse_ahead(&mut self, on: bool) {
        self.parse_ahead.enabled = on;
        if !on {
            self.parse_ahead.stand_still("off");
        }
    }
}

/// `parsing.ahead.status` and the pump's answer.
pub(crate) fn status(s: &Session) -> Value {
    let a = &s.parse_ahead;
    let c = a.counts;
    let p = a.power.now();
    json!({
        "enabled": a.enabled,
        "state": a.state,
        "active": a.in_flight.is_some() || !a.queue.is_empty(),
        "queued": a.queue.len(),
        "queue": a.queue.iter().take(50).map(|(why, id)| json!({"id": id.0, "why": why.id()})).collect::<Vec<_>>(),
        "working": a.in_flight.as_ref().map(|f| json!({"id": f.id.0, "why": f.why.id()})),
        "parsed": c.parsed,
        "cached": c.cached,
        "skipped": c.skipped,
        "failed": c.failed,
        "power": {"onBattery": p.on_battery, "percent": p.percent, "lowPower": p.low_power, "minBattery": power::MIN_BATTERY},
        "indexFile": a.index.as_ref().map(|(d, _, _)| d.join(index::FILE).display().to_string()),
    })
}

#[cfg(test)]
impl Session {
    /// Tests: build the queue anew at the next pump (the catalog changed less than [`REBUILD_EVERY`] ago).
    pub(crate) fn parse_ahead_test_rebuild(&mut self) {
        self.parse_ahead.stamp = None;
    }
}
