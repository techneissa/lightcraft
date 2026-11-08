//! Print the sliders a Learn My Edits profile gives a photo.
//!
//! `cargo run --release -p lightcraft-learn --example apply -- <profile.lclearn> <photo.jpg> [--model <model.safetensors>]`
//!
//! The JPEG is the photo developed at its defaults.

use std::path::PathBuf;
use std::time::Instant;

use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_learn::{Profile, encoder::Encoder, photo_features};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let model = args.iter().position(|a| a == "--model").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let (Some(profile), Some(photo)) = (args.first(), args.get(1)) else {
        return Err("usage: apply <profile.lclearn> <photo.jpg> [--model path]".into());
    };
    let profile = Profile::read(std::path::Path::new(profile))?;
    let encoder = match &profile.features.encoder {
        Some(_) => {
            let path = model
                .or_else(|| std::env::var_os("LIGHTCRAFT_LEARN_MODEL").map(PathBuf::from))
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|h| PathBuf::from(h).join("Library/Application Support/ArtCraft/models/dinov2-small/model.safetensors"))
                })
                .ok_or("no model path")?;
            Some(Encoder::load(&path)?)
        }
        None => None,
    };
    let img = image::open(photo)?.to_rgb8();
    let t = Instant::now();
    let f = photo_features(encoder.as_ref(), img.width() as usize, img.height() as usize, img.as_raw())?;
    let defaults = DevelopSettings::default();
    let out = lightcraft_learn::apply(&profile, &f, &defaults, &defaults)?;
    eprintln!("applied in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    for c in controls::CONTROLS {
        let (a, b) = (controls::get(&out, c.id).unwrap_or(0.0), controls::get(&defaults, c.id).unwrap_or(0.0));
        if (a - b).abs() > 1e-6 {
            println!("{:<24} {}", c.id, c.format(a));
        }
    }
    for (name, pts) in
        [("curve.master", &out.curve.master), ("curve.red", &out.curve.red), ("curve.green", &out.curve.green), ("curve.blue", &out.curve.blue)]
    {
        if !pts.is_empty() {
            let p: Vec<String> = pts.iter().map(|p| format!("({:.2}, {:.3})", p.x, p.y)).collect();
            println!("{name:<24} {}", p.join(" "));
        }
    }
    Ok(())
}
