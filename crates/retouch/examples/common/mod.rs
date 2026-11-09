//! Shared by every feature's example: read a JPEG (or PNG), retouch it with one feature, write the result.
//!
//! `cargo run -p lightcraft-retouch --example <feature> -- in.jpg out.jpg [--amount 50] [--face x0,y0,x1,y1]...`
//!
//! `--face` gives a face box in fractions of the picture (repeat it for more faces); without one the example has
//! no faces, and a per-face feature changes nothing. The picture is worked on in linear light (sRGB primaries;
//! the app works in linear Rec.2020, which only matters to a feature that reads hue).

use std::process::ExitCode;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::Rgb32f;
use lightcraft_retouch::{Face, Features, Input, NormRect, Retouch, Stage, WholePhoto};

pub fn main(feature: &str) -> ExitCode {
    match run(feature, &std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(feature: &str, args: &[String]) -> Result<(), String> {
    let usage = || format!("usage: {feature} in.jpg out.jpg [--amount 0..100] [--face x0,y0,x1,y1]...");
    let mut files = Vec::new();
    let mut amount = 50.0;
    let mut faces = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--amount" => amount = it.next().and_then(|v| v.parse().ok()).ok_or_else(usage)?,
            "--face" => {
                let v: Vec<f64> = it.next().ok_or_else(usage)?.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                let [x0, y0, x1, y1] = v.as_slice() else { return Err(usage()) };
                faces.push(Face::boxed(NormRect::new(*x0, *y0, *x1, *y1)));
            }
            _ => files.push(a.clone()),
        }
    }
    let [input, output] = files.as_slice() else { return Err(usage()) };
    let rgb = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut img = Rgb32f::new(w, h);
    for (px, p) in img.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    let mut global = Features::default();
    if !global.set(&format!("retouch.{feature}.amount"), amount) {
        return Err(format!("{feature} has no Amount slider: set its values in the example"));
    }
    let r = Retouch { global, faces: Vec::new() };
    let input = Input::new(faces);
    let view = WholePhoto { width: w, height: h };
    let t = std::time::Instant::now();
    for stage in [Stage::Retouch, Stage::Geometry, Stage::Background] {
        lightcraft_retouch::run::apply(&mut img, &r, &input, &view, stage);
    }
    eprintln!("{feature}: {w}×{h}, {} faces, {:.0} ms", input.faces.len(), t.elapsed().as_secs_f64() * 1e3);
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("the picture's size changed")?;
    out.save(output).map_err(|e| format!("{output}: {e}"))
}
