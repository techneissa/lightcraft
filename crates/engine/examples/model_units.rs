//! Issue #267: measure every AI model with a Core ML version in the shared models folder on each of its units, as
//! the app does in the background, and print the table (nothing is saved).
//!
//! `cargo run --release -p lightcraft-engine --features sam --example model_units [model id…]`

use lightcraft_engine::model_units::{Outcome, chosen, probes};

fn main() {
    let Some(models) = lightcraft_engine::people_parsing::default_models_dir() else {
        eprintln!("no shared models folder");
        return;
    };
    let only: Vec<String> = std::env::args().skip(1).collect();
    let mut specs = vec![probes::learn(&models.join(lightcraft_learn::ENCODER_ID))];
    specs.extend(lightcraft_engine::people_parsing::COREML_MODELS.iter().filter_map(|m| probes::parsing(&models, m)));
    specs.push(probes::birefnet(&models.join(lightcraft_engine::model_runtime::BIREFNET), false));
    specs.push(probes::birefnet(&models.join(lightcraft_engine::hair_edges::MODEL_DIR), true));
    println!("| Model | Option | Load | Run | Outcome |");
    println!("|---|---|---|---|---|");
    for spec in specs {
        let model = spec.model;
        if !only.is_empty() && !only.iter().any(|m| m == model) {
            continue;
        }
        if !spec.coreml_ready() {
            println!("| {model} | (no Core ML version here) | | | |");
            continue;
        }
        let options = match probes::measure_now(spec) {
            Ok(o) => o,
            Err(e) => {
                println!("| {model} | {e} | | | |");
                continue;
            }
        };
        let best = chosen(&options).map(|(u, _)| u.id());
        for t in &options {
            let secs = |v: Option<u64>| v.map_or("–".to_string(), |ms| format!("{:.2} s", ms as f64 / 1000.0));
            let ms = |v: Option<f64>| v.map_or("–".to_string(), |ms| format!("{ms:.1} ms"));
            let outcome = match t.outcome {
                Outcome::Ok if Some(t.unit.as_str()) == best => "**chosen**".to_string(),
                Outcome::Ok => "ok".to_string(),
                Outcome::Mismatch => "output differs".to_string(),
                Outcome::LoadCap => "load over the cap".to_string(),
                Outcome::Failed => format!("failed: {}", t.error.as_deref().unwrap_or("")),
            };
            println!("| {model} | {} | {} | {} | {outcome} |", t.unit, secs(t.load_ms), ms(t.run_ms));
        }
    }
    // (a load left past the cap keeps its thread; end without waiting for it)
    std::process::exit(0);
}
