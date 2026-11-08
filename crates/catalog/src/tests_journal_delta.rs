//! Issue #77: develop edits and History are logged as patches (format 14). Replaying them gives
//! the same catalog as the whole records of format 13 did; a bad patch is an error the library
//! recovers from; a brush stroke costs the log about the stroke.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;

use crate::journal::{LOG, SNAPSHOT, encode_record};
use crate::*;

fn add_photo(c: &mut Catalog) -> (PhotoId, Op) {
    let id = c.alloc_photo_id();
    (id, Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: format!("/p/{}.jpg", id.0) }, "p.jpg", "JPEG", 60, 40, "2026-01-01")) })
}

/// A 60-point drag (about a second at 60 frames per second); `points` 4 for sessions whose
/// whole-record (format 13) replay would otherwise take long.
fn stroke_of(i: usize, points: usize) -> BrushStroke {
    let y = 0.05 + (i % 90) as f64 * 0.01 + 0.000_123_456_7;
    BrushStroke {
        points: (0..points).map(|k| Point::new(0.1 + k as f64 * 0.013_333_37, y + (k as f64 * 0.37).sin() * 0.01)).collect(),
        ..Default::default()
    }
}

fn stroke(i: usize) -> BrushStroke {
    stroke_of(i, 60)
}

/// `d` with one more stroke in its first brush mask (made when there is none).
fn painted(d: &DevelopSettings, i: usize) -> DevelopSettings {
    painted_with(d, stroke(i))
}

fn painted_with(d: &DevelopSettings, s: BrushStroke) -> DevelopSettings {
    let mut d = d.clone();
    if d.masks.is_empty() {
        d.masks.push(Mask {
            id: 1,
            name: "Brush 1".into(),
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes: vec![] } }],
            ..Default::default()
        });
    }
    if let Some(MaskShape::Brush { strokes }) = d.masks.first_mut().and_then(|m| m.components.first_mut()).map(|c| &mut c.shape) {
        strokes.push(s);
    }
    d
}

/// The op an edit commits (as `Session::develop_op` builds it): one `Arc` for both.
fn edit(id: PhotoId, d: DevelopSettings, label: &str) -> Op {
    let settings = Arc::new(d);
    Op::Batch {
        ops: vec![
            Op::SetDevelop { id, settings: settings.clone(), label: label.into(), edited: Some("2026-11-08T10:00:00".into()) },
            Op::PushHistory { id, step: HistoryStep { label: label.into(), settings } },
        ],
    }
}

/// An edit session on a live catalog, as the engine runs one: each op applied, then logged;
/// undo and redo log the inverse ops.
struct Live {
    c: Catalog,
    ops: Vec<Op>,
    undo: Vec<Op>,
    redo: Vec<Op>,
}

impl Live {
    fn run(&mut self, op: Op) {
        let inv = self.c.apply(op.clone()).unwrap();
        self.ops.push(op);
        self.undo.push(inv);
        self.redo.clear();
    }
    fn undo(&mut self) {
        let op = self.undo.pop().unwrap();
        let inv = self.c.apply(op.clone()).unwrap();
        self.ops.push(op);
        self.redo.push(inv);
    }
    fn redo(&mut self) {
        let op = self.redo.pop().unwrap();
        let inv = self.c.apply(op.clone()).unwrap();
        self.ops.push(op);
        self.undo.push(inv);
    }
}

