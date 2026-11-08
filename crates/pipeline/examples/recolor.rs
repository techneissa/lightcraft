//! Recolor a masked area of a photo outside the app, to tune [`lightcraft_pipeline::recolor`] on real photos:
//! `cargo run --release -p lightcraft-pipeline --example recolor -- in.jpg mask.png out.jpg --to "#7a3fb0" --amount 100`
//! The mask is a greyscale picture (white = recolor) of any size; it is stretched to the photo.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use lightcraft_pipeline::recolor::{parse_hex, recolor_srgb};

fn main() {
    if let Err(e) = run() {
        eprintln!("recolor: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let (mut to, mut amount) = ("#7a3fb0".to_string(), 100.0f32);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => to = it.next().ok_or("--to needs a colour")?.clone(),
            "--amount" => amount = it.next().and_then(|v| v.parse().ok()).ok_or("--amount needs a number")?,
            _ => pos.push(a.clone()),
        }
    }
    let [input, mask, output] = pos.as_slice() else { return Err("usage: recolor in.jpg mask.png out.jpg [--to #rrggbb] [--amount 0..100]".into()) };
    let target = parse_hex(&to).ok_or_else(|| format!("not a colour: {to}"))?;
    let img = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    let (w, h) = img.dimensions();
    let m = image::open(mask).map_err(|e| format!("{mask}: {e}"))?.to_luma8();
    let m = image::imageops::resize(&m, w, h, image::imageops::FilterType::Triangle);
    let mut px: Vec<[f32; 3]> = img.pixels().map(|p| p.0.map(|v| f32::from(v) / 255.0)).collect();
    let alpha: Vec<f32> = m.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect();
    recolor_srgb(&mut px, &alpha, target, (amount / 100.0).clamp(0.0, 1.0));
    let out = image::RgbImage::from_fn(w, h, |x, y| {
        let i = (y as usize) * (w as usize) + x as usize;
        image::Rgb(px.get(i).copied().unwrap_or_default().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
    });
    out.save(output).map_err(|e| format!("{output}: {e}"))
}
