//! Describe (a mask from typed words, SAM 3) over a fixed list of prompts, through the app's own path (issue #439):
//! Session → `segment_text` → the SAM 3 worker. For each prompt it prints the outcome (the share of the photo
//! selected, nothing found, or the error the user would read) after the model's own lines, which give each phrase's
//! best score and the threshold.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example describe_prompts -- LIST`
//!
//! `LIST` has one photo per line, `PHOTO | prompt; prompt; …` (prompts may hold commas: Describe's own phrase list).
//! `BACKGROUND=1` runs each prompt as the app does (`mask.add`, then polled until done, printing the messages the app
//! would show); `SAM3_RUNTIME=candle` (or `coreml`) picks SAM 3's engine; `PAUSE=<seconds>` waits that long before each photo (the model is let go after a minute unused).
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use serde_json::json;

/// Prints the model's lines (its debug lines give the scores).
struct Lines;

impl log::Log for Lines {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("lightcraft") && (m.level() <= log::Level::Info || m.target().starts_with("lightcraft_segment"))
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            println!("    {}", r.args());
        }
    }

    fn flush(&self) {}
}

fn main() -> Result<(), String> {
    log::set_logger(&Lines).map_err(|e| e.to_string())?;
    log::set_max_level(log::LevelFilter::Debug);
    let list = std::env::args().nth(1).ok_or("usage: describe_prompts LIST")?;
    let list = std::fs::read_to_string(&list).map_err(|e| format!("{list}: {e}"))?;
    let mut s = Session::new().with_fs().with_default_sam3_model();
    if let Ok(runtime) = std::env::var("SAM3_RUNTIME") {
        s.execute("models.runtime", &json!({"model": "sam3", "runtime": runtime})).map_err(|e| e.to_string())?;
    }
    let background = std::env::var_os("BACKGROUND").is_some();
    s.segmenter.background = background;
    let pause: u64 = std::env::var("PAUSE").ok().and_then(|p| p.parse().ok()).unwrap_or(0);
    for line in list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((photo, prompts)) = line.split_once('|') else { continue };
        let path = std::fs::canonicalize(photo.trim()).map_err(|e| format!("{photo}: {e}"))?;
        let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).map_err(|e| e.to_string())?;
        let id = r["imported"].as_array().and_then(|a| a.first()).and_then(serde_json::Value::as_u64).map(PhotoId);
        let Some(id) = id else { return Err(format!("{photo}: not imported")) };
        if pause > 0 {
            println!("  -- waiting {pause} s");
            std::thread::sleep(std::time::Duration::from_secs(pause));
        }
        s.execute("library.select", &json!({"ids": [id.0]})).map_err(|e| e.to_string())?;
        println!("== {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
        for prompt in prompts.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            println!("  -- “{prompt}”");
            if background {
                s.execute("mask.deleteAll", &json!({})).ok();
                let started = s.execute("mask.add", &json!({"kind": "prompt", "text": prompt}));
                let mut said = Vec::new();
                let mut made = false;
                // (until something is made or said; a request that ends silently counts 10 s of an idle worker)
                let mut idle_polls = 0;
                while started.is_ok() && !made && said.is_empty() && idle_polls < 200 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let polled = s.segment_poll();
                    said.extend(polled.messages);
                    made |= polled.changed;
                    idle_polls = if s.segmenter.busy() { 0 } else { idle_polls + 1 };
                }
                let masks = s.develop_of(id).map_or(0, |d| d.masks.len());
                println!("  => “{prompt}”: start {:?}, masks {masks}, said {said:?}", started.err().map(|e| e.to_string()));
                continue;
            }
            let outcome = match s.segment_text(id, prompt) {
                Ok(Some(seg)) => {
                    let l = seg.logits().unwrap_or_default();
                    let cells = l.iter().filter(|v| **v > 0.0).count();
                    format!("selected {:.1}% of the photo ({cells} of {} cells)", cells as f64 * 100.0 / l.len().max(1) as f64, l.len())
                }
                Ok(None) => "nothing found".into(),
                Err(e) => format!("error: {e}"),
            };
            println!("  => “{prompt}”: {outcome}");
        }
    }
    Ok(())
}
