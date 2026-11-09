//! Which photos are parsed already, without rendering them: the cache key of each photo's parse, filed by what the
//! parse depends on ([`super::identity`]), kept beside the parse files (`ahead-index.json`). A photo whose identity is
//! listed and whose parse file is still in the cache is skipped. The file is small and rewritten whole (to a temporary
//! file, then moved); a damaged or unknown one is started afresh.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
        if self.keys.insert(key.to_string()) {
            let len = std::fs::metadata(cache::path(dir, key)).map_or(0, |m| m.len());
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

    /// Write the index when it changed and was last written a while ago (`now`: also when it was written just now),
    /// keeping only the photos whose parse is still in `listing`.
    pub fn save(&mut self, listing: &Listing, now: bool) {
        if !self.dirty || (!now && self.saved_at.is_some_and(|t| t.elapsed() < SAVE_EVERY)) {
            return;
        }
        let Some(path) = self.path.clone() else { return };
        self.map.retain(|_, k| listing.keys.contains(k));
        let entries: Vec<Value> = self.map.iter().map(|(id, k)| json!([format!("{id:032x}"), k])).collect();
        let text = json!({"version": VERSION, "entries": entries}).to_string();
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        let written = path.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
            && std::fs::write(&tmp, text).is_ok()
            && std::fs::rename(&tmp, &path).is_ok();
        if !written {
            let _ = std::fs::remove_file(&tmp);
            log::warn!("parse ahead: could not write {}", path.display());
        }
        self.dirty = false;
        self.saved_at = Some(Instant::now());
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
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
