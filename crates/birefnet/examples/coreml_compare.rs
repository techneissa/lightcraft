//! BiRefNet and BiRefNet Matting on Core ML against candle on the same input (issue #264): for each photo and model,
//! the largest and mean difference of the two mattes (after the sigmoid) and both mattes as grey PNGs.
//!
//! `cargo run --release -p lightcraft-birefnet --example coreml_compare -- MODELS_DIR OUT_DIR PHOTO...`
//!
//! `MODELS_DIR` holds `birefnet/` and `birefnet-matting/`, each with `model.safetensors` and
//! `coreml/<folder>.mlpackage` (`tools/convert-coreml-birefnet.py`); the package is compiled beside it on first use.
//! Writes `OUT_DIR/<stem>.<folder>.<candle|coreml>.png` (1024², white = selected).
use std::time::Instant;

use lightcraft_birefnet::{BiRefNet, Runtime, Variant, best_device};

fn sig(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}

fn grey(m: &lightcraft_birefnet::Matte, path: &std::path::Path) {
    let px: Vec<u8> = m.logits.iter().map(|v| (sig(*v) * 255.0).round() as u8).collect();
    image::GrayImage::from_raw(m.side as u32, m.side as u32, px).expect("size").save(path).expect("write");
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a.len() >= 3, "usage: coreml_compare MODELS_DIR OUT_DIR PHOTO...");
    let models = std::path::Path::new(&a[0]);
    let out = std::path::Path::new(&a[1]);
    std::fs::create_dir_all(out).expect("out dir");
    let photos: Vec<(String, image::RgbImage)> = a[2..]
        .iter()
        .map(|p| {
            let stem = std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            (stem, image::open(p).expect("photo").to_rgb8())
        })
        .collect();
    for variant in Variant::ALL {
        let dir = variant.dir_in(models);
        let t = Instant::now();
        let mut coreml = BiRefNet::load_with(&dir, variant, Runtime::CoreMl, best_device()).expect("Core ML load");
        println!("{}: Core ML loaded in {:.1} s", variant.label(), t.elapsed().as_secs_f64());
        let t = Instant::now();
        let mut candle = BiRefNet::load_with(&dir, variant, Runtime::Candle, best_device()).expect("candle load");
        println!("{}: candle loaded in {:.1} s", variant.label(), t.elapsed().as_secs_f64());
        for (stem, img) in &photos {
            let (w, h) = (img.width() as usize, img.height() as usize);
            let pixels = lightcraft_birefnet::image::preprocess(img.as_raw(), w, h, variant.input()).expect("pixels");
            let t = Instant::now();
            let c = candle.matte_pixels(pixels.clone()).expect("candle");
            let tc = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let m = coreml.matte_pixels(pixels).expect("Core ML");
            let tm = t.elapsed().as_secs_f64();
            let d: Vec<f32> = c.logits.iter().zip(&m.logits).map(|(x, y)| (sig(*x) - sig(*y)).abs()).collect();
            let max = d.iter().copied().fold(0.0, f32::max);
            let mean = d.iter().sum::<f32>() / d.len() as f32;
            let over = d.iter().filter(|v| **v > 0.1).count() as f64 / d.len() as f64;
            println!("{stem:28} {:17} max {max:.4} mean {mean:.6} >0.1 {:.4}%  candle {tc:.2} s  Core ML {tm:.2} s", variant.label(), over * 100.0);
            grey(&c, &out.join(format!("{stem}.{}.candle.png", variant.dir_name())));
            grey(&m, &out.join(format!("{stem}.{}.coreml.png", variant.dir_name())));
        }
    }
}
