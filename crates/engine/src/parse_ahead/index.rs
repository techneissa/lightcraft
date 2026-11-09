//! Which photos are parsed already, without rendering them: the cache key of each photo's parse, filed by what the
//! parse depends on ([`super::identity`]), kept beside the parse files (`ahead-index.json`). A photo whose identity is
//! listed and whose parse file is still in the cache is skipped. The file is small and rewritten whole (to a temporary
//! file, then moved); a damaged or unknown one is started afresh.
//!
//! The pump and a request read and fill it in memory ([`Shared`]); the parse-ahead worker reads, lists and writes the
//! files ([`load`], [`relist`], [`save`]), so the UI thread never waits for the disk.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_parsing::cache;
use serde_json::{Value, json};

/// The index's file name in the cache folder.
pub const FILE: &str = "ahead-index.json";
const VERSION: u64 = 1;
/// Most photos listed (each entry is about 100 bytes).
const MAX_ENTRIES: usize = 100_000;
/// Largest index file read.
const MAX_FILE: u64 = 32 << 20;
/// How often a changed index is written.
const SAVE_EVERY: Duration = Duration::from_secs(10);
/// Most parses a request files before the index is read.
const MAX_PENDING: usize = 1000;

/// The parse files in the cache folder, by key, and what they take together.
#[derive(Debug, Default)]
pub struct Listing {
    pub keys: HashSet<String>,
    pub bytes: u64,
}

impl Listing {
    /// The parse files in `dir` (none when it can't be read).
    pub fn read(dir: &Path) -> Listing {
        let mut out = Listing::default();
        let Ok(rd) = std::fs::read_dir(dir) else { return out };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != cache::EXT) {
                continue;
            }
            let (Some(stem), Ok(meta)) = (path.file_stem().and_then(|s| s.to_str()), e.metadata()) else { continue };
            out.bytes = out.bytes.saturating_add(meta.len());
            out.keys.insert(stem.to_string());
        }
        out
    }

    /// A parse filed under `key` in `dir` since the listing was read.
    pub fn add(&mut self, dir: &Path, key: &str) {
        if !self.keys.contains(key) {
            let len = std::fs::metadata(cache::path(dir, key)).map_or(0, |m| m.len());
            self.add_len(key, len);
        }
    }

    /// A parse of `len` bytes filed under `key` since the listing was read.
    pub fn add_len(&mut self, key: &str, len: u64) {
        if self.keys.insert(key.to_string()) {
            self.bytes = self.bytes.saturating_add(len);
        }
    }
}

#[derive(Debug, Default)]
pub struct Index {
    path: Option<PathBuf>,
    map: HashMap<u128, String>,
    dirty: bool,
    saved_at: Option<Instant>,
}

impl Index {
    /// The index kept in `dir`; empty when there is none or it is damaged.
    pub fn open(dir: &Path) -> Index {
        let path = dir.join(FILE);
        let map = read(&path).unwrap_or_default();
        Index { path: Some(path), map, dirty: false, saved_at: None }
    }

    /// The key the photo with `identity` was parsed under.
    pub fn get(&self, identity: u128) -> Option<&str> {
        self.map.get(&identity).map(String::as_str)
    }

    pub fn insert(&mut self, identity: u128, key: String) {
        if self.map.get(&identity) != Some(&key) && (self.map.len() < MAX_ENTRIES || self.map.contains_key(&identity)) {
            self.map.insert(identity, key);
            self.dirty = true;
        }
    }

    /// Whether the index changed and was last written a while ago (`now`: also when it was written just now).
    pub fn due(&self, now: bool) -> bool {
        self.dirty && (now || self.saved_at.is_none_or(|t| t.elapsed() >= SAVE_EVERY))
    }

    /// Write the index when it is [`Self::due`], keeping only the photos whose parse is still in `listing`.
    pub fn save(&mut self, listing: &Listing, now: bool) {
        if let Some((path, text)) = self.snapshot(listing, now) {
            write(&path, &text);
        }
    }

