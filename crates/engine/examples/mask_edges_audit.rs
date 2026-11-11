//! Mask edges at full size (issue #240): for each photo, the Subject, Background and Person (Select People, Entire
//! Person) masks as the app makes them now, rendered at the photo's full size as the render draws them, so their edge
//! can be checked at 1:1 around fingers, arm gaps and hair.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example mask_edges_audit -- OUT_DIR PHOTO...`
//!
//! Writes `OUT_DIR/<stem>.photo.png`, `OUT_DIR/<stem>.<subject|background|person>.png` (grey masks, white =
//! selected) and `OUT_DIR/seconds.json` (how long each mask took to make). `MASK_EDGES=default,current,birefnet-matting`
//! picks the Hair Edges settings to try (`default`: the ones a new user gets; the default list) (one set of files per edge after the first, named `<stem>.<mask>.<edge>.png`).
use std::time::Instant;

use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_pipeline::{MaskView, Overlay};
use serde_json::{Value, json};

fn save(img: &lightcraft_raster::Rgba8, path: &std::path::Path, grey: bool) {
    let (w, h) = (img.width as u32, img.height as u32);
    if grey {
        let px: Vec<u8> = img.data.iter().map(|p| p[0]).collect();
        image::GrayImage::from_raw(w, h, px).expect("size").save(path).expect("write");
    } else {
        let px: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        image::RgbImage::from_raw(w, h, px).expect("size").save(path).expect("write");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: mask_edges_audit OUT_DIR PHOTO...");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut s = Session::new().with_fs().with_default_sam3_model().with_default_parsing_models();
    s.segmenter.background = false;
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    let paths: Vec<String> = args[1..].iter().map(|p| std::fs::canonicalize(p).expect("photo").to_string_lossy().to_string()).collect();
    let r = s.execute("library.import", &json!({"paths": paths})).expect("import");
    let ids: Vec<PhotoId> = r["imported"].as_array().expect("ids").iter().filter_map(|v| v.as_u64()).map(PhotoId).collect();
    assert_eq!(ids.len(), paths.len(), "every photo imports");
    let list = std::env::var("MASK_EDGES").unwrap_or_else(|_| "default".into());
    let edges: Vec<&str> = list.split(',').map(str::trim).filter(|e| !e.is_empty()).collect();
    let only: Option<Vec<String>> = std::env::var("MASKS").ok().map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let wanted = |m: &str| only.as_ref().is_none_or(|o| o.iter().any(|x| x == m));
    let mut seconds: Vec<Value> = Vec::new();
    for (k, &id) in ids.iter().enumerate() {
        s.execute("library.select", &json!({"ids": [id.0]})).ok();
        let stem = std::path::Path::new(&args[1 + k]).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (w, h) = s.catalog.photo(id).map(|p| (p.width as usize, p.height as usize)).expect("photo size");
        let (w, h) = if w == 0 || h == 0 { (8192, 8192) } else { (w, h) };
        let photo = out.join(format!("{stem}.photo.png"));
        if !photo.exists() {
            let img = s.render_job(id, w, h, false, true).expect("job").run().rendered.expect("render").image;
            save(&img, &photo, false);
        }
        for (e, &edge) in edges.iter().enumerate() {
            // (`default`: the edges a new user gets)
            for kind in ["subject", "people"].into_iter().filter(|_| edge != "default") {
                s.execute("maskEdges.edge", &json!({"kind": kind, "edge": edge})).unwrap_or_else(|err| panic!("{kind} {edge}: {err}"));
            }
            let tag = if e == 0 { String::new() } else { format!(".{edge}") };
            for mask in ["subject", "background", "person"] {
                if !wanted(mask) {
                    continue;
                }
                s.execute("mask.deleteAll", &json!({})).ok();
                let t = Instant::now();
                let made = match mask {
                    "person" => s.execute("selectPeople.add", &json!({"parts": ["entirePerson"]})),
                    kind => s.execute("mask.add", &json!({"kind": kind})),
                };
                let secs = t.elapsed().as_secs_f64();
                let Some(m) = made.as_ref().ok().and(s.develop_of(id)).and_then(|d| d.masks.first().cloned()) else {
                    println!("{stem:28} {mask:10} {edge:17} nothing: {made:?}");
                    continue;
                };
                let model = match m.components.first().map(|c| &c.shape) {
                    Some(
                        lightcraft_develop::MaskShape::Subject { seg, .. }
                        | lightcraft_develop::MaskShape::Background { seg, .. }
                        | lightcraft_develop::MaskShape::People { seg, .. },
                    ) => seg.as_ref().map(|g| format!("{}² {}", g.side, g.model.as_deref().unwrap_or("sam3"))),
                    _ => None,
                };
                let job = s.render_job(id, w, h, false, true).expect("job").with_overlay(Overlay::Mask {
                    id: m.id as u16,
                    view: MaskView::WhiteOnBlack,
                    color: [255, 255, 255],
                    opacity: 100,
                });
                let img = job.run().rendered.expect("render").image;
                save(&img, &out.join(format!("{stem}.{mask}{tag}.png")), true);
                println!("{stem:28} {mask:10} {edge:17} {secs:6.2} s  {model:?}");
                seconds.push(json!({"photo": stem, "mask": mask, "edge": edge, "seconds": secs, "grid": model}));
            }
        }
        s.execute("mask.deleteAll", &json!({})).ok();
    }
    std::fs::write(out.join("seconds.json"), serde_json::to_string_pretty(&seconds).expect("json")).expect("write");
}