/// A session on two photos: strokes past the History limit, slider edits, undo and redo, a
/// History cursor and an edit made from it, a rating, a photo removed.
fn session(strokes: usize) -> (Catalog, Live) {
    let mut c = Catalog::new();
    let (a, add_a) = add_photo(&mut c);
    c.apply(add_a).unwrap();
    let base = c.clone();
    let mut live = Live { c, ops: vec![], undo: vec![], redo: vec![] };
    let (b, add_b) = add_photo(&mut live.c);
    live.run(add_b);
    for i in 0..strokes {
        let d = painted_with(&live.c.photo(a).unwrap().develop, stroke_of(i, 4));
        live.run(edit(a, d, "Brush"));
        if i % 7 == 3 {
            let mut d = (*live.c.photo(b).unwrap().develop).clone();
            d.light.exposure += 0.1;
            live.run(edit(b, d, "Exposure"));
        }
        if i % 41 == 5 {
            live.undo();
            live.undo();
            live.redo();
        }
    }
    let mut d = (*live.c.photo(a).unwrap().develop).clone();
    d.light.contrast = 12.0;
    if let Some(m) = d.masks.first_mut() {
        m.adjust.exposure = 0.5;
    }
    live.run(edit(a, d, "Contrast"));
    live.run(Op::SetRating { id: a, rating: 3 });
    // Go to an earlier History step, then edit from there (drops the steps after it)
    let steps = live.c.photo(a).unwrap().history.len();
    let at = steps.saturating_sub(5);
    let earlier = live.c.photo(a).unwrap().history[at].settings.clone();
    live.run(Op::Batch {
        ops: vec![
            Op::SetDevelop { id: a, settings: earlier, label: "History".into(), edited: None },
            Op::SetHistoryCursor { id: a, cursor: Some(at) },
        ],
    });
    let d = painted(&live.c.photo(a).unwrap().develop, 999);
    live.run(edit(a, d, "Brush"));
    live.undo();
    live.run(Op::RemovePhoto { id: b });
    live.undo();
    (base, live)
}

fn open(m: &MemStore) -> (Journal, Catalog, LoadReport) {
    Journal::open(Box::new(m.clone())).unwrap()
}

fn snap_of(c: &Catalog, version: u32) -> Vec<u8> {
    format!("{{\"format\":\"lightcraft-catalog\",\"version\":{version},\"seq\":1,\"catalog\":{}}}\n", c.to_snapshot()).into_bytes()
}

#[test]
fn replaying_patches_gives_what_whole_records_gave() {
    let (base, live) = session(230);
    // format 14: patches
    let m = MemStore::new();
    m.set(SNAPSHOT, snap_of(&base, journal::VERSION));
    let (mut j, _, _) = open(&m);
    j.policy = SnapshotPolicy { max_records: u64::MAX, max_bytes: u64::MAX };
    for op in &live.ops {
        j.append(std::slice::from_ref(op)).unwrap();
    }
    drop(j);
    let log = String::from_utf8(m.get(LOG).unwrap()).unwrap();
    assert!(log.contains("\"op\":\"patchBatch\"") && log.contains("\"op\":\"patchHistory\""), "written as patches");
    let (_, new, r) = open(&m);
    assert_eq!((r.failed, r.replayed), (0, live.ops.len()));
    // format 13: the same ops, whole
    let old = MemStore::new();
    old.set(SNAPSHOT, snap_of(&base, 13));
    let lines: String = live.ops.iter().enumerate().map(|(i, op)| format!("{}\n", encode_record(i as u64 + 2, op))).collect();
    old.set(LOG, lines.into_bytes());
    let (_, was, r) = open(&old);
    assert_eq!((r.failed, r.upgraded_from), (0, Some(13)));

    assert_eq!(new.to_snapshot(), was.to_snapshot());
    assert_eq!(new.to_snapshot(), live.c.to_snapshot());
    for p in live.c.photos() {
        assert_eq!(**new.photo(p.id).unwrap(), **p, "photo {}", p.id.0);
    }
    let a = live.c.photos().next().unwrap().id;
    assert_eq!(new.photo(a).unwrap().history.len(), HISTORY_LIMIT, "past the limit");
}

/// Ops appended in batches of several (the engine's `persist` after a few commands), and after
/// a reopen (the shadow then starts from the loaded catalog).
#[test]
fn patches_appended_in_batches_and_across_reopens_replay_right() {
    let (base, live) = session(60);
    let m = MemStore::new();
    m.set(SNAPSHOT, snap_of(&base, journal::VERSION));
    for chunk in live.ops.chunks(17) {
        let (mut j, _, _) = open(&m);
        for ops in chunk.chunks(3) {
            j.append(ops).unwrap();
        }
    }
    let (_, c, r) = open(&m);
    assert_eq!(r.failed, 0);
    assert_eq!(c.to_snapshot(), live.c.to_snapshot());
}