    /// What [`Self::save`] writes, and where (taken as written).
    fn snapshot(&mut self, listing: &Listing, now: bool) -> Option<(PathBuf, String)> {
        if !self.due(now) {
            return None;
        }
        let path = self.path.clone()?;
        self.map.retain(|_, k| listing.keys.contains(k));
        let entries: Vec<Value> = self.map.iter().map(|(id, k)| json!([format!("{id:032x}"), k])).collect();
        self.dirty = false;
        self.saved_at = Some(Instant::now());
        Some((path, json!({"version": VERSION, "entries": entries}).to_string()))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

fn write(path: &Path, text: &str) {
    // (the worker and a closing session may write at once: each to its own temporary file)
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{n}.tmp", std::process::id()));
    let written = path.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(&tmp, text).is_ok()
        && std::fs::rename(&tmp, path).is_ok();
    if !written {
        let _ = std::fs::remove_file(&tmp);
        log::warn!("parse ahead: could not write {}", path.display());
    }
}

/// The index and listing of the cache folder in use, shared by the pump (and requests) and the worker.
#[derive(Debug, Default)]
pub struct Shared {
    /// The folder the pump wants the index of.
    pub want: Option<PathBuf>,
    /// The folder `ix` and `listing` are of, once the worker has read them.
    pub dir: Option<PathBuf>,
    pub ix: Index,
    pub listing: Listing,
    /// Parses a request filed before the index was read.
    pending: Vec<(u128, String)>,
    /// The worker was asked to write the index or list the folder and has not yet.
    pub asked: bool,
}

impl Shared {
    /// Whether the index of the folder wanted is read.
    pub fn ready(&self) -> bool {
        self.dir.is_some() && self.dir == self.want
    }

    /// The photo with `identity` was parsed under `key`.
    pub fn file(&mut self, identity: u128, key: String) {
        if self.ready() {
            self.ix.insert(identity, key);
        } else if self.pending.len() < MAX_PENDING {
            self.pending.push((identity, key));
        }
    }
}

/// The index and listing, shared.
pub type Slot = Arc<Mutex<Shared>>;

pub fn lock(slot: &Slot) -> MutexGuard<'_, Shared> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The worker: read the index and listing of `dir`, writing the one of the folder before first. Nothing changes when
/// the pump wants another folder by then.
pub fn load(slot: &Slot, dir: &Path) {
    let old = {
        let mut sh = lock(slot);
        let listing = std::mem::take(&mut sh.listing);
        let old = sh.ix.snapshot(&listing, true);
        sh.dir = None;
        sh.ix = Index::default();
        old
    };
    if let Some((path, text)) = old {
        write(&path, &text);
    }
    let (ix, listing) = (Index::open(dir), Listing::read(dir));
    let mut sh = lock(slot);
    if sh.want.as_deref() != Some(dir) {
        return;
    }
    sh.ix = ix;
    sh.listing = listing;
    sh.dir = Some(dir.to_path_buf());
    for (identity, key) in std::mem::take(&mut sh.pending) {
        sh.ix.insert(identity, key);
    }
}

/// The worker: list the cache folder again (parses come and go: the size limit removes some).
pub fn relist(slot: &Slot) {
    let Some(dir) = lock(slot).dir.clone() else { return };
    let listing = Listing::read(&dir);
    let mut sh = lock(slot);
    if sh.dir.as_deref() == Some(dir.as_path()) {
        sh.listing = listing;
    }
    sh.asked = false;
}

/// The worker: write the index when it is due (`now`: whenever it changed).
pub fn save(slot: &Slot, now: bool) {
    let snap = {
        let mut sh = lock(slot);
        sh.asked = false;
        let Shared { ix, listing, .. } = &mut *sh;
        ix.snapshot(listing, now)
    };
    if let Some((path, text)) = snap {
        write(&path, &text);
    }
}

fn read(path: &Path) -> Option<HashMap<u128, String>> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE {
        return None;
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    if v.get("version")?.as_u64()? != VERSION {
        return None;
    }
    let mut map = HashMap::new();
    for e in v.get("entries")?.as_array()?.iter().take(MAX_ENTRIES) {
        let (Some(id), Some(key)) = (e.get(0).and_then(Value::as_str), e.get(1).and_then(Value::as_str)) else { continue };
        let Ok(id) = u128::from_str_radix(id, 16) else { continue };
        // (a key is a file name: nothing that could leave the folder)
        if key.is_empty() || key.contains(['/', '\\']) || key.starts_with('.') {
            continue;
        }
        map.insert(id, key.to_string());
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-ahead-index-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_index_comes_back_and_forgets_parses_that_left_the_cache() {
        let dir = scratch("roundtrip");
        std::fs::write(cache::path(&dir, "kept"), b"x").unwrap();
        let mut ix = Index::open(&dir);
        ix.insert(1, "kept".into());
        ix.insert(2, "gone".into());
        ix.save(&Listing::read(&dir), true);
        let back = Index::open(&dir);
        assert_eq!(back.get(1), Some("kept"));
        assert_eq!(back.get(2), None);
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn a_damaged_or_hostile_index_is_started_afresh() {
        let dir = scratch("damaged");
        std::fs::write(dir.join(FILE), b"{not json").unwrap();
        assert_eq!(Index::open(&dir).len(), 0);
        std::fs::write(dir.join(FILE), json!({"version": 99, "entries": [["1", "k"]]}).to_string()).unwrap();
        assert_eq!(Index::open(&dir).len(), 0);
        let hostile = json!({"version": 1, "entries": [["zz", "k"], ["1", "../escape"], ["2", ".hidden"], ["3"], 7, ["4", "ok"]]});
        std::fs::write(dir.join(FILE), hostile.to_string()).unwrap();
        let ix = Index::open(&dir);
        assert_eq!(ix.len(), 1);
        assert_eq!(ix.get(4), Some("ok"));
    }

    #[test]
    fn the_index_is_read_for_the_folder_wanted_and_keeps_what_a_request_filed_meanwhile() {
        let dir = scratch("shared");
        std::fs::write(cache::path(&dir, "early"), b"x").unwrap();
        let slot = Slot::default();
        lock(&slot).want = Some(dir.clone());
        // a request files a parse before the worker has read the index
        lock(&slot).file(7, "early".into());
        assert!(!lock(&slot).ready());
        load(&slot, &dir);
        assert!(lock(&slot).ready());
        assert_eq!(lock(&slot).ix.get(7), Some("early"));
        save(&slot, true);
        assert_eq!(Index::open(&dir).get(7), Some("early"));
        // another folder wanted by the time it is read: nothing is taken for it
        let other = scratch("shared-other");
        lock(&slot).want = Some(other);
        load(&slot, &dir);
        assert!(!lock(&slot).ready());
    }

    #[test]
    fn the_listing_counts_only_parse_files() {
        let dir = scratch("listing");
        std::fs::write(cache::path(&dir, "a"), [0u8; 10]).unwrap();
        std::fs::write(dir.join(FILE), b"{}").unwrap();
        let mut l = Listing::read(&dir);
        assert_eq!((l.keys.len(), l.bytes), (1, 10));
        std::fs::write(cache::path(&dir, "b"), [0u8; 5]).unwrap();
        l.add(&dir, "b");
        l.add(&dir, "b");
        assert_eq!((l.keys.len(), l.bytes), (2, 15));
    }
}
