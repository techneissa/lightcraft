//! Automatic XMP sidecar writes through the queue ([`crate::sidecar_queue`]).

use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::Session;

/// A library of `n` photos whose originals are (empty) files in a folder of their own, auto-write on and the queue on.
fn library(tag: &str, n: u64) -> (Session, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-sidecar-queue-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    for i in 1..=n {
        let path = dir.join(format!("IMG_{i}.jpg"));
        std::fs::write(&path, b"jpeg").unwrap();
        let p =
            Photo::new(PhotoId(i), Source::File { path: path.to_string_lossy().to_string() }, &format!("IMG_{i}.jpg"), "JPEG", 4, 3, "2026-10-28");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.xmp.auto_write = true;
    s.queue_sidecar_writes(true);
    (s, dir)
}

fn sidecar(dir: &Path, i: u64) -> PathBuf {
    dir.join(format!("IMG_{i}.xmp"))
}

fn rating(dir: &Path, i: u64) -> Option<String> {
    let text = std::fs::read_to_string(sidecar(dir, i)).ok()?;
    let (_, rest) = text.split_once("<xmp:Rating>")?;
    rest.split_once("</xmp:Rating>").map(|(r, _)| r.to_string())
}

fn rate(s: &mut Session, ids: &[u64], rating: u8) {
    s.execute("library.select", &json!({"ids": ids})).unwrap();
    s.execute("photo.rate", &json!({"rating": rating})).unwrap();
}

/// A bulk change on 200 photos returns before their sidecars are written, with one row in the activity stack; the
/// worker writes them all, and the row goes once the queue is empty.
#[test]
fn a_bulk_change_returns_before_the_sidecars_are_written() {
    let (mut s, dir) = library("bulk", 200);
    s.hold_sidecar_writes();
    let ids: Vec<u64> = (1..=200).collect();
    rate(&mut s, &ids, 4);
    assert_eq!(s.sidecar_writes_pending(), 200, "the command didn't wait for the files");
    assert!(!sidecar(&dir, 1).exists() && !sidecar(&dir, 200).exists());
    let rows = s.activity.list();
    assert!(rows.iter().any(|r| r.kind == "xmp" && r.label == crate::sidecar_queue::LABEL && r.total == 200), "{rows:?}");
    s.finish_sidecar_writes();
    assert!((1..=200).all(|i| rating(&dir, i).as_deref() == Some("4")));
    assert_eq!((s.sidecar_writes_pending(), s.unsaved_sidecars()), (0, 0));
    assert!(!s.activity.list().iter().any(|r| r.kind == "xmp"), "the row goes with the last write");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A newer change replaces the photo's waiting write, and Save Metadata to File lands what is queued first, so a
/// queued write never puts older data over a later save.
#[test]
fn a_queued_write_never_overwrites_a_later_save() {
    let (mut s, dir) = library("save", 1);
    s.hold_sidecar_writes();
    rate(&mut s, &[1], 2);
    rate(&mut s, &[1], 3);
    assert_eq!(s.sidecar_writes_pending(), 1, "the newer write replaced the waiting one");
    // a change the sidecar doesn't get by itself, then the explicit save
    s.xmp.auto_write = false;
    rate(&mut s, &[1], 5);
    s.save_sidecar(PhotoId(1)).unwrap();
    assert_eq!(rating(&dir, 1).as_deref(), Some("5"), "the save is the last word");
    s.finish_sidecar_writes();
    assert_eq!(rating(&dir, 1).as_deref(), Some("5"), "nothing queued is left to land after it");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Read Metadata from File reads the file as the library last wrote it: the queued write lands first.
#[test]
fn reading_a_sidecar_waits_for_its_queued_write() {
    let (mut s, dir) = library("read", 1);
    s.hold_sidecar_writes();
    rate(&mut s, &[1], 3);
    let (op, _) = s.read_sidecar_op(PhotoId(1)).unwrap().expect("the queued write is on disk when it is read");
    assert!(format!("{op:?}").contains("rating: 3"), "{op:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A queued write that fails (the original is gone) leaves the photo unsaved, and the retry writes it once the file
/// is back.
#[test]
fn a_failed_queued_write_stays_unsaved_and_is_retried() {
    let (mut s, dir) = library("retry", 1);
    let original = dir.join("IMG_1.jpg");
    std::fs::remove_file(&original).unwrap();
    rate(&mut s, &[1], 4);
    s.finish_sidecar_writes();
    assert!(!sidecar(&dir, 1).exists());
    assert_eq!(s.unsaved_sidecars(), 1, "the failed write waits");
    std::fs::write(&original, b"jpeg").unwrap();
    assert_eq!(s.queue_unsaved_sidecar_retries(), 1, "queued again");
    s.finish_sidecar_writes();
    assert_eq!(rating(&dir, 1).as_deref(), Some("4"));
    assert_eq!(s.unsaved_sidecars(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Quitting (the session dropped) with writes still queued finishes them: none is lost.
#[test]
fn dropping_the_session_finishes_the_queued_writes() {
    let (mut s, dir) = library("quit", 20);
    s.hold_sidecar_writes();
    let ids: Vec<u64> = (1..=20).collect();
    rate(&mut s, &ids, 1);
    assert!(s.sidecar_writes_pending() > 0);
    drop(s);
    assert!((1..=20).all(|i| rating(&dir, i).as_deref() == Some("1")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Without the queue (tests, the CLI, MCP) the write happens in the command, as before.
#[test]
fn without_the_queue_the_command_writes_in_place() {
    let (mut s, dir) = library("sync", 1);
    s.queue_sidecar_writes(false);
    rate(&mut s, &[1], 2);
    assert_eq!(rating(&dir, 1).as_deref(), Some("2"));
    let _ = std::fs::remove_dir_all(&dir);
}