/// Compaction mid-session: the snapshot holds the state, later records patch it.
#[test]
fn patches_after_a_snapshot_replay_right() {
    let (base, live) = session(40);
    let m = MemStore::new();
    m.set(SNAPSHOT, snap_of(&base, journal::VERSION));
    let (mut j, _, _) = open(&m);
    let mut c = base.clone();
    for (i, op) in live.ops.iter().enumerate() {
        c.apply(op.clone()).unwrap();
        j.append(std::slice::from_ref(op)).unwrap();
        if i % 25 == 24 {
            j.snapshot(&c).unwrap();
        }
    }
    drop(j);
    let (_, back, r) = open(&m);
    assert_eq!(r.failed, 0);
    assert_eq!(back.to_snapshot(), live.c.to_snapshot());
}

#[test]
fn a_stroke_costs_the_log_about_the_stroke() {
    let mut c = Catalog::new();
    let (id, add) = add_photo(&mut c);
    c.apply(add.clone()).unwrap();
    let m = MemStore::new();
    let (mut j, _, _) = open(&m);
    j.policy = SnapshotPolicy { max_records: u64::MAX, max_bytes: u64::MAX };
    j.append(&[add]).unwrap();
    let mut sizes = Vec::new();
    for i in 0..300 {
        let op = edit(id, painted(&c.photo(id).unwrap().develop, i), "Brush");
        let inv = c.apply(op.clone()).unwrap();
        let before = j.log_bytes();
        j.append(&[op]).unwrap();
        sizes.push(j.log_bytes() - before);
        if i == 299 {
            // an undo: the History minus its last step, and the settings before the stroke
            c.apply(inv.clone()).unwrap();
            let before = j.log_bytes();
            j.append(&[inv]).unwrap();
            let undo = j.log_bytes() - before;
            assert!(undo < 20_000, "an undo at 300 strokes wrote {undo} B");
        }
    }
    let one = serde_json::to_string(&stroke(0)).unwrap().len() as u64;
    // the 300th stroke costs what the 2nd did, and about one stroke (plus label, path and step)
    assert!(sizes[299] < one + 1500, "stroke 300 wrote {} B, a stroke is {one} B", sizes[299]);
    assert!(sizes[299] <= sizes[1] + 64, "{} vs {}", sizes[299], sizes[1]);
    drop(j);
    let (_, back, r) = open(&m);
    assert_eq!(r.failed, 0);
    assert_eq!(back.to_snapshot(), c.to_snapshot());
}

/// The CRC-valid log line for `body` (a hand-made record).
fn line(seq: u64, body: &str) -> String {
    format!("{{\"seq\":{seq},\"crc\":{},\"op\":{body}}}\n", crc32fast::hash(body.as_bytes()))
}

