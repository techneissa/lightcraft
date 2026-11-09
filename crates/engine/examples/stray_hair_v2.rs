//! Stray Hair v2 before and after (fork feature, `fork/retouch-hair.md`): for each photo, the face search (Apple Vision's
//! matte and LaMa), the time it took per face, then the head at 100 % with Stray Hair off and at 100, and a slider tick.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --example stray_hair_v2 -- OUT_DIR PHOTO...`
//!
//! Writes, per photo, `OUT_DIR/<stem>-{before,after}.png` (the whole photo at 2048 px) and `OUT_DIR/<stem>-head-{before,after,
//! after-50}.png` (the part of the photo Stray Hair changed most, from the full-size render at 1:1, 1200 × 900 px); prints the times.
//! Models are the app's defaults (the shared models folder). Never touches the photo's own folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_pipeline::{PixelWindow, Quality, RenderRequest, StageCache};
use lightcraft_raster::Rgba8;
use serde_json::json;
use std::sync::Arc;

const EDGE: usize = 2048;
const CROP_W: usize = 1200;
const CROP_H: usize = 900;

fn save(img: &Rgba8, path: &std::path::Path) {
    let mut bytes = Vec::with_capacity(img.width * img.height * 3);
    for p in &img.data {
        bytes.extend_from_slice(&p[..3]);
    }
    image::RgbImage::from_raw(img.width as u32, img.height as u32, bytes).expect("size").save(path).expect("save");
}

