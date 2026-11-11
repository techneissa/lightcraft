//! The cost of parsing a photo ahead of time (issue #200, `fork/parse-ahead.md`), by step, in wall and CPU time:
//!
//! ```text
//! LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features test-hooks \
//!     --example parse_profile -- OUT_DIR [--set segformer|bisenet|sapiens2] [--rounds N] PHOTO...
//! ```
//!
//! Imports the photos into a library in `OUT_DIR/library` (in place: give it copies) and parses each one as the
//! parse-ahead worker does (the render at 2048 px, YuNet, the models, the cache write), `--rounds` times (2 by
//! default; the parse cache is emptied before each round, so every round runs the models). The first round holds the
//! models' load and Core ML's first call per input size. The steps are the marks of `lightcraft_parsing::profile`:
//! the time between two marks belongs to the later one. CPU time is the whole process's (every thread), so run it
//! with nothing else in the process. The models come from the shared models folder.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;

use lightcraft_engine::Session;
use serde_json::json;

struct Clock {
    wall: Instant,
    cpu: f64,
    /// Per step: (wall ms, CPU ms, times).
    steps: BTreeMap<&'static str, (f64, f64, u32)>,
    order: Vec<&'static str>,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);

fn cpu_ms() -> f64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
    t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
}

fn mark(stage: &'static str) {
    let (now, cpu) = (Instant::now(), cpu_ms());
    let mut c = CLOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(c) = c.as_mut() {
        let e = c.steps.entry(stage).or_insert((0.0, 0.0, 0));
        e.0 += (now - c.wall).as_secs_f64() * 1e3;
        e.1 += cpu - c.cpu;
        e.2 += 1;
        if !c.order.contains(&stage) {
            c.order.push(stage);
        }
        c.wall = now;
        c.cpu = cpu;
    }
}

fn start() {
    *CLOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(Clock { wall: Instant::now(), cpu: cpu_ms(), steps: BTreeMap::new(), order: Vec::new() });
}

fn take() -> Option<Clock> {
    CLOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
}

fn load_average() -> String {
    std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .map_or_else(|_| "?".into(), |o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut flag = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        let v = args.get(i + 1).cloned();
        args.drain(i..(i + 2).min(args.len()));
        v
    };
    let set = flag("--set");
    let rounds: usize = flag("--rounds").and_then(|v| v.parse().ok()).unwrap_or(2);
    let (out, photos) = args.split_first().ok_or("usage: parse_profile OUT_DIR [--set S] [--rounds N] PHOTO...")?;
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    lightcraft_parsing::profile::on_mark(mark);
    let mut s = Session::new().with_fs().with_default_face_models().with_default_parsing_models();
    let models = s.parsing.models_dir();
    s.parsing.set_paths(models.clone(), Some(out.join("parse-cache")), None);
    if let Some(set) = set.as_deref() {
        s.execute("parsing.modelSet", &json!({ "set": set }))?;
    }
    s.open_library(out.join("library"), false)?;
    s.execute("library.import", &json!({ "paths": photos }))?;
    let view = s.visible_cloned();
    let names: BTreeMap<u64, String> = view.iter().filter_map(|id| s.catalog.photo(*id).map(|p| (id.0, p.file_name.clone()))).collect();
    println!("set {}, backends {}, load average {}", s.parsing.model_set().id(), backends(&s), load_average());
    for round in 0..rounds {
        let _ = std::fs::remove_dir_all(out.join("parse-cache"));
        s.parsing.set_paths(models.clone(), Some(out.join("parse-cache")), None);
        if let Some(set) = set.as_deref() {
            s.execute("parsing.modelSet", &json!({ "set": set }))?;
        }
        if round > 0 {
            // (the models stay loaded from the first round: `set_paths` dropped them, so load them once untimed)
            if let Some(first) = view.first() {
                s.parse_ahead_one(*first)?;
                s.parsing.forget_kept();
                let _ = std::fs::remove_dir_all(out.join("parse-cache"));
                std::fs::create_dir_all(out.join("parse-cache"))?;
            }
        }
        let mut totals: BTreeMap<&'static str, (f64, f64, u32)> = BTreeMap::new();
        let mut order: Vec<&'static str> = Vec::new();
        for id in &view {
            s.parsing.forget_kept();
            start();
            let (t, c) = (Instant::now(), cpu_ms());
            let ran = s.parse_ahead_one(*id);
            mark("return");
            let (wall, cpu) = (t.elapsed().as_secs_f64() * 1e3, cpu_ms() - c);
            let Some(clock) = take() else { continue };
            let people =
                s.execute("parsing.run", &json!({ "ids": [id.0], "wait": true }))?["photos"][0]["parse"]["people"].as_array().map_or(0, Vec::len);
            println!(
                "round {round} {} ({}): {:?}, {people} people, {wall:.0} ms wall, {cpu:.0} ms CPU, load {}",
                names.get(&id.0).map_or("?", String::as_str),
                id.0,
                ran,
                load_average()
            );
            for k in &clock.order {
                if let Some((w, c, n)) = clock.steps.get(k) {
                    println!("    {k:<14} {w:>7.1} ms wall {c:>8.1} ms CPU ({n}×)");
                    let e = totals.entry(k).or_insert((0.0, 0.0, 0));
                    e.0 += w;
                    e.1 += c;
                    e.2 += 1;
                    if !order.contains(k) {
                        order.push(k);
                    }
                }
            }
        }
        let n = view.len().max(1) as f64;
        println!("round {round}: mean per photo over {} photos", view.len());
        for k in &order {
            if let Some((w, c, _)) = totals.get(k) {
                println!("    {k:<14} {:>7.1} ms wall {:>8.1} ms CPU", w / n, c / n);
            }
        }
    }
    Ok(())
}

fn backends(s: &Session) -> String {
    lightcraft_engine::people_parsing::COREML_MODELS
        .iter()
        .map(|m| format!("{m}={}", s.parsing.backend(m).unwrap_or("-")))
        .collect::<Vec<_>>()
        .join(" ")
}
