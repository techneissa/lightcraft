//! Comparison of the Subject models on photos (fork feature, issue #48): for each photo, the mask SAM 3, BiRefNet and
//! "SAM 3 + BiRefNet Edges" make through the same command (`mask.add subject`), with the seconds each took.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example birefnet_compare -- OUT_DIR PHOTO...`
//!
//! Writes `OUT_DIR/<stem>.<model>.f32` (the stored grid's logits, little-endian) and `OUT_DIR/timings.json`
//! (`[{photo, model, seconds, side}]`). SAM 3's analysis of a photo is cached for the next model, so the combined
//! seconds leave it out (the sheet says so). The models and the SAM 3 folder are the app's defaults.
use std::time::Instant;

use lightcraft_develop::MaskShape;
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use serde_json::json;

fn grid(s: &Session, id: PhotoId) -> Option<(usize, Vec<f32>)> {
    let d = s.develop_of(id)?;
    let MaskShape::Subject { seg: Some(seg), .. } = &d.masks.last()?.components.first()?.shape else { return None };
    Some((seg.side as usize, seg.logits()?))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: birefnet_compare OUT_DIR PHOTO...");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut s = Session::new().with_fs().with_default_sam3_model();
    s.segmenter.background = false;
    let paths: Vec<String> = args[1..].iter().map(|p| std::fs::canonicalize(p).expect("photo").to_string_lossy().to_string()).collect();
    let r = s.execute("library.import", &json!({"paths": paths})).expect("import");
    let ids: Vec<PhotoId> = r["imported"].as_array().expect("ids").iter().filter_map(|v| v.as_u64()).map(PhotoId).collect();
    assert_eq!(ids.len(), paths.len(), "every photo imports");
    let models = ["sam3", "birefnet", "sam3+birefnet"];
    for m in models {
        s.execute("segment.selectionModel", &json!({"model": m})).unwrap_or_else(|e| panic!("{m}: {e}"));
    }
    // warm-up: load every model on the first photo (a nudged exposure, so its analysis is not reused below)
    s.execute("library.select", &json!({"ids": [ids[0].0]})).ok();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.07})).expect("nudge");
    for m in models {
        s.execute("segment.selectionModel", &json!({"model": m})).unwrap();
        let _ = s.execute("mask.add", &json!({"kind": "subject"})); // (a photo with no subject still loads the model)
    }
    s.execute("mask.deleteAll", &json!({})).ok();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.0})).expect("reset");
    let mut timings = Vec::new();
    for (k, &id) in ids.iter().enumerate() {
        s.execute("library.select", &json!({"ids": [id.0]})).ok();
        let stem = std::path::Path::new(&args[1 + k]).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for m in models {
            s.execute("segment.selectionModel", &json!({"model": m})).unwrap();
            let t = Instant::now();
            let made = s.execute("mask.add", &json!({"kind": "subject"}));
            let seconds = t.elapsed().as_secs_f64();
            match (made, grid(&s, id)) {
                (Ok(_), Some((side, logits))) => {
                    let bytes: Vec<u8> = logits.iter().flat_map(|v| v.to_le_bytes()).collect();
                    std::fs::write(out.join(format!("{stem}.{m}.f32")), bytes).expect("write");
                    println!("{stem:20} {m:14} {seconds:6.2} s  {side}²");
                    timings.push(json!({"photo": stem, "model": m, "seconds": seconds, "side": side}));
                }
                (r, _) => {
                    println!("{stem:20} {m:14} nothing: {r:?}");
                    timings.push(json!({"photo": stem, "model": m, "seconds": seconds, "side": 0}));
                }
            }
            s.execute("mask.deleteAll", &json!({})).ok();
        }
    }
    std::fs::write(out.join("timings.json"), serde_json::to_string_pretty(&timings).expect("json")).expect("write timings");
}
