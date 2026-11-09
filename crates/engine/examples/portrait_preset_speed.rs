//! How long applying a portrait preset takes (fork feature, `fork/portrait-presets.md`, issue #106):
//!
//! ```text
//! cargo run --release -p lightcraft-engine --example portrait_preset_speed -- OUT_DIR
//! ```
//!
//! Makes a library in `OUT_DIR/library` with 60 photos (50 of them with Ann on, next to Bob; their files need not
//! exist: applying reads no pixels), then times `preset.apply` on 1 and on 50 photos (one undo step for them all)
//! and `retouch.applyToPerson` (one undo step per photo). Each run is repeated and the median printed.

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
    for n in 0..60 {
        let id = s.catalog.alloc_photo_id();
        let name = format!("IMG_{n:04}.jpg");
        let mut p = Photo::new(id, Source::File { path: lib.join(&name).to_string_lossy().into() }, &name, "JPEG", 6000, 4000, "2026-11-09T10:00:00");
        p.meta.regions = if n < 50 { vec![region(0.1, "Bob"), region(0.6, "Ann")] } else { vec![region(0.4, "Bob")] };
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) })?;
        if n < 50 {
            ann.push(id.0);
        }
    }
    let mut features = Features::default();
    features.set("retouch.skin.smoothness", 60.0);
    features.set("retouch.eyes.whites", 30.0);
    let preset = pp::make("Ann Look", pp::GROUP, &Portrait { features, who: Who::named("Ann") }, json!({}));
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
    let t50 = time(&mut s, "preset.apply", json!({"id": pid, "ids": ann}))?;
    // applyToPerson commits only photos that change: undo it before each run
    let mut person = Vec::new();
    for _ in 0..runs {
        let before = s.undo.len();
        let t = Instant::now();
        let r = s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid}))?;
        person.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(r["count"], 50, "{r}");
        for _ in before..s.undo.len() {
            s.execute("edit.undo", &json!({}))?;
        }
    }
    println!("preset.apply, 1 photo: {t1:.2} ms");
    println!("preset.apply, 50 photos: {t50:.2} ms");
    println!("retouch.applyToPerson, 50 photos of 60 (50 undo steps): {:.2} ms", median(person));
    Ok(())
}
