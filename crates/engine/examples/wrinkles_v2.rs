//! Wrinkles before and after (fork feature, `fork/retouch-wrinkles.md`): for each photo, the face search with the
//! wrinkle search (LaMa when installed), then the same face with no Wrinkles, with the old Wrinkles at 100 and with the
//! new ones at 100 and 50, and the seconds each step took.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --example wrinkles_v2 -- OUT_DIR PHOTO...`
//!
//! Writes, per photo, `OUT_DIR/<stem>-{before,v1,v2-100,v2-50}.png` at 2048 px and, cut round the first face's forehead at
//! 1:1, `OUT_DIR/<stem>-forehead-*.png`; prints the times. Models are the app's defaults (the shared models folder);
//! `NO_LAMA=1` runs the banded heal. Never touches the photo's own folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Instant;

use lightcraft_develop::retouch::Input;
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_pipeline::{Quality, RenderRequest, StageCache};
use lightcraft_raster::Rgba8;
use serde_json::json;

const EDGE: usize = 2048;

fn save(img: &Rgba8, crop: Option<[usize; 4]>, path: &std::path::Path) {
    let [x0, y0, x1, y1] = crop.unwrap_or([0, 0, img.width, img.height]);
    let (w, h) = (x1 - x0, y1 - y0);
    let mut bytes = Vec::with_capacity(w * h * 3);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.data[y * img.width + x];
            bytes.extend_from_slice(&p[..3]);
        }
    }
    image::RgbImage::from_raw(w as u32, h as u32, bytes).expect("size").save(path).expect("save");
}

fn render(s: &mut Session, id: PhotoId, faces: Option<Arc<Input>>) -> Rgba8 {
    let mut job = s.render_job(id, EDGE, EDGE, false, false).expect("job");
    if let Some(f) = faces {
        job.info.retouch = Some(f);
    }
    job.run().rendered.expect("render").image
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: wrinkles_v2 OUT_DIR PHOTO...");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models().with_default_lama_model();
    if std::env::var_os("NO_LAMA").is_some() {
        s.lama.set_dir(None);
    }
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    println!("LaMa installed: {}", s.lama.installed());
    for path in &args[1..] {
        let abs = std::fs::canonicalize(path).expect("photo").to_string_lossy().to_string();
        let stem = std::path::Path::new(path).file_stem().expect("stem").to_string_lossy().to_string();
        let r = s.execute("library.import", &json!({"paths": [abs]})).expect("import");
        let id = PhotoId(r["imported"][0].as_u64().expect("id"));
        s.execute("library.select", &json!({"ids": [id.0]})).expect("select");
        // the people parse, the spots and strands, and the wrinkle search
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
            let cells = f.wrinkles.as_ref().map(|w| w.width * w.height);
            println!("{stem}: face {i}: box {:?}, {} landmarks, {} masks, stored change cells {cells:?}", f.bounds, f.landmarks.len(), f.masks.len());
        }
        let Some(face) = found.input.faces.first() else { continue };
        let old = {
            let mut faces = found.input.faces.clone();
            faces.iter_mut().for_each(|f| f.wrinkles = None);
            Arc::new(Input::new_on_photo(faces))
        };
        let set = |s: &mut Session, v: f64| {
            for c in ["forehead", "crowsFeet", "smileLines", "neck"] {
                s.execute("retouch.set", &json!({"control": format!("retouch.wrinkles.{c}"), "value": v})).expect("set");
            }
        };
        set(&mut s, 0.0);
        let before = render(&mut s, id, None);
        set(&mut s, 100.0);
        let v1 = render(&mut s, id, Some(old.clone()));
        let v2 = render(&mut s, id, None);
        set(&mut s, 50.0);
        let v2_half = render(&mut s, id, None);
        // the forehead at 1:1: the face's box, from a little above it to a third of the way down
        let b = face.bounds;
        let (w, h) = (before.width as f64, before.height as f64);
        let crop = [
            ((b.x0 - 0.02) * w).max(0.0) as usize,
            ((b.y0 - 0.1 * b.height()) * h).max(0.0) as usize,
            (((b.x1 + 0.02) * w) as usize).min(before.width),
            (((b.y0 + 0.33 * b.height()) * h) as usize).min(before.height),
        ];
        for (name, img) in [("before", &before), ("v1", &v1), ("v2-100", &v2), ("v2-50", &v2_half)] {
            save(img, None, &out.join(format!("{stem}-{name}.png")));
            save(img, Some(crop), &out.join(format!("{stem}-forehead-{name}.png")));
            let c = [
                (b.x0 * w).max(0.0) as usize,
                ((b.y0 - 0.15 * b.height()) * h).max(0.0) as usize,
                ((b.x1 * w) as usize).min(img.width),
                (((b.y1 + 0.3 * b.height()) * h) as usize).min(img.height),
            ];
            save(img, Some(c), &out.join(format!("{stem}-face-{name}.png")));
        }
        // a slider tick: a draft render with the stage cache warm, one slider moving
        let ticks: [(&str, &str, f64, bool); 3] = [
            ("Wrinkles Forehead, the stored change", "retouch.wrinkles.forehead", 60.0, false),
            ("Wrinkles Forehead, the old Wrinkles", "retouch.wrinkles.forehead", 60.0, true),
            ("Skin Smoothness, for comparison", "retouch.skin.smoothness", 40.0, false),
        ];
        for (name, control, base, old_faces) in ticks {
            let cache = Arc::new(StageCache::default());
            let mut times = Vec::new();
            for k in 0..14 {
                s.execute("retouch.set", &json!({"control": control, "value": base + k as f64})).expect("set");
                let mut job = s.render_job(id, 1920, 1280, false, false).expect("job");
                job.request = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(1920, 1280) };
                job.stages = Some(cache.clone());
                if old_faces {
                    job.info.retouch = Some(old.clone());
                }
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
