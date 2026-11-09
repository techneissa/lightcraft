//! Time the image encoder per photo: candle (GPU, CPU) against Core ML (all units; CPU + Neural Engine). Issue #105.
//!
//! `cargo run --release -p lightcraft-learn --example coreml_speed -- <model folder> [photo.jpg …]`
//!
//! The model folder is the shared one, `~/Library/Application Support/ArtCraft/models/dinov2-small` (`model.safetensors`,
//! `coreml/dinov2-small.mlpackage`; the first Core ML load compiles it there, as the app does). Without photos it uses five made-up
//! pictures (the network's cost does not depend on what the photo shows: every photo is squashed to 224 × 224).

use std::path::{Path, PathBuf};
use std::time::Instant;

use lightcraft_coreml::{ComputeUnits, Model};
use lightcraft_learn::ImageFeatures;
use lightcraft_learn::encoder::{Encoder, INPUT, pixels};
use lightcraft_learn::image_encoder::{ImageEncoder, Runtime, coreml_paths};

type Res<T> = Result<T, Box<dyn std::error::Error>>;
const ROUNDS: usize = 4;

fn made_up(n: usize) -> (usize, usize, Vec<u8>) {
    let (w, h) = (384usize, 256usize); // the preview size Learn My Edits takes features from
    let rgb = (0..w * h).flat_map(|i| {
        let (x, y) = (i % w, i / w);
        [(x * (n + 1) % 256) as u8, (y * 3 + n * 40) as u8, ((x + y) / (n + 2)) as u8]
    });
    (w, h, rgb.collect())
}

fn photos(args: &[String]) -> Res<Vec<(usize, usize, Vec<u8>)>> {
    if args.is_empty() {
        return Ok((0..5).map(made_up).collect());
    }
    args.iter()
        .map(|p| {
            let img = image::open(p)?.to_rgb8();
            Ok((img.width() as usize, img.height() as usize, img.into_raw()))
        })
        .collect()
}

/// Mean ms per photo over `ROUNDS` passes, after one warm-up photo.
fn per_photo(set: &[(usize, usize, Vec<u8>)], run: &dyn Fn(usize, usize, &[u8]) -> Res<Vec<f32>>) -> Res<f64> {
    if let Some((w, h, rgb)) = set.first() {
        run(*w, *h, rgb)?;
    }
    let t = Instant::now();
    for _ in 0..ROUNDS {
        for (w, h, rgb) in set {
            run(*w, *h, rgb)?;
        }
    }
    Ok(t.elapsed().as_secs_f64() * 1000.0 / (ROUNDS * set.len()) as f64)
}

fn timed<T>(f: impl FnOnce() -> Res<T>) -> Res<(T, f64)> {
    let t = Instant::now();
    let v = f()?;
    Ok((v, t.elapsed().as_secs_f64() * 1000.0))
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(args.first().ok_or("usage: coreml_speed <model folder> [photo …]")?);
    let set = photos(args.get(1..).unwrap_or(&[]))?;
    let weights = dir.join(lightcraft_learn::ENCODER_FILE);
    println!("| Runtime | Load (ms) | Per photo (ms) |\n| --- | ---: | ---: |");

    let (gpu, load) = timed(|| Ok(ImageEncoder::load(&dir, Runtime::Candle)?))?;
    let ms = per_photo(&set, &|w, h, rgb| Ok(gpu.features(w, h, rgb)?))?;
    println!("| candle, GPU (Metal) | {load:.0} | {ms:.1} |");
    drop(gpu);

    let (cpu, load) = timed(|| Ok(Encoder::load_cpu(&weights)?))?;
    let ms = per_photo(&set, &|w, h, rgb| Ok(cpu.features(w, h, rgb)?))?;
    println!("| candle, CPU | {load:.0} | {ms:.1} |");
    drop(cpu);

    let (_, compiled) = coreml_paths(&dir);
    let first = !compiled.exists();
    let (coreml, load) = timed(|| Ok(ImageEncoder::load(&dir, Runtime::CoreMl)?))?;
    let ms = per_photo(&set, &|w, h, rgb| Ok(coreml.features(w, h, rgb)?))?;
    let note = if first { " (first load compiles the model)" } else { "" };
    println!("| Core ML, all units{note} | {load:.0} | {ms:.1} |");
    drop(coreml);

    let (ne, load) = timed(|| Ok(Model::load(Path::new(&compiled), ComputeUnits::CpuAndNeuralEngine)?))?;
    let ms = per_photo(&set, &|w, h, rgb| Ok(ne.predict("pixels", &[1, 3, INPUT, INPUT], &pixels(w, h, rgb)?, "features")?.data))?;
    println!("| Core ML, CPU + Neural Engine | {load:.0} | {ms:.1} |");
    Ok(())
}
