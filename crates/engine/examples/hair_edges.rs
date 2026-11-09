//! Hair edges before and after (fork feature, issue #103): for each photo, the Subject mask (`mask.add subject`, with
//! the app's selection model) and the people parse's Entire Person and Background (`parsing.mask`) with the Current
//! edge and with BiRefNet Matting, and the seconds each took.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example hair_edges -- OUT_DIR PHOTO...`
//!
//! Writes `OUT_DIR/<stem>.subject.<edge>.f32` (the stored grid's logits, little-endian; its side is in
//! `timings.json`), `OUT_DIR/<stem>.<part>.<edge>.png` (grey masks at 2048 px) and `OUT_DIR/timings.json`. The
//! models are the app's defaults (the shared models folder); parses are cached in `OUT_DIR/parse-cache`.
use std::time::Instant;

use lightcraft_develop::MaskShape;
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use serde_json::{Value, json};

fn grid(s: &Session, id: PhotoId) -> Option<(usize, Vec<f32>, Option<String>)> {
    let d = s.develop_of(id)?;
    let MaskShape::Subject { seg: Some(seg), .. } = &d.masks.last()?.components.first()?.shape else { return None };
    Some((seg.side as usize, seg.logits()?, seg.model.clone()))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: hair_edges OUT_DIR PHOTO...");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut s = Session::new().with_fs().with_default_sam3_model().with_default_parsing_models();
    s.segmenter.background = false;
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    let paths: Vec<String> = args[1..].iter().map(|p| std::fs::canonicalize(p).expect("photo").to_string_lossy().to_string()).collect();
    let r = s.execute("library.import", &json!({"paths": paths})).expect("import");
    let ids: Vec<PhotoId> = r["imported"].as_array().expect("ids").iter().filter_map(|v| v.as_u64()).map(PhotoId).collect();
    assert_eq!(ids.len(), paths.len(), "every photo imports");
    println!("selection model: {}", s.execute("segment.selectionModel", &json!({})).expect("model")["model"]);
    // (`HAIR_EDGES=current,birefnet-matting,apple-vision` picks the edges; `fork/apple-vision-masks.md`)
    let list = std::env::var("HAIR_EDGES").unwrap_or_else(|_| "current,birefnet-matting".into());
    let edges: Vec<&str> = list.split(',').map(str::trim).filter(|e| !e.is_empty()).collect();
    let set = |s: &mut Session, kind: &str, edge: &str| {
        s.execute("maskEdges.edge", &json!({"kind": kind, "edge": edge})).unwrap_or_else(|e| panic!("{kind} {edge}: {e}"));
    };
    let mut timings: Vec<Value> = Vec::new();
    for (k, &id) in ids.iter().enumerate() {
        s.execute("library.select", &json!({"ids": [id.0]})).ok();
        let stem = std::path::Path::new(&args[1 + k]).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for &edge in &edges {
            set(&mut s, "subject", edge);
            let t = Instant::now();
            let made = s.execute("mask.add", &json!({"kind": "subject"}));
            let seconds = t.elapsed().as_secs_f64();
            match (made, grid(&s, id)) {
                (Ok(_), Some((side, logits, model))) => {
                    let bytes: Vec<u8> = logits.iter().flat_map(|v| v.to_le_bytes()).collect();
                    std::fs::write(out.join(format!("{stem}.subject.{edge}.f32")), bytes).expect("write");
                    println!("{stem:22} subject {edge:17} {seconds:6.2} s  {side}²  {model:?}");
                    timings.push(json!({"photo": stem, "mask": "subject", "edge": edge, "seconds": seconds, "side": side, "model": model}));
                }
                (r, _) => println!("{stem:22} subject {edge:17} nothing: {r:?}"),
            }
            s.execute("mask.deleteAll", &json!({})).ok();
        }
        for &edge in &edges {
            set(&mut s, "people", edge);
            let t = Instant::now();
            let run = s.execute("parsing.run", &json!({"wait": true})).expect("parse");
            let seconds = t.elapsed().as_secs_f64();
            let state = run["photos"][0]["state"].as_str().unwrap_or("").to_string();
            let cached = run["photos"][0]["cached"] == true;
            println!("{stem:22} people  {edge:17} {seconds:6.2} s  {state} cached={cached} {}", run["photos"][0]["error"]);
            timings.push(json!({"photo": stem, "mask": "people", "edge": edge, "seconds": seconds, "cached": cached, "state": state}));
            for part in ["entirePerson", "background"] {
                let path = out.join(format!("{stem}.{part}.{edge}.png"));
                if let Err(e) = s.execute("parsing.mask", &json!({"part": part, "path": path.to_string_lossy(), "edge": 2048})) {
                    println!("  {part}: {e}");
                }
            }
        }
    }
    std::fs::write(out.join("timings.json"), serde_json::to_string_pretty(&timings).expect("json")).expect("write timings");
}
