//! One BiRefNet model on one engine (issue #264, for #267's choice of units): its load time and its first two runs at
//! the model's input shape, on a photo.
//!
//! `cargo run --release -p lightcraft-birefnet --example coreml_units -- MODELS_DIR birefnet|birefnet-matting UNITS PHOTO`
//!
//! `UNITS`: `gpu` (CPU and GPU), `all`, `ane` (CPU and Neural Engine), `cpu`, or `candle` (candle on Metal).
use std::time::Instant;

use lightcraft_birefnet::coreml::ComputeUnits;
use lightcraft_birefnet::{BiRefNet, Runtime, Variant, best_device};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a.len() == 4, "usage: coreml_units MODELS_DIR MODEL UNITS PHOTO");
    let variant = Variant::ALL.into_iter().find(|v| v.dir_name() == a[1]).expect("model: birefnet or birefnet-matting");
    let dir = variant.dir_in(std::path::Path::new(&a[0]));
    let img = image::open(&a[3]).expect("photo").to_rgb8();
    let pixels = lightcraft_birefnet::image::preprocess(img.as_raw(), img.width() as usize, img.height() as usize, variant.input()).expect("pixels");
    let (runtime, units) = match a[2].as_str() {
        "gpu" => (Runtime::CoreMl, ComputeUnits::CpuAndGpu),
        "all" => (Runtime::CoreMl, ComputeUnits::All),
        "ane" => (Runtime::CoreMl, ComputeUnits::CpuAndNeuralEngine),
        "cpu" => (Runtime::CoreMl, ComputeUnits::CpuOnly),
        "candle" => (Runtime::Candle, ComputeUnits::CpuAndGpu),
        other => panic!("unknown units {other}"),
    };
    let t = Instant::now();
    let mut m = BiRefNet::load_with_units(&dir, variant, runtime, units, best_device()).expect("load");
    println!("{} {}: loaded in {:.2} s ({})", variant.label(), a[2], t.elapsed().as_secs_f64(), m.backend());
    for run in ["first run", "second run"] {
        let t = Instant::now();
        m.matte_pixels(pixels.clone()).expect("run");
        println!("{} {}: {run} {:.2} s", variant.label(), a[2], t.elapsed().as_secs_f64());
    }
}
