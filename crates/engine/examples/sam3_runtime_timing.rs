//! SAM 3 through the app's own path (Session → the SAM 3 worker) on one engine, timed (issue #268): the photo ready
//! for clicks (the model's load and the photo's analysis), Core ML's warm-up in the background, a click, a second
//! click, the first prompt on the photo and the next one. One process per engine, so its peak memory is its own:
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> /usr/bin/time -l cargo run -p lightcraft-engine --features sam --example sam3_runtime_timing -- coreml|candle PHOTO`
//!
//! The SAM 3 folder is the app's default; `LIGHTCRAFT_CONFIG_DIR` keeps the runtime choice out of the user's settings.
use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::segment::Click;
use lightcraft_geom::Point;
use serde_json::json;

fn secs(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1000.0).round() / 1000.0
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [runtime, photo] = args.as_slice() else { return Err("usage: sam3_runtime_timing coreml|candle PHOTO".into()) };
    let mut s = Session::new().with_fs().with_default_sam3_model();
    s.segmenter.background = false;
    s.execute("models.runtime", &json!({"model": "sam3", "runtime": runtime})).map_err(|e| e.to_string())?;
    let path = std::fs::canonicalize(photo).map_err(|e| e.to_string())?.to_string_lossy().to_string();
    let r = s.execute("library.import", &json!({"paths": [path]})).map_err(|e| e.to_string())?;
    let id = r["imported"].as_array().and_then(|a| a.first()).and_then(|v| v.as_u64()).map(PhotoId).ok_or("no photo imported")?;
    let click = |x: f64, y: f64, include: bool| Click { at: Point::new(x, y), include };

    let t = Instant::now();
    s.segment_prepare(id)?;
    let ready = secs(t);
    let t = Instant::now();
    while s.activity.list().iter().any(|r| r.kind == "sam3-warm-up") {
        std::thread::sleep(Duration::from_millis(20));
    }
    let warm_up = secs(t);
    let t = Instant::now();
    s.segment_clicks(id, &[click(0.45, 0.55, true)])?;
    let first_click = secs(t);
    let t = Instant::now();
    s.segment_clicks(id, &[click(0.45, 0.55, true), click(0.85, 0.6, false)])?;
    let next_click = secs(t);
    let t = Instant::now();
    let first = s.segment_text(id, "dog")?.is_some();
    let first_prompt = secs(t);
    let t = Instant::now();
    let next = s.segment_text(id, "grass")?.is_some();
    let next_prompt = secs(t);
    let report = json!({
        "runtime": runtime,
        "backend": s.segmenter.sam3_backend(),
        "photo_ready_s": ready,
        "warm_up_after_ready_s": warm_up,
        "click_s": first_click,
        "second_click_s": next_click,
        "first_prompt_s": first_prompt,
        "first_prompt_matched": first,
        "next_prompt_s": next_prompt,
        "next_prompt_matched": next,
    });
    println!("{report}");
    Ok(())
}
