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
//! photo already parsed for the current model set and People edge is skipped without being rendered: an index beside
//! the cache ([`index`]) files each parse's key by what the parse depends on ([`identity`]). The worker reads and
//! writes that index; the pump queues nothing until it is read.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lightcraft_catalog::{Aspects, Photo, PhotoId};
use lightcraft_meta::RegionKind;
use lightcraft_preview::Hasher128;
use serde_json::{Value, json};

use crate::Session;
use crate::activity::{Cancel, TaskGuard};
use crate::people_parsing::{Filed, PARSE_EDGE, QuietRun};

pub(crate) mod index;
pub(crate) mod peek;
pub mod power;
pub(crate) mod queue;
mod worker;

use index::Slot;
use queue::Why;

/// The share of the cache's size limit the idle sweep fills at most.
pub const IDLE_FILL: f64 = 0.9;
/// How often the queue is rebuilt after the catalog changed (the photos around the open one: at once).
const REBUILD_EVERY: Duration = Duration::from_secs(3);
/// What of a photo the queue reads: its faces and the rest of its metadata, its dates, its file, whether it is in the
/// library. Other changes (edits, ratings) leave the queue as it is.
const READS: Aspects = Aspects::META.with(Aspects::CAPTURED).with(Aspects::FILE).with(Aspects::PRESENCE);

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
    /// The installed YuNet's file ([`yunet`]).
    yunet: u64,
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
    index: Slot,
    row: Option<TaskGuard>,
    row_cancel: Arc<AtomicBool>,
    /// Photos done since the row appeared.
    row_done: u64,
    /// ✕ was pressed while this photo was open: nothing until another one is.
    stopped_at: Option<Option<PhotoId>>,
    counts: Counts,
    state: &'static str,
    /// Photos queued that the pace allows now.
    left: usize,
    /// Times the queue was rebuilt from the catalog.
    rebuilds: u64,
    /// The early check for faces (`peek`).
    pub(crate) peek: peek::Peek,
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
            index: Slot::default(),
            row: None,
            row_cancel: Arc::new(AtomicBool::new(false)),
            row_done: 0,
            stopped_at: None,
            counts: Counts::default(),
            state: "idle",
            left: 0,
            rebuilds: 0,
            peek: peek::Peek::default(),
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
        // (the app is closing: written here, the worker may not get to it)
        let mut sh = index::lock(&self.index);
        let index::Shared { ix, listing, .. } = &mut *sh;
        ix.save(listing, true);
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

    /// Times the queue was rebuilt from the catalog.
    pub fn rebuilds(&self) -> u64 {
        self.rebuilds
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
pub(crate) fn identity(p: &Photo, yunet: u64, version: &str) -> u128 {
    let mut h = Hasher128::new();
    h.str("parse-ahead/1")
        .str(&crate::media::content_key(p))
        .u64(crate::media::RENDER_CACHE_VERSION)
        .u64(PARSE_EDGE as u64)
        .str(version)
        .u64(yunet)
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

/// A stamp of the installed YuNet's file, read from the file system only (0: none): whether faces come from YuNet.
fn yunet(s: &Session) -> u64 {
    crate::cmd::face_detect::detector_file(s).map_or(0, |(_, stamp)| stamp)
}

/// Whether the face scan has looked at photo `id` (the web build has no face scan).
fn face_scanned(s: &Session, id: PhotoId) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return s.faces.scanned.contains(id.0);
    #[cfg(target_arch = "wasm32")]
    return {
        let _ = (s, id);
        false
    };
}

/// The photos around the open one worth parsing: those with faces, and those the face scan has not looked at yet.
fn near_photos(s: &mut Session, backward: bool) -> Vec<PhotoId> {
    let Some(at) = s.active() else { return Vec::new() };
    queue::near(s.visible(), at, backward)
        .into_iter()
        .filter(|id| s.catalog.photo(*id).is_some_and(|p| parseable(p) && (has_faces(p) || !face_scanned(s, *id))))
        .collect()
}

/// Rebuild the photos after the open one's neighbours: new imports with faces and (while the cache has room) every
/// other photo with faces, minus those whose parse is in the cache. Hashes each photo with faces: done when what the
/// queue reads of the catalog changed, not when the user steps to another photo. The worker then lists the cache
/// folder again, for the next rebuild.
fn rebuild_rest(s: &mut Session, stamp: &Stamp) {
    if s.parse_ahead.known.as_ref().is_none_or(|(lib, _)| *lib != stamp.library) {
        let ids = s.catalog.photos().map(|p| p.id).collect();
        s.parse_ahead.known = Some((stamp.library, ids));
    }
    let room = {
        let mut sh = index::lock(&s.parse_ahead.index);
        let room = (sh.listing.bytes as f64) < crate::people_parsing::cache_budget() as f64 * IDLE_FILL;
        let ask = !std::mem::replace(&mut sh.asked, true);
        drop(sh);
        if ask && !send(&mut s.parse_ahead, worker::Job::Relist) {
            index::lock(&s.parse_ahead.index).asked = false;
        }
        room
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
    s.parse_ahead.rebuilds += 1;
}

/// Whether the photo with `ident` needs no parse: done since the app started, or its parse is in the cache (then it
/// counts as skipped, once).
fn skip(ahead: &mut ParseAhead, ident: u128) -> bool {
    if ahead.tried.contains(&ident) {
        return true;
    }
    let cached = {
        let sh = index::lock(&ahead.index);
        sh.ix.get(ident).is_some_and(|k| sh.listing.keys.contains(k))
    };
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
            worker::Ended::Parsed { key, bytes, cached } => {
                ahead.tried.insert(f.identity);
                if cached {
                    ahead.counts.cached += 1;
                } else {
                    ahead.counts.parsed += 1;
                }
                ahead.row_done += 1;
                let Some(key) = key else { continue };
                let mut sh = index::lock(&ahead.index);
                sh.listing.add_len(&key, bytes);
                sh.file(f.identity, key);
                // the cache filled up: the idle sweep ends here
                if (sh.listing.bytes as f64) >= crate::people_parsing::cache_budget() as f64 * IDLE_FILL {
                    drop(sh);
                    ahead.queue.retain(|(why, _)| *why != Why::Idle);
                    ahead.rest.retain(|(why, _, _)| *why != Why::Idle);
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
    if let Some(why) = s.parse_ahead.power.holds_back() {
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

/// How the catalog changed since the queue was built.
#[derive(PartialEq, Eq)]
enum Change {
    None,
    /// Only what the queue does not read (edits, ratings).
    Unread,
    /// What the queue reads: rebuilt when [`REBUILD_EVERY`] has passed.
    Read,
    /// The library, the photo count, the model version, YuNet or the cache folder: rebuilt at once.
    All,
}

fn change(s: &Session, stamp: &Stamp) -> Change {
    let Some(old) = &s.parse_ahead.stamp else { return Change::All };
    if old.library != stamp.library
        || old.photos != stamp.photos
        || old.version != stamp.version
        || old.yunet != stamp.yunet
        || old.cache != stamp.cache
    {
        return Change::All;
    }
    if old.revision == stamp.revision {
        return Change::None;
    }
    match s.catalog.aspects_since(old.revision) {
        Some(a) if !a.any_of(READS) => Change::Unread,
        _ => Change::Read,
    }
}

/// Whether the work is held back now (nothing new starts), before the power and the setting: the pace, the photo on
/// screen being made, another parse (a request), an export.
fn holding(s: &Session, pace: Pace, mine: Option<&str>) -> bool {
    pace == Pace::Pause || crate::prep::foreground_busy() || s.parsing.others_running(mine) || worker::exporting(&s.activity)
}

/// One pump: take in finished photos, follow the pace, rebuild the queue when what it was built from changed, and
/// hand the worker the next photo. Cheap when there is nothing to do.
pub(crate) fn pump(s: &mut Session, pace: Pace, backward: bool) {
    collect(s);
    // (the Portrait icon's early check for faces: it needs no parse models, so it goes before the work below stands still)
    peek::pump(s, pace);
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
    // the index lives beside the parses; nothing is queued until the worker has read it
    if !open_index(s) {
        s.parse_ahead.state = "loading";
        return;
    }
    let mine = s.parse_ahead.in_flight.as_ref().and_then(|f| f.run.key());
    let held = holding(s, pace, mine.as_deref());
    s.parse_ahead.hold.store(held, Ordering::SeqCst);
    // the model run gives way, unless a request waits for it (also one about to start: the stop waits for it)
    if held
        && let Some(f) = &s.parse_ahead.in_flight
        && !mine.as_deref().is_some_and(|k| s.parsing.waited_on(k))
    {
        f.run.stop();
    }
    let near = near_photos(s, backward);
    let stamp = Stamp {
        library: Arc::as_ptr(&s.library_identity) as usize,
        photos: s.catalog.len(),
        revision: s.catalog.revision,
        version: s.parsing.ahead_version(),
        yunet: yunet(s),
        cache: s.parsing.cache_dir(),
    };
    let rebuild = match change(s, &stamp) {
        Change::All => true,
        Change::Read => s.parse_ahead.built_at.is_none_or(|t| t.elapsed() >= REBUILD_EVERY),
        Change::Unread => {
            // (measured from here next time)
            if let Some(old) = s.parse_ahead.stamp.as_mut() {
                old.revision = stamp.revision;
            }
            false
        }
        Change::None => false,
    };
    // (never while held: the user is working)
    if rebuild && !held {
        rebuild_rest(s, &stamp);
        requeue(s, near, &stamp);
        s.parse_ahead.stamp = Some(stamp);
    } else if near != s.parse_ahead.near {
        // (the stamp stays: a catalog change waiting for its rebuild is still seen)
        requeue(s, near, &stamp);
    }
    if !held && s.parse_ahead.in_flight.is_none() {
        submit_next(s, pace);
    }
    ask_save(&mut s.parse_ahead);
    show(s, pace, held);
}

/// The state and the row after a pump.
fn show(s: &mut Session, pace: Pace, held: bool) {
    let ahead = &mut s.parse_ahead;
    let left = ahead.queue.iter().filter(|(why, _)| pace == Pace::Pause || pace.allows(*why)).count();
    ahead.left = left;
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
    let row = ahead
        .row
        .get_or_insert_with(|| s.activity.start_background("parseAhead", "Finding people parts ahead", Cancel::Flag(ahead.row_cancel.clone())));
    let total = ahead.row_done + left as u64 + u64::from(ahead.in_flight.is_some());
    row.progress(ahead.row_done, total);
}

/// Have the worker write the index when it changed a while ago.
fn ask_save(ahead: &mut ParseAhead) {
    let ask = {
        let mut sh = index::lock(&ahead.index);
        let ask = !sh.asked && sh.ix.due(false);
        sh.asked |= ask;
        ask
    };
    if ask && !send(ahead, worker::Job::Save) {
        index::lock(&ahead.index).asked = false;
    }
}

/// The worker, started when there is none.
fn ensure_worker(s: &mut Session) -> Option<&worker::Worker> {
    let ahead = &mut s.parse_ahead;
    if ahead.worker.is_none() {
        let holds = worker::Holds { hold: ahead.hold.clone(), parsing: s.parsing.clone(), activity: s.activity.clone() };
        match worker::Worker::start(holds, ahead.index.clone()) {
            Ok(w) => ahead.worker = Some(w),
            Err(e) => log::warn!("parse ahead: {e}"),
        }
    }
    ahead.worker.as_ref()
}

/// Hand the worker `job` (started already); `false` when its thread has gone (a new one next time).
fn send(ahead: &mut ParseAhead, job: worker::Job) -> bool {
    let sent = ahead.worker.as_ref().is_some_and(|w| w.send(job));
    if !sent {
        ahead.worker = None;
    }
    sent
}

/// Hand the worker the first photo the pace allows.
fn submit_next(s: &mut Session, pace: Pace) {
    let yunet = yunet(s);
    let version = s.parsing.ahead_version();
    while let Some(pos) = s.parse_ahead.queue.iter().position(|(why, _)| pace.allows(*why)) {
        let Some((why, id)) = s.parse_ahead.queue.remove(pos) else { return };
        let Some(photo) = s.catalog.photo(id).cloned() else { continue };
        let ident = identity(&photo, yunet, &version);
        // (the detector is read by the worker, never here)
        let prepare = match crate::cmd::parsing::prepare_ahead(s, id) {
            Ok(p) => p,
            Err(e) => {
                log::info!("parse ahead: photo {}: {e}", id.0);
                s.parse_ahead.tried.insert(ident);
                continue;
            }
        };
        if ensure_worker(s).is_none() {
            s.parse_ahead.queue.push_front((why, id));
            return;
        }
        let ahead = &mut s.parse_ahead;
        let cancel = Arc::new(AtomicBool::new(false));
        let run = QuietRun::default();
        let task = worker::Task { id, identity: ident, version: version.clone(), prepare, cancel: cancel.clone(), run: run.clone() };
        if send(ahead, worker::Job::Parse(Box::new(task))) {
            ahead.in_flight = Some(InFlight { id, why, cancel, run });
        } else {
            ahead.queue.push_front((why, id));
        }
        return;
    }
}

/// Have the worker read the index beside the cache in use (writing the one of another folder first); `true` once it
/// is read.
fn open_index(s: &mut Session) -> bool {
    let cache = s.parsing.cache_dir();
    let asked = {
        let mut sh = index::lock(&s.parse_ahead.index);
        if sh.want == cache {
            return sh.ready();
        }
        sh.want = cache.clone();
        cache
    };
    let Some(dir) = asked else { return false };
    if ensure_worker(s).is_none() || !send(&mut s.parse_ahead, worker::Job::Load(dir.clone())) {
        // (no thread to give it to: read here)
        index::load(&s.parse_ahead.index, &dir);
    }
    index::lock(&s.parse_ahead.index).ready()
}

/// The parse of photo `id`, when one was filed for it as it is now (by parsing ahead or an earlier request): read from
/// the cache without rendering the photo. `None`: not known (or made under another model set or People edge); parse
/// as usual.
pub(crate) fn ready(s: &mut Session, id: PhotoId) -> Option<Arc<lightcraft_parsing::Parsing>> {
    let version = s.parsing.ahead_version();
    let parsing = s.parsing.clone();
    known(s, id, &version)?.read(&parsing)
}

/// A request parsed photo `id` as `filed`: the next request finds it without a render.
pub(crate) fn remember(s: &mut Session, id: PhotoId, filed: Filed) {
    if let Some(k) = known(s, id, &filed.version) {
        k.file(filed);
    }
}

/// The longest a worker waits for the index to be read ([`Known::read_when_loaded`]).
#[cfg(not(target_arch = "wasm32"))]
const INDEX_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// What the index knows of one photo's parse under one version, taken on the calling thread so a worker can read the
/// parse ([`Known::read`]) and file the one it makes ([`Known::file`]) without the session: portrait retouching's
/// face search (`crate::retouch_input`) asks for its parse this way, as [`ready`] and [`remember`] do.
pub(crate) struct Known {
    slot: Slot,
    identity: u128,
    version: String,
    /// The cache key the photo's parse is filed under (`None`: not known, or the index is not read yet).
    key: Option<String>,
}

/// [`Known`] for photo `id` parsed under `version` (`None`: no such photo). Reads nothing but the index in memory;
/// asks the worker to read the index when it is not read yet.
pub(crate) fn known(s: &mut Session, id: PhotoId, version: &str) -> Option<Known> {
    let open = open_index(s);
    let identity = identity(s.catalog.photo(id)?, yunet(s), version);
    let key = if open { index::lock(&s.parse_ahead.index).ix.get(identity).map(str::to_string) } else { None };
    Some(Known { slot: s.parse_ahead.index.clone(), identity, version: version.to_string(), key })
}

impl Known {
    /// The parse filed for the photo, from memory or disk (no run). `None`: not known, gone from the cache, or made
    /// under another version.
    pub(crate) fn read(&self, parsing: &crate::people_parsing::PeopleParsing) -> Option<Arc<lightcraft_parsing::Parsing>> {
        // (a parse made under a version this one does not name is not taken for it; one of nobody has no edges to draw)
        parsing.cached(self.key.as_deref()?).filter(|p| p.model_version == self.version || p.people.is_empty())
    }

    /// [`Self::read`] on a worker that may not wait for the UI thread: when the index was not read yet at planning
    /// time (the worker was still reading it), waits for it, up to [`INDEX_WAIT`], and looks the photo up then, so a
    /// photo parsed before is never run again because a run started first. Stops waiting when `cancel` is set, when no
    /// cache folder is wanted, or when the folder wanted changed.
    pub(crate) fn read_when_loaded(
        &self,
        parsing: &crate::people_parsing::PeopleParsing,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Option<Arc<lightcraft_parsing::Parsing>> {
        let key = match &self.key {
            Some(k) => Some(k.clone()),
            None => self.key_when_loaded(cancel),
        }?;
        parsing.cached(&key).filter(|p| p.model_version == self.version || p.people.is_empty())
    }

    #[cfg(target_arch = "wasm32")]
    fn key_when_loaded(&self, _: &std::sync::atomic::AtomicBool) -> Option<String> {
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn key_when_loaded(&self, cancel: &std::sync::atomic::AtomicBool) -> Option<String> {
        // (a guard against a worker that never answers, not a timing the result depends on)
        let give_up = std::time::Instant::now() + INDEX_WAIT;
        loop {
            {
                let sh = index::lock(&self.slot);
                if sh.ready() {
                    return sh.ix.get(self.identity).map(str::to_string);
                }
                sh.want.as_ref()?;
            }
            if cancel.load(std::sync::atomic::Ordering::Relaxed) || std::time::Instant::now() >= give_up {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The photo was parsed as `filed`: the next request finds it without a render. A parse filed under another
    /// version than this one's identity names is left out (its identity would be another).
    pub(crate) fn file(&self, filed: Filed) {
        if filed.version != self.version {
            return;
        }
        let mut sh = index::lock(&self.slot);
        if let Some(dir) = sh.dir.clone() {
            sh.listing.add(&dir, &filed.key);
        }
        sh.file(self.identity, filed.key);
    }
}

impl Session {
    /// Tests and tools: use `power` instead of the Mac's from now on (`None`: the Mac's again).
    #[doc(hidden)]
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn parse_ahead_fix_power(&mut self, power: Option<power::Power>) {
        self.parse_ahead.power.fix(power);
    }

    /// Tests and tools: how many early checks for faces (`peek`) have started.
    #[doc(hidden)]
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn parse_ahead_peeks(&self) -> usize {
        self.parse_ahead.peek.started()
    }

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
        "left": a.left,
        "power": {"onBattery": p.on_battery, "percent": p.percent, "lowPower": p.low_power, "minBattery": power::MIN_BATTERY},
        "indexFile": index::lock(&a.index).dir.as_ref().map(|d| d.join(index::FILE).display().to_string()),
    })
}

#[cfg(test)]
impl Session {
    /// Tests: build the queue anew at the next pump (the catalog changed less than [`REBUILD_EVERY`] ago).
    pub(crate) fn parse_ahead_test_rebuild(&mut self) {
        self.parse_ahead.stamp = None;
    }

    /// Tests: whether the index beside the cache is read (asking the worker to read it).
    pub(crate) fn parse_ahead_test_index_read(&mut self) -> bool {
        open_index(self)
    }

    /// Tests: the queue was built long ago (a change it reads rebuilds it at the next pump).
    pub(crate) fn parse_ahead_test_age(&mut self) {
        self.parse_ahead.built_at = None;
    }

    /// Tests: the model run going, if any.
    pub(crate) fn parse_ahead_test_run(&self) -> Option<QuietRun> {
        self.parse_ahead.in_flight.as_ref().map(|f| f.run.clone())
    }
}
