//! What parsing ahead does while the user culls (issue #200, `fork/parse-ahead.md`): steps through the photos as the
//! arrow keys do, pumping the queue as the app does (paused for 1.5 s after each key, then at the light pace), and
//! counts the model runs and the CPU time the process spent.
//!
//! ```text
//! LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --example parse_cull -- OUT_DIR \
//!     [--every SECONDS] [--edit] PHOTO...
//! ```
//!
//! Imports the photos into a library in `OUT_DIR/library` (in place: give it copies) and finds their faces (YuNet) first.
//! Without `--edit` the steps are made outside the Edit view (`culling`); with it, in the Edit view, no photo edited.

use std::time::{Duration, Instant};

use lightcraft_engine::{Selection, Session};
use serde_json::json;

/// Process CPU time in ms (all threads); 0 where the platform has no such clock (Windows).
fn cpu_ms() -> f64 {
    #[cfg(unix)]
    {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
        t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
    }
    #[cfg(not(unix))]
    {
        0.0
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let edit = args.iter().position(|a| a == "--edit").map(|i| args.remove(i)).is_some();
    let every = match args.iter().position(|a| a == "--every") {
        Some(i) => {
            let v: f64 = args.get(i + 1).and_then(|v| v.parse().ok()).ok_or("--every SECONDS")?;
            args.drain(i..i + 2);
            v
        }
        None => 2.0,
    };
    let (out, photos) = args.split_first().ok_or("usage: parse_cull OUT_DIR [--every SECONDS] [--edit] PHOTO...")?;
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models();
    s.parsing.set_paths(s.parsing.models_dir(), Some(out.join("parse-cache")), None);
    s.open_library(out.join("library"), false)?;
    s.execute("library.import", &json!({ "paths": photos }))?;
    let view = s.visible_cloned();
    let ids: Vec<u64> = view.iter().map(|id| id.0).collect();
    s.execute("faces.detect", &json!({ "ids": ids }))?;
    let culling = !edit;
    let quiet = Duration::from_secs_f64(1.5_f64.min(every));
    let step = Duration::from_secs_f64(every);
    let (t, c, runs) = (Instant::now(), cpu_ms(), s.parsing.runs());
    for id in &view {
        s.selection = Selection::single(*id);
        let at = Instant::now();
        while at.elapsed() < step {
            let pace = if at.elapsed() < quiet { "pause" } else { "light" };
            s.execute("parsing.ahead.pump", &json!({ "pace": pace, "culling": culling }))?;
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    // (the walk ends here: a run still going is counted as started, its time so far as spent)
    let st = s.execute("parsing.ahead.pump", &json!({ "pace": "pause", "culling": culling }))?;
    println!(
        "{} photos, a step every {every} s, {}: {} model runs started, {} finished, {:.1} s wall, {:.1} s CPU (the face scan done before)",
        view.len(),
        if culling { "culling" } else { "in the Edit view, no edit" },
        s.parsing.runs() - runs,
        st["parsed"],
        t.elapsed().as_secs_f64(),
        (cpu_ms() - c) / 1e3
    );
    Ok(())
}
