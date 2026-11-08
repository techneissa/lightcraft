//! Train a Learn My Edits profile from a folder of `name.jpg` + `name.json` pairs and print its accuracy.
//!
//! `cargo run --release -p lightcraft-learn --example train -- <folder> <profile.lclearn> [--groups tone,whiteBalance]
//! [--no-encoder] [--model <model.safetensors>]`
//!
//! Each JPEG is a photo developed at its defaults; its JSON is the edit, as develop settings (a partial object is
//! merged onto the defaults). Without `--model`, the encoder is read from `LIGHTCRAFT_LEARN_MODEL` or the shared
//! models folder; `--no-encoder` uses the photo statistics only.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lightcraft_develop::DevelopSettings;
use lightcraft_learn::{Example, Features, Group, Profile, encoder::Encoder, layout, photo_features, sliders};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

pub fn model_path(arg: Option<&str>) -> Option<PathBuf> {
    arg.map(PathBuf::from)
        .or_else(|| std::env::var_os("LIGHTCRAFT_LEARN_MODEL").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/ArtCraft/models/dinov2-small/model.safetensors")))
}

fn load_rgb(path: &Path) -> Res<(usize, usize, Vec<u8>)> {
    let img = image::open(path)?.to_rgb8();
    Ok((img.width() as usize, img.height() as usize, img.into_raw()))
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let (mut groups, mut no_encoder, mut model) = (Group::ALL.to_vec(), false, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--groups" => {
                i += 1;
                groups = args.get(i).ok_or("--groups needs a list")?.split(',').filter_map(Group::from_id).collect();
            }
            "--no-encoder" => no_encoder = true,
            "--model" => {
                i += 1;
                model = args.get(i).cloned();
            }
            a => pos.push(a.to_string()),
        }
        i += 1;
    }
    let [folder, out] = &pos[..] else { return Err("usage: train <folder> <profile.lclearn> [--groups a,b] [--no-encoder] [--model path]".into()) };
    let encoder = if no_encoder {
        None
    } else {
        let path = model_path(model.as_deref()).ok_or("no model path")?;
        let t = Instant::now();
        let e = Encoder::load(&path)?;
        eprintln!("encoder loaded in {:.2} s (GPU: {})", t.elapsed().as_secs_f64(), e.on_gpu());
        Some(e)
    };
    let mut profile = Profile::new(Path::new(out).file_stem().and_then(|s| s.to_str()).unwrap_or("Profile"), "", Features { encoder: encoder.as_ref().map(|_| lightcraft_learn::ENCODER_ID.to_string()), layout: layout(encoder.is_some()) }, &groups);
    let mut jpgs: Vec<PathBuf> = std::fs::read_dir(folder)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg"))).collect();
    jpgs.sort();
    let defaults = DevelopSettings::default();
    let t = Instant::now();
    for jpg in &jpgs {
        let json = jpg.with_extension("json");
        let Ok(text) = std::fs::read_to_string(&json) else { continue };
        let edit = defaults.merged(&serde_json::from_str(&text)?)?;
        let (w, h, rgb) = load_rgb(jpg)?;
        let features = photo_features(encoder.as_ref(), w, h, &rgb)?;
        let name = jpg.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        profile.add_example(Example { photo: name, features, targets: sliders::targets(&edit, &defaults) })?;
    }
    let n = profile.examples.len();
    let feat = t.elapsed().as_secs_f64();
    eprintln!("{n} examples, features in {feat:.2} s ({:.0} ms each)", feat * 1000.0 / n.max(1) as f64);
    let t = Instant::now();
    let report = profile.train(&|| false, &mut |_, _| {}).cloned();
    eprintln!("trained in {:.2} s", t.elapsed().as_secs_f64());
    profile.write(Path::new(out))?;
    let report = report?;
    println!(
        "held out: closer than defaults on {}/{} photos, than the average edit on {}/{}; error {:.0} % of the defaults'",
        report.beats_defaults,
        report.examples,
        report.beats_preset,
        report.examples,
        report.ratio * 100.0
    );
    for g in &report.groups {
        println!("{} — {} — error {:.0} % of the defaults', a fixed preset {:.0} %", g.group.label(), if g.kept { "kept" } else { "dropped" }, g.ratio * 100.0, g.preset_ratio * 100.0);
        for s in &g.sliders {
            println!("    {:<24} error {:>8.2}   defaults {:>8.2}", s.id, s.error, s.default_error);
        }
    }
    if let Some(m) = &profile.model {
        for g in &m.groups {
            println!("{}: encoder weight {}, {}", g.group.label(), g.kernel.encoder_weight, if g.kernel.rbf.is_some() { "gaussian" } else { "linear" });
        }
    }
    Ok(())
}
