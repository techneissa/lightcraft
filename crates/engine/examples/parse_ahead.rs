//! See people parsing ahead of time (fork feature, `fork/parse-ahead.md`) work on real photos, without the app:
//!
//! ```text
//! LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --example parse_ahead -- OUT_DIR PHOTO...
//! ```
//!
//! Imports the photos into a library in `OUT_DIR/library` (in place: give it copies), finds their faces (YuNet, from
//! the shared models folder, as the face scan does), opens the middle photo and parses ahead into `OUT_DIR/parse-cache`:
//! first at the light pace (the photos around the open one), then at the normal pace (the rest). Then it opens every
//! photo as a request would (`parsing.run`) and says whether a model had to run. The models come from the shared
//! models folder; the chosen set from `LIGHTCRAFT_CONFIG_DIR`.

use std::time::Instant;

use lightcraft_engine::{Selection, Session};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (out, photos) = args.split_first().ok_or("usage: parse_ahead OUT_DIR PHOTO...")?;
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models();
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    s.open_library(out.join("library"), false)?;
    s.execute("library.import", &json!({ "paths": photos }))?;
    let ids: Vec<u64> = s.visible_cloned().iter().map(|id| id.0).collect();
    let t = Instant::now();
    let faces = s.execute("faces.detect", &json!({ "ids": ids }))?;
    println!("faces.detect: {:.1} s", t.elapsed().as_secs_f64());
    for r in faces["photos"].as_array().into_iter().flatten() {
        println!("  photo {}: {} faces", r["id"], r["faces"].as_array().map_or(0, Vec::len));
    }
    let view = s.visible_cloned();
    let middle = *view.get(view.len() / 2).ok_or("no photos")?;
    s.selection = Selection::single(middle);
    println!("open photo: {} (filmstrip: {:?})", middle.0, ids);
    let names: std::collections::HashMap<u64, String> =
        view.iter().filter_map(|id| s.catalog.photo(*id).map(|p| (id.0, p.file_name.clone()))).collect();
    for pace in ["light", "normal"] {
        let t = Instant::now();
        let runs = s.parsing.runs();
        let st = settle(&mut s, pace)?;
        println!(
            "pace {pace}: {:.1} s, {} model runs, state {}, parsed {}, skipped {}, failed {}, queued {}",
            t.elapsed().as_secs_f64(),
            s.parsing.runs() - runs,
            st["state"],
            st["parsed"],
            st["skipped"],
            st["failed"],
            st["queued"]
        );
    }
    println!("opening each photo (parsing.run, as the portrait tools ask):");
    for id in &view {
        let runs = s.parsing.runs();
        let t = Instant::now();
        let r = s.execute("parsing.run", &json!({ "ids": [id.0], "wait": true }))?;
        let p = &r["photos"][0];
        let people = p["parse"]["people"].as_array().map_or(0, Vec::len);
        println!(
            "  {} ({}): {} in {:.1} ms, cached {}, model runs {}, {} people",
            names.get(&id.0).map_or("?", String::as_str),
            id.0,
            p["state"],
            t.elapsed().as_secs_f64() * 1000.0,
            p["cached"],
            s.parsing.runs() - runs,
            people
        );
    }
    let st: Value = s.execute("parsing.ahead.status", &json!({}))?;
    println!("status: {st}");
    // a restart: what the index says is parsed is skipped without a render
    drop(s);
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models();
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    s.open_library(out.join("library"), false)?;
    s.selection = Selection::single(middle);
    let t = Instant::now();
    let st = settle(&mut s, "normal")?;
    println!(
        "after a restart: {:.2} s, {} model runs, parsed {}, skipped {}",
        t.elapsed().as_secs_f64(),
        s.parsing.runs(),
        st["parsed"],
        st["skipped"]
    );
    // opened after the restart: from disk, nothing in memory yet
    for id in &view {
        let t = Instant::now();
        let r = s.execute("parsing.run", &json!({ "ids": [id.0], "wait": true }))?;
        println!(
            "  after the restart, {}: {} in {:.1} ms, cached {}, model runs so far {}",
            names.get(&id.0).map_or("?", String::as_str),
            r["photos"][0]["state"],
            t.elapsed().as_secs_f64() * 1000.0,
            r["photos"][0]["cached"],
            s.parsing.runs()
        );
    }
    Ok(())
}

/// Pump at `pace` until the queue is done or stands still (a waiting pump returns after a few seconds).
fn settle(s: &mut Session, pace: &str) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let st = s.execute("parsing.ahead.pump", &json!({ "pace": pace, "wait": true }))?;
        if !matches!(st["state"].as_str(), Some("working" | "paused" | "loading")) {
            return Ok(st);
        }
    }
}
