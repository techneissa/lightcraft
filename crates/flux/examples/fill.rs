//! Repaint the masked part of a picture with FLUX.1 Fill.
//!
//! ```text
//! cargo run --release -p lightcraft-flux --example fill -- WEIGHTS_DIR IN MASK OUT [--prompt TEXT] [--steps N]
//!     [--side 768] [--seed 0] [--guidance 30] [--prompt-cache FILE]
//! ```
//!
//! `IN` and `MASK` (white = repaint) are resized to `side`²; `OUT` is the model's picture at that size. With
//! `--prompt-cache`, the prompt's embeddings are read from FILE when it holds them, else made and written there.

use std::path::PathBuf;
use std::time::Instant;

use lightcraft_flux::{FillModel, Prompt, Request, Weights, best_device, text};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pos = Vec::new();
    let (mut prompt, mut steps, mut side, mut seed, mut guidance, mut cache) = (
        String::from("A close-up photo of a person wearing glasses, eyes clearly visible through clean lenses."),
        None,
        768usize,
        0u64,
        30.0f64,
        None::<PathBuf>,
    );
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--prompt" => prompt = val()?,
            "--steps" => steps = Some(val()?.parse()?),
            "--side" => side = val()?.parse()?,
            "--seed" => seed = val()?.parse()?,
            "--guidance" => guidance = val()?.parse()?,
            "--prompt-cache" => cache = Some(PathBuf::from(val()?)),
            _ => pos.push(a),
        }
    }
    let [dir, input, mask, out] = pos.as_slice() else {
        return Err("usage: fill WEIGHTS_DIR IN MASK OUT [--prompt TEXT] [--steps N] [--side 768] [--seed 0]".into());
    };
    let w = Weights::in_dir(dir.as_ref())?;
    let dev = best_device();
    let s = side as u32;
    let img = image::open(input)?.resize_exact(s, s, image::imageops::FilterType::Lanczos3).to_rgb8();
    let rgb: Vec<f32> = img.pixels().flat_map(|p| p.0).map(|v| f32::from(v) / 255.0).collect();
    let m = image::open(mask)?.resize_exact(s, s, image::imageops::FilterType::Triangle).to_luma8();
    let mask: Vec<f32> = m.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect();

    let t = Instant::now();
    let emb = match cache.as_ref().and_then(|c| std::fs::read(c).ok()).and_then(|b| Prompt::from_bytes(&b)) {
        Some(p) => p,
        None => {
            let p = text::encode(&w, &prompt, &dev, &|| false)?;
            if let Some(c) = &cache {
                std::fs::write(c, p.to_bytes())?;
            }
            p
        }
    };
    eprintln!("prompt: {:.1} s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let model = FillModel::load(&w, &dev)?;
    eprintln!("load: {:.1} s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let req = Request { rgb: &rgb, mask: &mask, side, steps: steps.unwrap_or(w.default_steps()), guidance, seed };
    let res = model.fill(&req, &emb, &mut |d, n| {
        eprintln!("step {d}/{n} at {:.1} s", t.elapsed().as_secs_f64());
        true
    })?;
    eprintln!("fill: {:.1} s", t.elapsed().as_secs_f64());
    let bytes: Vec<u8> = res.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    image::RgbImage::from_raw(s, s, bytes).ok_or("bad output size")?.save(out)?;
    Ok(())
}
