//! Runs the model on the input a reference run saved and compares the logits.
//!
//!     cargo run --release -p lightcraft-birefnet --example compare -- <model dir> <reference dir> [side]
//!
//! `<reference dir>` holds `input.f32` and `logits.f32` (raw little-endian f32) from the Python
//! reference; see `docs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use lightcraft_birefnet::BiRefNet;

fn read(path: &std::path::Path) -> Vec<f32> {
    let b = std::fs::read(path).unwrap();
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&args[1]);
    let refdir = std::path::Path::new(&args[2]);
    let side: usize = args.get(3).map_or(1024, |s| s.parse().unwrap());
    let cpu = std::env::var("CPU").is_ok();
    let t = Instant::now();
    let dev = if cpu { candle_core_cpu() } else { lightcraft_birefnet::best_device() };
    let model = BiRefNet::load_with_side(dir, side, dev).unwrap();
    println!("loaded in {:.2}s on {:?}", t.elapsed().as_secs_f32(), model.device().location());
    let input = read(&refdir.join("input.f32"));
    let want = read(&refdir.join("logits.f32"));
    for run in 0..3 {
        let t = Instant::now();
        let m = model.matte_pixels(input.clone()).unwrap();
        println!("run {run}: {:.2}s", t.elapsed().as_secs_f32());
        if run == 0 {
            let n = want.len().min(m.logits.len());
            let (mut max, mut sum, mut agree) = (0f32, 0f64, 0usize);
            for (a, b) in m.logits.iter().zip(&want) {
                let d = (a - b).abs();
                max = max.max(d);
                sum += f64::from(d);
                if (*a > 0.0) == (*b > 0.0) {
                    agree += 1;
                }
            }
            println!("logits: max |diff| {max:.4}, mean |diff| {:.5}, same side of 0 for {:.4}% of {n}", sum / n as f64, 100.0 * agree as f64 / n as f64);
        }
    }
}

fn candle_core_cpu() -> candle_core::Device {
    candle_core::Device::Cpu
}
