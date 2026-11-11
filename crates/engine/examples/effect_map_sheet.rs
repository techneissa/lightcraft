//! The ⌥-drag map of every Portrait slider (fork feature, `fork/portrait-effect-map.md`, issue #208), on real photos:
//! one sheet per slider, a row per photo cut round its first face: the photo with the slider at 100, then the map at
//! 25, 50 and 100. Then the draft frame time of an ⌥-drag tick against a plain drag tick.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --example effect_map_sheet -- OUT_DIR PHOTO...`
//!
//! `ONLY=skin,teethLips` keeps the sliders whose id holds one of those; `TICKS=0` skips the timing. Models are the
//! app's defaults (the shared models folder). Never touches the photo's own folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Instant;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::FEATURES;
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::media::develop;
use lightcraft_pipeline::{Overlay, Quality, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::{Rgb32f, Rgba8};
use serde_json::json;

/// The long edge the sheets are rendered at, and each cell's side.
const EDGE: usize = 1600;
const CELL: u32 = 360;

/// One photo, ready to render: its source, its faces and its first face's box in the render.
struct Photo {
    stem: String,
    src: Arc<Rgb32f>,
    info: SourceInfo,
    face: [f64; 4],
}

/// The sliders the sheets show: every Portrait slider but the lip colour's channels (they only tint Lip Color).
fn sliders() -> Vec<(u8, u8, &'static str, f64)> {
    let only = std::env::var("ONLY").unwrap_or_default();
    let mut out = Vec::new();
    for (fi, f) in FEATURES.iter().enumerate() {
        for (ci, spec) in (f.specs)().iter().enumerate() {
            let keep = only.is_empty() || only.split(',').any(|o| spec.id.contains(o));
            if keep && !spec.id.contains("lipColorR") && !spec.id.contains("lipColorG") && !spec.id.contains("lipColorB") && spec.max > spec.min {
                out.push((fi as u8, ci as u8, spec.id, spec.max));
            }
        }
    }
    out
}

/// `img` cut to `b` (pixels, a square round it) and fitted into a [`CELL`]-pixel square.
fn cell(img: &Rgba8, b: [f64; 4]) -> image::RgbImage {
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let side = (b[2] - b[0]).max(b[3] - b[1]);
    let (x0, y0) = ((cx - side / 2.0).max(0.0) as usize, (cy - side / 2.0).max(0.0) as usize);
    let (x1, y1) = (((cx + side / 2.0) as usize).min(img.width), ((cy + side / 2.0) as usize).min(img.height));
    let (w, h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
    let mut bytes = Vec::with_capacity(w * h * 3);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            bytes.extend_from_slice(&img.data[y.min(img.height - 1) * img.width + x.min(img.width - 1)][..3]);
        }
    }
    let crop = image::RgbImage::from_raw(w as u32, h as u32, bytes).expect("size");
    let k = f64::from(CELL) / w.max(h) as f64;
    let (fw, fh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
    let fitted = image::imageops::resize(&crop, fw, fh, image::imageops::FilterType::Lanczos3);
    let mut out = image::RgbImage::from_pixel(CELL, CELL, image::Rgb([24, 24, 24]));
    image::imageops::overlay(&mut out, &fitted, i64::from((CELL - fw) / 2), i64::from((CELL - fh) / 2));
    out
}

/// `BEFORE=1`: per photo, the view before issue #208 (the feature's area from the faces, as grey) and after (the
/// slider's change), at half the slider's top, as `before-after/<slider>-<photo>-{before,after}.png`.
fn before_after(out: &std::path::Path, photos: &[Photo], (feature, control, id, max): (u8, u8, &str, f64), name: &str) {
    let dir = out.join("before-after");
    std::fs::create_dir_all(&dir).expect("dir");
    let req = RenderRequest::fit(EDGE, EDGE);
    for p in photos {
        let overlay = Overlay::RetouchEffect { feature, control, face: Overlay::ALL_FACES };
        let after = develop(&p.src, &p.info, &settings(id, 0.5 * max), &RenderRequest { overlay, ..req }, None, false).image;
        let (w, h) = (after.width, after.height);
        let input = p.info.retouch.as_deref().expect("faces");
        let view = lightcraft_develop::retouch::WholePhoto { width: w, height: h };
        let scene = lightcraft_develop::retouch::Scene::new(input, &view, w, h);
        let faces: Vec<usize> = (0..scene.faces.len()).collect();
        let area = (FEATURES[feature as usize].area)(&scene, &faces);
        let grey = |i: usize| area.as_ref().and_then(|a| a.data.get(i)).map_or(0, |v| (255.0 * v.clamp(0.0, 1.0)).round() as u8);
        let before = Rgba8 { width: w, height: h, data: (0..w * h).map(|i| [grey(i), grey(i), grey(i), 255]).collect() };
        for (side, img) in [("before", &before), ("after", &after)] {
            cell(img, p.face).save(dir.join(format!("{name}-{}-{side}.png", p.stem))).expect("save");
        }
    }
}

fn settings(id: &str, v: f64) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.retouch.global.set(id, v);
    s
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: effect_map_sheet OUT_DIR PHOTO...");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models().with_default_lama_model();
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    let mut photos = Vec::new();
    for path in &args[1..] {
        let abs = std::fs::canonicalize(path).expect("photo").to_string_lossy().to_string();
        let stem = std::path::Path::new(path).file_stem().expect("stem").to_string_lossy().to_string();
        let r = s.execute("library.import", &json!({"paths": [abs]})).expect("import");
        let id = PhotoId(r["imported"][0].as_u64().expect("id"));
        s.execute("library.select", &json!({"ids": [id.0]})).expect("select");
        let t = Instant::now();
        s.execute("retouch.detect", &json!({"id": id.0})).expect("detect");
        let found = s.retouch_faces_of(id).found.expect("faces");
        println!("{stem}: {} faces in {:.1} s", found.input.faces.len(), t.elapsed().as_secs_f64());
        let Some(first) = found.input.faces.first() else { continue };
        let job = s.render_job(id, EDGE, EDGE, false, false).expect("job");
        let src = job.source.load().expect("source");
        let info = SourceInfo { retouch: Some(found.input.clone()), ..job.info.clone() };
        let req = RenderRequest::fit(EDGE, EDGE);
        let probe = develop(&src, &info, &DevelopSettings::default(), &req, None, false).image;
        let (w, h) = (probe.width as f64, probe.height as f64);
        let b = first.bounds;
        // the face, a little of the hair above it and the neck below
        let face = [(b.x0 - 0.2 * b.width()) * w, (b.y0 - 0.2 * b.height()) * h, (b.x1 + 0.2 * b.width()) * w, (b.y1 + 0.35 * b.height()) * h];
        photos.push(Photo { stem, src, info, face });
    }
    let req = RenderRequest::fit(EDGE, EDGE);
    for (feature, control, id, max) in sliders() {
        let cols = 4u32;
        let mut sheet = image::RgbImage::from_pixel(cols * CELL + (cols - 1) * 4, photos.len() as u32 * (CELL + 4), image::Rgb([0, 0, 0]));
        for (row, p) in photos.iter().enumerate() {
            // (Background works around the face: its rows show the whole photo)
            let whole = [0.0, 0.0, f64::INFINITY, f64::INFINITY];
            let area = if id.starts_with("retouch.background.") { whole } else { p.face };
            let mut cells = Vec::new();
            let photo = develop(&p.src, &p.info, &settings(id, max), &req, None, false).image;
            let area = if area[2].is_infinite() { [0.0, 0.0, photo.width as f64, photo.height as f64] } else { area };
            cells.push(cell(&photo, area));
            for v in [0.25, 0.5, 1.0] {
                let overlay = Overlay::RetouchEffect { feature, control, face: Overlay::ALL_FACES };
                let img = develop(&p.src, &p.info, &settings(id, v * max), &RenderRequest { overlay, ..req }, None, false).image;
                cells.push(cell(&img, area));
            }
            for (c, img) in cells.iter().enumerate() {
                image::imageops::overlay(&mut sheet, img, i64::from(c as u32 * (CELL + 4)), i64::from(row as u32 * (CELL + 4)));
            }
        }
        let name = id.trim_start_matches("retouch.").replace('.', "-");
        sheet.save(out.join(format!("{name}.jpg"))).expect("save");
        println!("wrote {name}.jpg");
        if std::env::var_os("BEFORE").is_some() {
            before_after(&out, &photos, (feature, control, id, max), &name);
        }
    }
    let ticks: usize = std::env::var("TICKS").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let Some(p) = photos.first().filter(|_| ticks > 0) else { return };
    let gpu = lightcraft_engine::gpu::available();
    let draft = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(1920, 1280) };
    for (id, base) in [("retouch.skin.smoothness", 40.0), ("retouch.teethLips.teethWhitening", 40.0), ("retouch.reshape.slimFace", 40.0)] {
        let feature = FEATURES.iter().position(|f| id.starts_with(&format!("retouch.{}.", f.key))).expect("feature") as u8;
        let control = (FEATURES[feature as usize].specs)().iter().position(|s| s.id == id).expect("control") as u8;
        let mut cols = Vec::new();
        for g in if gpu { vec![false, true] } else { vec![false] } {
            for alt in [false, true] {
                let cache = StageCache::default();
                let overlay = if alt { Overlay::RetouchEffect { feature, control, face: Overlay::ALL_FACES } } else { Overlay::None };
                let req = RenderRequest { overlay, ..draft };
                drop(develop(&p.src, &p.info, &settings(id, base), &req, Some(&cache), g));
                drop(develop(&p.src, &p.info, &settings(id, base), &req, Some(&cache), g));
                let mut times = Vec::new();
                for k in 0..ticks {
                    let at = Instant::now();
                    drop(develop(&p.src, &p.info, &settings(id, base + (k + 1) as f64), &req, Some(&cache), g));
                    times.push(at.elapsed().as_secs_f64() * 1e3);
                }
                times.sort_by(f64::total_cmp);
                cols.push(format!("{} {}: median {:.1} ms", if g { "gpu" } else { "cpu" }, if alt { "⌥" } else { "plain" }, times[times.len() / 2]));
            }
        }
        println!("{}: draft tick {id}: {}", p.stem, cols.join(" | "));
    }
}