/// A patch that doesn't fit the photo (made from other settings) is refused, reported, and the
/// photo's later patches with it; other photos and other ops still replay. Never a panic.
#[test]
fn a_patch_that_does_not_fit_is_reported_and_the_rest_replays() {
    let mut c = Catalog::new();
    let (a, add_a) = add_photo(&mut c);
    let (b, add_b) = add_photo(&mut c);
    c.apply(add_a).unwrap();
    c.apply(add_b).unwrap();
    let m = MemStore::new();
    m.set(SNAPSHOT, snap_of(&c, journal::VERSION));
    let bad = r#"{"op":"patchDevelop","id":1,"patch":{"o":{"s":{"masks":{"a":{"n":7,"h":7,"t":0,"v":[]}}}}},"label":"Brush","edited":null}"#;
    let later = r#"{"op":"patchDevelop","id":1,"patch":{"o":{"s":{"light":{"o":{"s":{"exposure":{"v":1.0}}}}}}},"label":"Exposure","edited":null}"#;
    let other = format!(
        r#"{{"op":"patchDevelop","id":{},"patch":{{"o":{{"s":{{"light":{{"o":{{"s":{{"exposure":{{"v":2.0}}}}}}}}}}}}}},"label":"Exposure","edited":null}}"#,
        b.0
    );
    let hostile = r#"{"op":"patchHistory","id":2,"steps":[{"keep":{"from":18446744073709551615,"len":2}}]}"#;
    let push = r#"{"op":"patchPush","id":2,"label":"X","base":"develop"}"#;
    let log =
        [line(2, bad), line(3, later), line(4, &other), line(5, hostile), line(6, push), line(7, r#"{"op":"setRating","id":1,"rating":2}"#)].concat();
    assert_eq!(a.0, 1);
    m.set(LOG, log.into_bytes());
    let (_, back, r) = open(&m);
    assert_eq!((r.replayed, r.failed), (2, 4), "{r:?}");
    assert_eq!(back.photo(a).unwrap().develop.light.exposure, 0.0, "a's later patch is refused too");
    assert_eq!(back.photo(a).unwrap().rating, 2);
    assert_eq!(back.photo(b).unwrap().develop.light.exposure, 2.0);
    assert!(back.photo(b).unwrap().history.is_empty());
}

/// A patch record cut off by a crash mid-append is a torn tail: dropped, the rest kept.
#[test]
fn a_torn_patch_record_is_dropped() {
    let mut c = Catalog::new();
    let (id, add) = add_photo(&mut c);
    c.apply(add.clone()).unwrap();
    let m = MemStore::new();
    let (mut j, _, _) = open(&m);
    j.append(&[add]).unwrap();
    let mut ops = Vec::new();
    for i in 0..3 {
        let op = edit(id, painted(&c.photo(id).unwrap().develop, i), "Brush");
        c.apply(op.clone()).unwrap();
        ops.push(op);
    }
    j.append(&ops[..2]).unwrap();
    let whole = j.log_bytes();
    j.append(&ops[2..]).unwrap();
    drop(j);
    let mut log = m.get(LOG).unwrap();
    let cut = (whole as usize + log.len()) / 2;
    log.truncate(cut);
    m.set(LOG, log);
    let (_, back, r) = open(&m);
    assert!(r.torn_bytes > 0 && r.failed == 0, "{r:?}");
    let MaskShape::Brush { strokes } = &back.photo(id).unwrap().develop.masks[0].components[0].shape else { panic!() };
    assert_eq!(strokes.len(), 2);
    assert_eq!(back.photo(id).unwrap().history.len(), 2);
}

/// A store whose appends fail while `fail` is set.
#[derive(Clone, Default)]
struct Flaky {
    files: MemStore,
    fail: Arc<AtomicBool>,
}

impl Store for Flaky {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "flaky".into()
    }
}

/// A failed append leaves the shadow where it was: the retry patches against the same state.
#[test]
fn a_failed_append_is_retried_against_the_same_state() {
    let mut c = Catalog::new();
    let (id, add) = add_photo(&mut c);
    c.apply(add.clone()).unwrap();
    let store = Flaky::default();
    let (mut j, _, _) = Journal::open(Box::new(store.clone())).unwrap();
    j.append(&[add]).unwrap();
    let mut queued = Vec::new();
    for i in 0..4 {
        let op = edit(id, painted(&c.photo(id).unwrap().develop, i), "Brush");
        c.apply(op.clone()).unwrap();
        queued.push(op);
        if i == 1 {
            j.append(&queued).unwrap();
            queued.clear();
            store.fail.store(true, Ordering::SeqCst);
        } else if i == 2 {
            assert!(j.append(&queued).is_err());
        }
    }
    store.fail.store(false, Ordering::SeqCst);
    j.append(&queued).unwrap();
    drop(j);
    let (_, back, r) = Journal::open(Box::new(store.files.clone())).unwrap();
    assert_eq!(r.failed, 0);
    assert_eq!(back.to_snapshot(), c.to_snapshot());
}
