//! Opening a portrait and exporting it with every Portrait slider on (fork, issue #241): the face search by step, the
//! first render of the loupe's 2560 px preview (cold: no stage cache, no kept retouch work) and a full-size export
//! render, on the GPU and the CPU.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> LIGHTCRAFT_PROFILE=1 cargo run --release -p lightcraft-engine --example portrait_open_profile -- OUT_DIR PHOTO`
//!
//! `LIGHTCRAFT_PROFILE=1` prints each search step, each feature's run (`retouch <feature>`) and the GPU's copies
//! (`gpu retouch on the CPU`). `GPU_ONLY=1` / `LIGHTCRAFT_GPU=0` pick one renderer; `NO_EXPORT=1` skips the export;
//! `PARSE_FIRST=1` parses the photo before the search, as parsing ahead does (the search then reads the parse from the
//! index).
//! Models are the app's defaults (the shared models folder); the parse cache is a new folder in `OUT_DIR`, so the
//! parse runs. Never touches the photo's own folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::FEATURES;
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::media::develop;
use lightcraft_pipeline::{RenderRequest, SourceInfo, StageCache};
use serde_json::json;

/// Every Portrait slider at half its top (the lip colour's channels left alone: they only tint Lip Color); with
/// `background` false, Background's left at 0 (every other feature writes only near its face).
fn every_slider_on(background: bool) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    for f in FEATURES.iter().filter(|f| background || f.key != "background") {
        for spec in (f.specs)() {
            if spec.max > 0.0 && !spec.id.contains("lipColor") {
                s.retouch.global.set(spec.id, 0.5 * spec.max);
            }
        }
    }
    s
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() == 2, "usage: portrait_open_profile OUT_DIR PHOTO");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models().with_default_lama_model();
    // (`KEEP_PARSE=1`: one parse cache for every run, so only the first runs the parse)
    let cache = if std::env::var_os("KEEP_PARSE").is_some() { "parse-cache".to_string() } else { format!("parse-cache-{nanos}") };
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join(cache)), None);
    let abs = std::fs::canonicalize(&args[1]).expect("photo").to_string_lossy().to_string();
    let r = s.execute("library.import", &json!({"paths": [abs]})).expect("import");
    let id = PhotoId(r["imported"][0].as_u64().expect("id"));
    s.execute("library.select", &json!({"ids": [id.0]})).expect("select");
    // (`PARSE_FIRST=1`: the photo parsed before the search, as parsing ahead does: the search reads it from the index)
    if std::env::var_os("PARSE_FIRST").is_some() {
        s.execute("parsing.run", &json!({"ids": [id.0], "wait": true})).expect("parse");
    }
    eprintln!("--- face search");
    let t = Instant::now();
    s.execute("retouch.detect", &json!({"id": id.0})).expect("detect");
    let found = s.retouch_faces_of(id).found.expect("faces");
    println!("face search: {} faces, {:.0} ms", found.input.faces.len(), t.elapsed().as_secs_f64() * 1e3);
    let gpu = lightcraft_engine::gpu::available();
    let backends: Vec<bool> = match (gpu, std::env::var_os("GPU_ONLY").is_some()) {
        (true, true) => vec![true],
        (true, false) => vec![true, false],
        _ => vec![false],
    };
    let mut sizes = vec![("open (2560 px preview)", 2560usize)];
    if std::env::var_os("NO_EXPORT").is_none() {
        sizes.push(("export (full size)", 0));
    }
    let edits = [("every slider", every_slider_on(true)), ("every slider but Background", every_slider_on(false))];
    for ((what, edge), (edit, settings)) in sizes.iter().flat_map(|s| edits.iter().map(move |e| (*s, e))) {
        let (w, h) = if edge == 0 { (100_000, 100_000) } else { (edge, edge) };
        let job = s.render_job(id, w, h, false, true).expect("job");
        let src = job.source.load().expect("source");
        let info = SourceInfo { retouch: Some(found.input.clone()), ..job.info.clone() };
        // (the export at the source's own size)
        let req = if edge == 0 { RenderRequest::fit(src.width, src.height) } else { RenderRequest::fit(w, h) };
        for &g in &backends {
            let label = if g { "gpu" } else { "cpu" };
            lightcraft_develop::retouch::forget_kept();
            let cache = StageCache::default();
            eprintln!("--- {what}, {edit}, {label}, first render");
            let t = Instant::now();
            let img = develop(&src, &info, settings, &req, Some(&cache), g).image;
            let first = t.elapsed().as_secs_f64() * 1e3;
            eprintln!("--- {what}, {edit}, {label}, second render");
            let t = Instant::now();
            drop(develop(&src, &info, settings, &req, Some(&cache), g));
            let second = t.elapsed().as_secs_f64() * 1e3;
            println!(
                "{what}, {edit}, {label}: source {}×{}, out {}×{}, first {first:.0} ms, again {second:.0} ms",
                src.width, src.height, img.width, img.height
            );
        }
    }
}
