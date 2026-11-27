//! One AI mask timed through the app's own path (issue #353): Session → `mask.add` → the SAM 3 worker, with the
//! model the Masking heading's dropdown picks. Masks the first photo (first use: the models load), the second right
//! after (second use), and so on, waiting the seconds `GAPS` gives before each mask (a mask after a pause). Every log line of the
//! models (loads, runs, the edge refinement) is printed with the wall and CPU seconds since the start, so the split of
//! one mask between its stages can be read from one run.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example mask_timing -- sam3+birefnet 0,0,75 PHOTO1 PHOTO2 PHOTO3`
//!
//! `LIGHTCRAFT_CONFIG_DIR` keeps the model choice out of the user's settings. `MASK_KIND=sky` (or
//! `background`) makes that kind of mask instead of Subject; `/usr/bin/time -l` before the command gives the peak
//! memory (issue #437).
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use serde_json::json;

static START: OnceLock<Instant> = OnceLock::new();

/// The process's CPU time in seconds, all threads.
fn cpu() -> f64 {
    #[cfg(unix)]
    {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
        t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
    }
    #[cfg(not(unix))]
    {
        0.0
    }
}

fn wall() -> f64 {
    START.get().map_or(0.0, |t| t.elapsed().as_secs_f64())
}

/// Prints the engine's info lines with the time they came.
struct Clock;

impl log::Log for Clock {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info && m.target().starts_with("lightcraft")
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            println!("{:8.2} s wall {:8.2} s CPU  {}", wall(), cpu(), r.args());
        }
    }

    fn flush(&self) {}
}

fn main() -> Result<(), String> {
    START.get_or_init(Instant::now);
    log::set_logger(&Clock).map_err(|e| e.to_string())?;
    log::set_max_level(log::LevelFilter::Info);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [model, gaps, photos @ ..] = args.as_slice() else {
        return Err("usage: mask_timing sam3|birefnet|sam3+birefnet GAPS PHOTO...".into());
    };
    let kind = std::env::var("MASK_KIND").unwrap_or_else(|_| "subject".into());
    let gaps: Vec<u64> = gaps.split(',').map(|g| g.trim().parse().map_err(|_| "GAPS is whole seconds, comma-separated")).collect::<Result<_, _>>()?;
    let mut s = Session::new().with_fs().with_default_sam3_model().with_default_parsing_models();
    s.segmenter.background = false;
    s.execute("segment.selectionModel", &json!({"model": model})).map_err(|e| e.to_string())?;
    let paths: Vec<String> = photos.iter().filter_map(|p| std::fs::canonicalize(p).ok()).map(|p| p.to_string_lossy().to_string()).collect();
    let r = s.execute("library.import", &json!({"paths": paths})).map_err(|e| e.to_string())?;
    let ids: Vec<PhotoId> = r["imported"].as_array().ok_or("no photos imported")?.iter().filter_map(|v| v.as_u64()).map(PhotoId).collect();
    for (i, id) in ids.iter().enumerate() {
        let gap = gaps.get(i).copied().unwrap_or(0);
        if gap > 0 {
            println!("{:8.2} s wall {:8.2} s CPU  -- waiting {gap} s", wall(), cpu());
            std::thread::sleep(Duration::from_secs(gap));
        }
        s.execute("library.select", &json!({"ids": [id.0]})).map_err(|e| e.to_string())?;
        s.execute("mask.deleteAll", &json!({})).ok();
        let (t, c) = (wall(), cpu());
        println!("{t:8.2} s wall {c:8.2} s CPU  -- mask {} starts ({})", i + 1, photos.get(i).map_or("", String::as_str));
        let made = s.execute("mask.add", &json!({"kind": kind}));
        let grid = s.develop_of(*id).and_then(|d| d.masks.first().cloned()).and_then(|m| match m.components.first().map(|c| c.shape.clone()) {
            Some(
                lightcraft_develop::MaskShape::Subject { seg: Some(g), .. }
                | lightcraft_develop::MaskShape::Sky { seg: Some(g), .. }
                | lightcraft_develop::MaskShape::Background { seg: Some(g), .. },
            ) => Some(format!("{}² {}", g.side, g.model.unwrap_or_default())),
            _ => None,
        });
        println!(
            "{:8.2} s wall {:8.2} s CPU  -- mask {} done in {:.2} s wall, {:.2} s CPU: {:?} {:?}",
            wall(),
            cpu(),
            i + 1,
            wall() - t,
            cpu() - c,
            grid,
            made.err().map(|e| e.to_string())
        );
    }
    Ok(())
}
