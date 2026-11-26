//! SCRATCH for issue #438, never merged: how much of each mask's plane is empty, so what keeping only the used part
//! (identical pixels) would save. `u16_sparse <photo> <cases.json>`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk};
use lightcraft_pipeline::{OutputDepth, OutputSpace};
use serde_json::{Value, json};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(&a[1]).unwrap()).unwrap();
    let mut s = Session::new().with_fs().with_system_clock();
    let r = s.execute("library.import", &json!({"paths": [std::fs::canonicalize(&a[0]).unwrap()]})).unwrap();
    let id = r["imported"][0].as_u64().expect("imported");
    s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    let ph = s.catalog.photo(PhotoId(id)).expect("photo");
    let long = ph.width.max(ph.height) as usize;
    let stem = std::path::Path::new(&a[0]).file_stem().unwrap().to_string_lossy().to_string();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        // one case a kind is enough: the edit doesn't change where a mask selects (but the global Shadows leave ranges alone)
        if !name.ends_with("-normal") || name.starts_with("all") {
            continue;
        }
        s.execute("develop.reset", &json!({})).unwrap();
        s.execute("develop.merge", &json!({"settings": case["settings"]})).unwrap();
        let job = s.export_job(PhotoId(id), long, long, OutputSpace::Srgb, OutputDepth::U8).unwrap();
        let source = job.source.load_source().unwrap();
        let info = source.info.clone().unwrap_or_else(|| job.info.clone());
        let ask = PlanesAsk { want: MaskWant::All, known: None };
        let out = lightcraft_pipeline::mask_planes(&source.image, &info, &job.settings, &job.request, None, &ask);
        let p = out.planes.expect("planes").remove(0).1;
        let (w, h) = (p.width, p.height);
        let n = (w * h) as f64;
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let (mut zero, mut one) = (0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                let v = p.data[y * w + x];
                if v == 0.0 {
                    zero += 1;
                } else {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
                if v == 1.0 {
                    one += 1;
                }
            }
        }
        let boxed = if x1 > x0 { ((x1 - x0) * (y1 - y0)) as f64 / n } else { 0.0 };
        let t = 256;
        let (mut stored_zero, mut stored_flat) = (0usize, 0usize);
        for ty in (0..h).step_by(t) {
            for tx in (0..w).step_by(t) {
                let (xe, ye) = ((tx + t).min(w), (ty + t).min(h));
                let vals = (ty..ye).flat_map(|y| p.data[y * w + tx..y * w + xe].iter().copied());
                let (mut all0, mut all1) = (true, true);
                for v in vals {
                    all0 &= v == 0.0;
                    all1 &= v == 1.0;
                }
                let size = (xe - tx) * (ye - ty);
                if !all0 {
                    stored_zero += size;
                    if !all1 {
                        stored_flat += size;
                    }
                }
            }
        }
        println!(
            "SPARSE\t{stem}\t{}\t{w}x{h}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}",
            name.trim_end_matches("-normal"),
            boxed,
            stored_zero as f64 / n,
            stored_flat as f64 / n,
            zero as f64 / n,
            one as f64 / n
        );
    }
}
