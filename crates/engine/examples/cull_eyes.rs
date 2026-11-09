//! Assisted Culling's eyes (fork feature, `fork/ai-culling.md`) on real photos, without the app:
//!
//! ```text
//! cargo run --release -p lightcraft-engine --example cull_eyes -- OUT_DIR [--parse K] [--candle] PHOTO_OR_FOLDER...
//! ```
//!
//! Opens a library in `OUT_DIR/library` and adds the photos in place (only read: XMP writing is off), then times
//! `photo.analyze` without the eyes and with them (face detector and face mesh: the fast path), and prints each
//! photo's eyes. With `--parse K`, the first K photos are people-parsed first (into `OUT_DIR/parse-cache`) and
//! measured again, to show that a parsed photo runs no model. `--candle` runs the face mesh on candle, not Core ML. Results go to `OUT_DIR/results.json`. The models come
//! from the shared models folder.

use std::time::Instant;

use lightcraft_engine::Session;
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut parse = 0usize;
    if let Some(i) = args.iter().position(|a| a == "--parse") {
        parse = args.get(i + 1).and_then(|v| v.parse().ok()).ok_or("--parse K")?;
        args.drain(i..i + 2);
    }
    let candle = args.iter().any(|a| a == "--candle");
    args.retain(|a| a != "--candle");
    let (out, photos) = args.split_first().ok_or("usage: cull_eyes OUT_DIR [--parse K] PHOTO_OR_FOLDER...")?;
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models();
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    if candle {
        s.parsing.set_runtime(lightcraft_parsing::models::FACE_MESH.folder, lightcraft_engine::model_runtime::Runtime::Candle);
    }
    s.open_library(out.join("library"), false)?;
    s.execute("library.xmpPreferences", &json!({"autoWrite": false}))?;
    s.execute("library.import", &json!({ "paths": photos }))?;
    let ids: Vec<u64> = s.visible_cloned().iter().map(|id| id.0).collect();
    let names: std::collections::HashMap<u64, String> =
        s.visible_cloned().iter().filter_map(|id| s.catalog.photo(*id).map(|p| (id.0, p.file_name.clone()))).collect();
    println!("{} photos", ids.len());
    let t = Instant::now();
    s.execute("photo.analyze", &json!({ "ids": ids, "eyes": false }))?;
    let plain = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let r = s.execute("photo.analyze", &json!({ "ids": ids, "eyes": "again" }))?;
    let with_eyes = t.elapsed().as_secs_f64() * 1000.0;
    let n = ids.len().max(1) as f64;
    println!(
        "without eyes: {plain:.0} ms ({:.0} ms/photo); with eyes: {with_eyes:.0} ms ({:.0} ms/photo); eyes add {:.0} ms/photo; {}",
        plain / n,
        with_eyes / n,
        (with_eyes - plain) / n,
        r["eyes"]
    );
    for p in r["photos"].as_array().into_iter().flatten() {
        let id = p["id"].as_u64().unwrap_or(0);
        println!("  {} sharp {} eyes {} faces {}", names.get(&id).map_or("?", String::as_str), p["sharpness"], p["eyes"], p["faces"]);
    }
    println!("suggest reject: {}", r["suggestReject"]);
    let mut all = json!({"plainMs": plain, "eyesMs": with_eyes, "result": r, "names": names});
    if parse > 0 {
        let first: Vec<u64> = ids.iter().take(parse).copied().collect();
        let t = Instant::now();
        s.execute("parsing.run", &json!({ "ids": first, "wait": true }))?;
        println!("parsed {} photos in {:.1} s", first.len(), t.elapsed().as_secs_f64());
        let runs = s.parsing.runs();
        let t = Instant::now();
        let r: Value = s.execute("photo.analyze", &json!({ "ids": first, "eyes": "again" }))?;
        println!(
            "parsed photos again: {:.0} ms/photo, {} parse runs, eyes {}",
            t.elapsed().as_secs_f64() * 1000.0 / first.len().max(1) as f64,
            s.parsing.runs() - runs,
            r["eyes"]
        );
        all["parsed"] = r;
    }
    std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&all)?)?;
    Ok(())
}
