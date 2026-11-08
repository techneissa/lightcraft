//! Issue #77: what a brush stroke costs the catalog journal. `measure` paints strokes on one photo
//! of a library in memory and prints the bytes written (log and snapshots), the log records, the
//! History's serialized size and the save time per stroke. Run by hand:
//! `cargo test -p lightcraft-engine --lib journal_growth -- --ignored --nocapture`.

use std::sync::{Arc, Mutex};

use lightcraft_catalog::{MemStore, Store};
use serde_json::json;

use crate::Session;
use crate::library::LibraryStores;

/// A [`MemStore`] that counts the bytes written to it.
#[derive(Clone, Default)]
struct Counting {
    files: MemStore,
    log_bytes: Arc<Mutex<u64>>,
    snap_bytes: Arc<Mutex<u64>>,
    snaps: Arc<Mutex<u64>>,
}

impl Store for Counting {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        *self.snap_bytes.lock().unwrap() += data.len() as u64;
        *self.snaps.lock().unwrap() += 1;
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        *self.log_bytes.lock().unwrap() += data.len() as u64;
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "counting".into()
    }
}

/// A 60-point drag (about a second at 60 frames per second), different for each stroke.
fn stroke(i: usize) -> serde_json::Value {
    let y = 0.05 + (i % 90) as f64 * 0.01 + 0.000_123_456_7;
    let pts: Vec<[f64; 2]> = (0..60).map(|k| [0.1 + k as f64 * 0.013_333_37, y + (k as f64 * 0.37).sin() * 0.01]).collect();
    json!({"points": pts, "size": 0.03})
}

fn store_len(store: &Counting, name: &str) -> usize {
    store.files.files.lock().unwrap().get(name).map_or(0, Vec::len)
}

#[test]
#[ignore = "measurement for #77, run by hand"]
fn measure() {
    let store = Counting::default();
    let mut s = Session::new();
    let stores = LibraryStores { dir: "counting".into(), catalog: Box::new(store.clone()), files: Box::new(MemStore::new()), on_disk: false };
    s.open_library_in(stores, true).unwrap();
    let id = s.selection.active.unwrap();
    let (log0, snap0) = (*store.log_bytes.lock().unwrap(), *store.snap_bytes.lock().unwrap());
    let mut times = Vec::new();
    for i in 1..=300 {
        let t = std::time::Instant::now();
        s.execute("mask.brushStroke", &stroke(i)).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        if [1, 30, 100, 200, 300].contains(&i) {
            let p = s.catalog.photo(id).unwrap();
            let history: usize = p.history.iter().map(|h| serde_json::to_string(&h.settings).unwrap().len()).sum();
            let settings = serde_json::to_string(&p.develop).unwrap().len();
            let recent = &times[times.len().saturating_sub(10)..];
            eprintln!(
                "strokes {i:>3}: settings {settings:>8} B | log written {:>11} B, snapshots written {:>11} B ({} snapshots) | catalog.log now {:>10} B, catalog.snap now {:>10} B | History {} steps, {:>11} B | last 10 strokes avg {:.2} ms",
                *store.log_bytes.lock().unwrap() - log0,
                *store.snap_bytes.lock().unwrap() - snap0,
                *store.snaps.lock().unwrap(),
                store_len(&store, "catalog.log"),
                store_len(&store, "catalog.snap"),
                p.history.len(),
                history,
                recent.iter().sum::<f64>() / recent.len() as f64,
            );
        }
    }
    for (cmd, label) in [("edit.undo", "undo"), ("edit.redo", "redo")] {
        let before = *store.log_bytes.lock().unwrap();
        let t = std::time::Instant::now();
        for _ in 0..10 {
            s.execute(cmd, &json!({})).unwrap();
        }
        let ms = t.elapsed().as_secs_f64() * 100.0;
        eprintln!("10 x {label} at 300 strokes: log written {:>11} B, avg {ms:.2} ms each", *store.log_bytes.lock().unwrap() - before);
    }
    let t = std::time::Instant::now();
    s.close_library().unwrap();
    eprintln!("close (snapshot): catalog.snap {:>11} B, {:.0} ms", store_len(&store, "catalog.snap"), t.elapsed().as_secs_f64() * 1000.0);
    let expect = s.catalog.photo(id).unwrap().clone();
    let t = std::time::Instant::now();
    let mut s2 = Session::new();
    let stores = LibraryStores { dir: "counting".into(), catalog: Box::new(store.clone()), files: Box::new(MemStore::new()), on_disk: false };
    s2.open_library_in(stores, false).unwrap();
    eprintln!("reopen: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    assert_eq!(*s2.catalog.photo(id).unwrap().develop, *expect.develop);
    assert_eq!(s2.catalog.photo(id).unwrap().history, expect.history);
}
