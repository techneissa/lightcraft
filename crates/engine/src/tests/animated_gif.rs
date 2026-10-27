//! `export.animatedGif`: the selection as one GIF, validation, the activity row, cancel and
//! photos that can't be rendered.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lightcraft_catalog::{Op, Photo, Source};
use serde_json::{Value, json};

use crate::Session;

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-gif-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().to_string()
    }
    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> =
            std::fs::read_dir(&self.0).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
        v.sort();
        v
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A demo session and the ids of its first `n` photos, in view order.
fn session(n: usize) -> (Session, Vec<u64>) {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned().into_iter().take(n).map(|id| id.0).collect();
    (s, ids)
}

fn export(s: &mut Session, mut p: Value) -> Value {
    p["wait"] = json!(true);
    s.execute("export.animatedGif", &p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

/// Each frame's (width, height, delay) and the first pixel, decoded.
fn frames(path: &str) -> (u16, u16, Vec<(u16, u16, u16, [u8; 4])>) {
    let mut d = gif::DecodeOptions::new();
    d.set_color_output(gif::ColorOutput::RGBA);
    let mut r = d.read_info(std::fs::File::open(path).unwrap()).unwrap();
    let (w, h) = (r.width(), r.height());
    let mut out = Vec::new();
    while let Some(f) = r.read_next_frame().unwrap() {
        out.push((f.width, f.height, f.delay, [f.buffer[0], f.buffer[1], f.buffer[2], f.buffer[3]]));
    }
    (w, h, out)
}

#[test]
fn every_parameter_is_checked() {
    let (mut s, ids) = session(2);
    for p in [
        json!({}),
        json!({"path": "  "}),
        json!({"path": "x.gif", "size": 10}),
        json!({"path": "x.gif", "size": 100_000}),
        json!({"path": "x.gif", "size": "big"}),
        json!({"path": "x.gif", "seconds": 0}),
        json!({"path": "x.gif", "seconds": 61}),
        json!({"path": "x.gif", "seconds": "1"}),
        json!({"path": "x.gif", "loop": "yes"}),
        json!({"path": "x.gif", "dither": 1}),
        json!({"path": "x.gif", "wait": "no"}),
        json!({"path": "x.gif", "ids": "all"}),
        json!({"path": "x.gif", "ids": [-1]}),
        json!({"path": "x.gif", "fps": 30}),
        json!({"path": "x.gif", "ids": []}),
    ] {
        let mut p = p;
        if p.get("ids").is_none() {
            p["ids"] = json!(ids);
        }
        assert!(s.execute("export.animatedGif", &p).is_err(), "{p}");
    }
    assert!(s.activity.list().is_empty(), "nothing started");
}

#[test]
fn the_selection_becomes_one_frame_per_photo_in_view_order() {
    let dir = Scratch::new("sel");
    let (mut s, ids) = session(3);
    // selected last to first: the GIF still follows the view
    s.execute("library.select", &json!({"ids": [ids[2], ids[0], ids[1]]})).unwrap();
    let path = dir.path("sel.gif");
    let r = export(&mut s, json!({"path": path, "size": 120, "seconds": 0.5}));
    assert_eq!((r["frames"].as_u64(), r["skipped"].as_u64(), r["seconds"].as_f64()), (Some(3), Some(0), Some(1.5)), "{r}");
    assert_eq!(r["width"].as_u64().unwrap().max(r["height"].as_u64().unwrap()), 120);
    let (w, h, f) = frames(&path);
    assert_eq!((u64::from(w), u64::from(h)), (r["width"].as_u64().unwrap(), r["height"].as_u64().unwrap()));
    assert_eq!(f.len(), 3);
    assert!(f.iter().all(|x| (x.0, x.1, x.2) == (w, h, 50)), "{f:?}");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(r["bytes"].as_u64(), Some(bytes.len() as u64));
    assert!(bytes.windows(11).any(|x| x == b"NETSCAPE2.0"), "loops by default");
    // the same three as explicit ids in view order give the same file
    let again = dir.path("ids.gif");
    export(&mut s, json!({"path": again, "size": 120, "seconds": 0.5, "ids": ids}));
    assert_eq!(std::fs::read(&again).unwrap(), bytes);
    assert!(dir.files().iter().all(|f| !f.ends_with(".part")));
}

#[test]
fn plays_once_and_undithered_when_asked() {
    let dir = Scratch::new("once");
    let (mut s, ids) = session(2);
    let path = dir.path("once.gif");
    let r = export(&mut s, json!({"path": path, "size": 64, "loop": false, "dither": false, "ids": ids}));
    assert_eq!((r["loop"].as_bool(), r["dither"].as_bool()), (Some(false), Some(false)));
    assert!(!std::fs::read(&path).unwrap().windows(11).any(|x| x == b"NETSCAPE2.0"));
    assert_eq!(frames(&path).2.len(), 2);
}

#[test]
fn photos_of_other_shapes_fit_on_black() {
    let dir = Scratch::new("shape");
    let (mut s, ids) = session(1);
    // the same photo cropped to a narrow strip, after it: the strip sits in the middle of the frame
    let id = lightcraft_catalog::PhotoId(ids[0]);
    let tall = s.catalog.alloc_photo_id();
    let mut p = Photo::clone(s.catalog.photo(id).unwrap());
    p.id = tall;
    std::sync::Arc::make_mut(&mut p.develop).crop.geometry.rect = lightcraft_geom::Rect { x0: 0.45, y0: 0.0, x1: 0.55, y1: 1.0 };
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let path = dir.path("shape.gif");
    let r = export(&mut s, json!({"path": path, "size": 96, "ids": [ids[0], tall.0]}));
    let first = s.slide_photo_aspect(id).unwrap();
    let shape = r["width"].as_f64().unwrap() / r["height"].as_f64().unwrap();
    assert!((shape - first).abs() < 0.05, "the first photo's shape {first}: {r}");
    let (_, _, f) = frames(&path);
    let corner = f[1].3;
    assert!(corner[0] < 8 && corner[1] < 8 && corner[2] < 8, "black beside the strip: {corner:?}");
}

fn wait_for(s: &Session) -> Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let run = s.animated_gif.clone().unwrap();
        if !run.running() {
            return run.json();
        }
        assert!(Instant::now() < deadline, "export did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn background_export_has_an_activity_row_and_reports_its_result() {
    let dir = Scratch::new("bg");
    let (mut s, ids) = session(4);
    let path = dir.path("bg.gif");
    let r = s.execute("export.animatedGif", &json!({"path": path, "size": 64, "ids": ids})).unwrap();
    assert_eq!((r["background"].as_bool(), r["frames"].as_u64()), (Some(true), Some(4)));
    let rows = s.activity.list();
    let row = rows.iter().find(|t| t.id == r["taskId"].as_u64().unwrap()).expect("a row in the activity stack");
    assert_eq!((row.kind, row.label.as_str(), row.total), ("export", "Exporting animated GIF", 4));
    assert!(row.cancellable);
    assert!(s.execute("export.animatedGif", &json!({"path": dir.path("other.gif"), "ids": ids})).is_err(), "one at a time");
    let status = wait_for(&s);
    assert_eq!(status["result"]["frames"], 4, "{status}");
    assert_eq!(status["done"], 4);
    assert!(s.activity.list().is_empty(), "the row goes when the export ends");
    assert_eq!(s.execute("export.animatedGifStatus", &json!({})).unwrap()["result"]["path"], json!(path));
    assert!(Path::new(&path).is_file());
}

#[test]
fn cancel_leaves_no_file_behind() {
    let dir = Scratch::new("cancel");
    let (mut s, ids) = session(12);
    let path = dir.path("c.gif");
    let r = s.execute("export.animatedGif", &json!({"path": path, "size": 400, "ids": ids})).unwrap();
    s.execute("activity.cancel", &json!({"id": r["taskId"]})).unwrap();
    let status = wait_for(&s);
    assert!(status["error"].as_str().is_some_and(|e| e.contains("cancelled")), "{status}");
    assert!(!Path::new(&path).exists());
    assert!(dir.files().is_empty(), "{:?}", dir.files());
    assert!(s.activity.list().is_empty());
}

#[test]
fn a_photo_that_cannot_render_is_skipped_with_a_warning() {
    let dir = Scratch::new("skip");
    let (mut s, mut ids) = session(2);
    let id = s.catalog.alloc_photo_id();
    let gone = dir.path("gone.jpg");
    s.catalog
        .apply(Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: gone }, "gone.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00")) })
        .unwrap();
    ids.insert(1, id.0);
    let r = export(&mut s, json!({"path": dir.path("s.gif"), "size": 64, "ids": ids}));
    assert_eq!((r["frames"].as_u64(), r["skipped"].as_u64()), (Some(2), Some(1)));
    assert!(r["warnings"][0].as_str().unwrap().contains("gone.jpg"), "{r}");
    // only the broken photo: nothing to write, and nothing written
    let e = s.execute("export.animatedGif", &json!({"path": dir.path("none.gif"), "ids": [id.0], "wait": true})).unwrap_err();
    assert!(e.to_string().contains("no photo could be rendered"), "{e}");
    assert!(dir.files().iter().all(|f| f != "none.gif" && !f.ends_with(".part")), "{:?}", dir.files());
    // nothing selected
    s.execute("library.select", &json!({"ids": []})).unwrap();
    s.selection.active = None;
    let e = s.execute("export.animatedGif", &json!({"path": dir.path("empty.gif")})).unwrap_err();
    assert!(e.to_string().contains("select the photos"), "{e}");
}

#[test]
fn an_original_is_never_overwritten() {
    let dir = Scratch::new("orig");
    let (mut s, _) = session(1);
    let id = s.catalog.alloc_photo_id();
    let original = dir.path("photo.gif");
    std::fs::write(&original, b"GIF89a").unwrap();
    s.catalog
        .apply(Op::AddPhoto {
            photo: Box::new(Photo::new(id, Source::File { path: original.clone() }, "photo.gif", "GIF", 1, 1, "2026-01-01T10:00:00")),
        })
        .unwrap();
    let ids: Vec<u64> = s.visible_cloned().into_iter().take(2).map(|x| x.0).collect();
    assert!(s.execute("export.animatedGif", &json!({"path": original, "ids": ids, "wait": true})).is_err());
    assert_eq!(std::fs::read(&original).unwrap(), b"GIF89a");
}