fn render(s: &mut Session, id: PhotoId, window: Option<(usize, usize, PixelWindow)>) -> Rgba8 {
    let (w, h) = window.map_or((EDGE, EDGE), |(w, h, _)| (w, h));
    let mut job = s.render_job(id, w, h, false, false).expect("job");
    if let Some((_, _, win)) = window {
        job.request.window = Some(win);
        job.request.exact_size = true;
    }
    job.run().rendered.expect("render").image
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: stray_hair_v2 OUT_DIR PHOTO...");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models().with_default_lama_model();
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    println!("LaMa installed: {}", s.lama.installed());
    for path in &args[1..] {
        let abs = std::fs::canonicalize(path).expect("photo").to_string_lossy().to_string();
        let stem = std::path::Path::new(path).file_stem().expect("stem").to_string_lossy().to_string();
        let r = s.execute("library.import", &json!({"paths": [abs]})).expect("import");
        let id = PhotoId(r["imported"][0].as_u64().expect("id"));
        s.execute("library.select", &json!({"ids": [id.0]})).expect("select");
        let t = Instant::now();
        s.execute("retouch.detect", &json!({"id": id.0})).expect("detect");
        println!("{stem}: faces (parse + searches): {:.2} s", t.elapsed().as_secs_f64());
        // the searches again: the parse is known now, so this is the searches alone
        let t = Instant::now();
        s.execute("retouch.detect", &json!({"id": id.0, "again": true})).expect("detect");
        println!("{stem}: face search with the parse known: {:.2} s", t.elapsed().as_secs_f64());
        println!("{stem}: LaMa fills kept {}, message {:?}", s.lama.cached_fills(), s.lama.message());
        let found = s.retouch_faces_of(id).found.expect("faces");
        for (i, f) in found.input.faces.iter().enumerate() {
            let st = f.strands.as_ref();
            let removal = st.and_then(|s| s.removal.as_ref());
            println!(
                "{stem}: face {i}: {} px, strands {:?} (v2 took {:?}), v2 {:?} ms + crop {:?} ms, stored {:?} bytes",
                st.map_or(0, |s| s.face_px),
                st.map(|s| s.count),
                removal.map(|r| r.count),
                removal.map(|r| r.ms),
                removal.map(|r| r.crop_ms),
                removal.map(|r| r.bytes())
            );
        }
        let Some(face) = found.input.faces.first() else { continue };
        let p = s.catalog.photo(id).expect("photo");
        let (pw, ph) = (p.width as usize, p.height as usize);
        let set = |s: &mut Session, v: f64| s.execute("retouch.set", &json!({"control": "retouch.hair.amount", "value": v})).expect("set");
        set(&mut s, 0.0);
        let before = render(&mut s, id, None);
        set(&mut s, 100.0);
        let after = render(&mut s, id, None);
        save(&before, &out.join(format!("{stem}-before.png")));
        save(&after, &out.join(format!("{stem}-after.png")));
        // where Stray Hair changed the picture most, at 1:1 from the full-size render: the densest window of the changed
        // pixels of the 2048 px pictures, scaled up
        let scale = pw as f64 / before.width as f64;
        let (cw, ch) = ((CROP_W as f64 / scale) as usize, (CROP_H as f64 / scale) as usize);
        let changed = |x: usize, y: usize| {
            let (p, q) = (before.data[y * before.width + x], after.data[y * after.width + x]);
            (0..3).map(|k| p[k].abs_diff(q[k]) as u32).sum::<u32>() > 12
        };
        let (mut best, mut at) = (0usize, (0usize, 0usize));
        for y in (0..before.height.saturating_sub(ch)).step_by(8) {
            for x in (0..before.width.saturating_sub(cw)).step_by(8) {
                let n = (y..y + ch).step_by(2).map(|yy| (x..x + cw).step_by(2).filter(|xx| changed(*xx, yy)).count()).sum::<usize>();
                if n > best {
                    (best, at) = (n, (x, y));
                }
            }
        }
        let x0 = ((at.0 as f64 * scale) as usize).min(pw.saturating_sub(CROP_W));
        let y0 = ((at.1 as f64 * scale) as usize).min(ph.saturating_sub(CROP_H));
        let win = PixelWindow { x: x0, y: y0, w: CROP_W.min(pw - x0), h: CROP_H.min(ph - y0) };
        for (name, v) in [("before", 0.0), ("after", 100.0), ("after-50", 50.0)] {
            set(&mut s, v);
            save(&render(&mut s, id, Some((pw, ph, win))), &out.join(format!("{stem}-head-{name}.png")));
        }
        // Stray Hair's own share of a tick: the stored change applied to a draft-sized picture
        if let Some(removal) = face.strands.as_ref().and_then(|s| s.removal.clone()) {
            let (dw, dh) = (1920usize, (1920.0 * ph as f64 / pw as f64).round() as usize);
            let view = lightcraft_develop::retouch::WholePhoto { width: dw, height: dh };
            let face_px = (face.bounds.width() * dw as f64).max(face.bounds.height() * dh as f64);
            let mut times = Vec::new();
            for _ in 0..12 {
                let mut img = lightcraft_raster::Rgb32f::filled(dw, dh, [0.3; 3]);
                let t = Instant::now();
                removal.apply(&mut img, &view, face_px, 0.7);
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
            times.sort_by(f64::total_cmp);
            println!("{stem}: Stray Hair apply alone on {dw} × {dh}: median {:.2} ms, min {:.2} ms", times[times.len() / 2], times[0]);
        }
        // a slider tick: a draft render with the stage cache warm, one slider moving
        for (name, control, base) in
            [("Stray Hair Amount", "retouch.hair.amount", 40.0), ("Skin Smoothness, for comparison", "retouch.skin.smoothness", 40.0)]
        {
            let cache = Arc::new(StageCache::default());
            let mut times = Vec::new();
            for k in 0..14 {
                s.execute("retouch.set", &json!({"control": control, "value": base + k as f64})).expect("set");
                let mut job = s.render_job(id, 1920, 1280, false, false).expect("job");
                job.request = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(1920, 1280) };
                job.stages = Some(cache.clone());
                let t = Instant::now();
                drop(job.run());
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
            times.drain(..2);
            times.sort_by(f64::total_cmp);
            println!("{stem}: draft slider tick, {name}: median {:.1} ms, min {:.1} ms", times[times.len() / 2], times[0]);
            s.execute("retouch.set", &json!({"control": control, "value": 0.0})).expect("set");
        }
        set(&mut s, 0.0);
    }
}
