//! Hair colour after a background swap (fork feature, issue #115): for each photo, a Background mask (the app's
//! Subject model, turned around) with the Current edge and with BiRefNet Matting, its Recolor set to a colour far from
//! the photo's background, rendered with Decontaminate off (0) and on (100); the seconds each step took.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example decontam_compare -- OUT_DIR PHOTO=RRGGBB...`
//!
//! Writes `OUT_DIR/<stem>.<edge>.d<0|100>.png` (1600 px on the long side) and `OUT_DIR/timings.json`.
use std::time::Instant;

use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::export::{ExportOptions, export_photo};
use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: decontam_compare OUT_DIR PHOTO=RRGGBB...");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut s = Session::new().with_fs().with_default_sam3_model().with_default_parsing_models();
    s.segmenter.background = false;
    let mut timings: Vec<Value> = Vec::new();
    for arg in &args[1..] {
        let (path, hex) = arg.rsplit_once('=').expect("PHOTO=RRGGBB");
        let rgb: Vec<f64> = (0..3).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex") as f64 / 255.0).collect();
        let full = std::fs::canonicalize(path).expect("photo").to_string_lossy().to_string();
        let r = s.execute("library.import", &json!({"paths": [full]})).expect("import");
        let id = PhotoId(r["imported"][0].as_u64().expect("imported"));
        s.execute("library.select", &json!({"ids": [id.0]})).ok();
        let stem = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for edge in ["current", "birefnet-matting"] {
            s.execute("maskEdges.edge", &json!({"kind": "subject", "edge": edge})).expect("edge");
            s.execute("mask.deleteAll", &json!({})).ok();
            let t = Instant::now();
            if let Err(e) = s.execute("mask.add", &json!({"kind": "background"})) {
                println!("{stem:28} {edge:17} no background: {e}");
                continue;
            }
            let mask_s = t.elapsed().as_secs_f64();
            let values = json!({"recolor": 100.0, "recolor_r": rgb[0], "recolor_g": rgb[1], "recolor_b": rgb[2]});
            s.execute("mask.adjust", &json!({"values": values})).expect("recolor");
            for d in [0.0, 100.0] {
                s.execute("mask.decontaminate", &json!({"value": d})).expect("decontaminate");
                let o = ExportOptions::from_json(&json!({"format": "png", "width": 1600, "height": 1600}));
                let t = Instant::now();
                let e = export_photo(&mut s, id, &o, 1).expect("export");
                let render_s = t.elapsed().as_secs_f64();
                std::fs::write(out.join(format!("{stem}.{edge}.d{d}.png")), &e.bytes).expect("write");
                println!("{stem:28} {edge:17} mask {mask_s:6.2} s  decontaminate {d:3}: render {render_s:5.2} s");
                timings.push(json!({"photo": stem, "edge": edge, "decontaminate": d, "mask_seconds": mask_s, "render_seconds": render_s}));
            }
        }
        s.execute("mask.deleteAll", &json!({})).ok();
    }
    std::fs::write(out.join("timings.json"), serde_json::to_string_pretty(&timings).expect("json")).expect("write timings");
}
