//! How long applying a portrait preset takes (fork feature, `fork/portrait-presets.md`, issue #106):
//!
//! ```text
//! cargo run --release -p lightcraft-engine --example portrait_preset_speed -- OUT_DIR
//! ```
//!
//! Makes a library in `OUT_DIR/library` with 2 060 photos (2 000 of them with Ann on, next to Bob; their files need
//! not exist: applying reads no pixels), then times `preset.apply` on 1 and on 50 photos and `retouch.applyToPerson`
//! over the library, which runs on a worker: the time the UI thread spends (reading the photos' faces, then
//! committing the one undo step) and the worker's. Each run is repeated and the median printed.

use std::time::Instant;

use lightcraft_catalog::{Op, Photo, Source};
use lightcraft_develop::retouch::Features;
use lightcraft_engine::Session;
use lightcraft_engine::portrait_presets::{self as pp, Portrait, Who};
use lightcraft_geom::Rect;
use lightcraft_meta::{Region, RegionKind};
use serde_json::json;

fn region(x: f64, name: &str) -> Region {
    Region { rect: Rect { x0: x, y0: 0.2, x1: x + 0.2, y1: 0.5 }, kind: RegionKind::Face, name: Some(name.into()), description: None }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).ok_or("usage: portrait_preset_speed OUT_DIR")?);
    let lib = out.join("library");
    let _ = std::fs::remove_dir_all(&lib);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false)?;
    let mut ann = Vec::new();
    for n in 0..2060 {
        let id = s.catalog.alloc_photo_id();
        let name = format!("IMG_{n:04}.jpg");
        let mut p = Photo::new(id, Source::File { path: lib.join(&name).to_string_lossy().into() }, &name, "JPEG", 6000, 4000, "2026-11-09T10:00:00");
        p.meta.regions = if n < 2000 { vec![region(0.1, "Bob"), region(0.6, "Ann")] } else { vec![region(0.4, "Bob")] };
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) })?;
        if n < 2000 {
            ann.push(id.0);
        }
    }
    let mut features = Features::default();
    features.set("retouch.skin.smoothness", 60.0);
    features.set("retouch.eyes.whites", 30.0);
    let preset = pp::make("Ann Look", pp::GROUP, &Portrait::of(&features, Who::named("Ann")), json!({}));
    let pid = s.add_presets(vec![preset]).into_iter().next().ok_or("no preset")?;
    let one = ann.first().copied().ok_or("no photos")?;
    s.execute("library.select", &json!({"ids": [one]}))?;
    let runs = 7;
    let time = |s: &mut Session, cmd: &str, p: serde_json::Value| -> Result<f64, Box<dyn std::error::Error>> {
        let mut ms = Vec::new();
        for _ in 0..runs {
            s.execute("retouch.reset", &json!({"id": one}))?;
            let t = Instant::now();
            s.execute(cmd, &p)?;
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        Ok(median(ms))
    };
    let t1 = time(&mut s, "preset.apply", json!({"id": pid, "ids": [one]}))?;
    let t50 = time(&mut s, "preset.apply", json!({"id": pid, "ids": ann.get(..50).ok_or("too few")?}))?;
    // Apply to Every Photo: the call on the UI thread, the worker, the commit on the UI thread
    let (mut start, mut worker, mut commit) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..runs {
        let before = s.undo.len();
        let t = Instant::now();
        let r = s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid}))?;
        start.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(r["state"], "running", "{r}");
        let t = Instant::now();
        let done = loop {
            let c = Instant::now();
            let r = s.execute("retouch.applyStatus", &json!({}))?;
            if r["state"] != "running" {
                commit.push(c.elapsed().as_secs_f64() * 1000.0);
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        worker.push(t.elapsed().as_secs_f64() * 1000.0 - commit.last().copied().unwrap_or(0.0));
        assert_eq!(done["count"], 2000, "{done}");
        for _ in before..s.undo.len() {
            s.execute("edit.undo", &json!({}))?;
        }
    }
    println!("preset.apply, 1 photo: {t1:.2} ms");
    println!("preset.apply, 50 photos: {t50:.2} ms");
    println!(
        "retouch.applyToPerson, 2000 photos of 2060 (one undo step): UI thread {:.1} ms to start + {:.1} ms to commit; worker {:.1} ms",
        median(start),
        median(commit),
        median(worker)
    );
    Ok(())
}
