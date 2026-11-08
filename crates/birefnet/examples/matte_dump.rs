//! `cargo run --release -p lightcraft-birefnet --example matte_dump -- MODEL_DIR MODEL_ID OUT_DIR PHOTO...`
//!
//! Runs a BiRefNet model folder (`birefnet`, `birefnet-matting`…) on photos and writes `OUT_DIR/<stem>.<MODEL_ID>.f32`
//! (the 1024² logits, little-endian) and prints the seconds: the comparison sheets of issue #48 use it for models the
//! app does not offer (the matting model).
use std::time::Instant;

use lightcraft_birefnet::{BiRefNet, Variant};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a.len() >= 4, "usage: matte_dump MODEL_DIR MODEL_ID OUT_DIR PHOTO...");
    let model = BiRefNet::load(std::path::Path::new(&a[0]), Variant::General).expect("load");
    std::fs::create_dir_all(&a[2]).expect("out dir");
    for (n, photo) in a[3..].iter().enumerate() {
        let img = image::open(photo).expect("photo").to_rgb8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        // the app feeds a 1008 px render: scale to that so the model sees what it sees there
        let k = 1008.0 / w.max(h) as f32;
        let (sw, sh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        let small = image::imageops::resize(&img, sw, sh, image::imageops::FilterType::Lanczos3);
        if n == 0 {
            // warm-up (device, kernels)
            model.matte(small.as_raw(), sw as usize, sh as usize).expect("matte");
        }
        let t = Instant::now();
        let m = model.matte(small.as_raw(), sw as usize, sh as usize).expect("matte");
        let seconds = t.elapsed().as_secs_f64();
        let stem = std::path::Path::new(photo).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let bytes: Vec<u8> = m.logits.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(format!("{}/{stem}.{}.f32", a[2], a[1]), bytes).expect("write");
        println!("{stem:20} {} {seconds:6.2} s", a[1]);
    }
}
