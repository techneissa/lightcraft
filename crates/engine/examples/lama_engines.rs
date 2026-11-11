//! One LaMa fill through the app's own path (`LamaFill`, as a spot's render asks it) on candle and then on Core ML
//! (issue #268): a large Remove hole and a small heal spot cut from a photo, with the time of each.
//!
//! `cargo run --release -p lightcraft-engine --example lama_engines -- PHOTO`
//!
//! Uses the LaMa folder the app uses (`default_lama_dir`), with `big-lama.pt` and `coreml/lama.mlpackage`. The first
//! fill on each engine also loads it; the time printed is the second fill of a slightly moved crop (a new cache key;
//! each engine its own crops).
use std::time::Instant;

use lightcraft_coreml::Runtime;
use lightcraft_engine::lama_fill::{LamaFill, default_lama_dir};
use lightcraft_pipeline::learned_fill::{FillRun, LearnedFill};

/// A `w × h` crop of `img` at `(x0, y0)` in linear light, with an elliptical hole of `frac` of its sides in the middle.
fn crop(img: &image::RgbImage, x0: u32, y0: u32, w: usize, h: usize, frac: f32) -> (Vec<f32>, Vec<f32>) {
    let lin = |v: u8| lightcraft_color::transfer::srgb_to_linear(f32::from(v) / 255.0);
    let mut rgb = Vec::with_capacity(w * h * 3);
    let mut hole = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel((x0 + x as u32).min(img.width() - 1), (y0 + y as u32).min(img.height() - 1));
            rgb.extend(p.0.map(lin));
            let (dx, dy) = ((x as f32 + 0.5) / w as f32 - 0.5, (y as f32 + 0.5) / h as f32 - 0.5);
            hole.push(if (dx / frac).powi(2) + (dy / frac).powi(2) <= 0.25 { 1.0 } else { 0.0 });
        }
    }
    (rgb, hole)
}

fn time(fill: &LamaFill, name: &str, from: u32, job: &dyn Fn(u32) -> (Vec<f32>, Vec<f32>, usize, usize)) {
    for shift in [from, from + 3] {
        let (rgb, hole, w, h) = job(shift);
        let t = Instant::now();
        let got = fill.fill(&FillRun::new(), &rgb, &hole, w, h, false);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if shift > from {
            println!("{name} {w}×{h}: {ms:.0} ms on {:?} ({})", fill.backend(), if got.is_some() { "filled" } else { "no fill" });
        }
    }
}

fn main() {
    let Some(photo) = std::env::args().nth(1) else {
        eprintln!("usage: lama_engines PHOTO");
        std::process::exit(2);
    };
    let img = match image::open(&photo) {
        Ok(i) => i.to_rgb8(),
        Err(e) => {
            eprintln!("{photo}: {e}");
            std::process::exit(1);
        }
    };
    let fill = LamaFill::new(lightcraft_engine::activity::Activity::default());
    fill.set_runtime(Runtime::Candle);
    fill.set_dir(default_lama_dir());
    // a Remove spot: a 1200 × 900 crop of the photo with a big hole (its fill runs at 512 × 384); a heal spot: 160 × 160
    let remove = |s: u32| {
        let (r, h) = crop(&img, 100 + s, 100, 1200, 900, 0.5);
        (r, h, 1200, 900)
    };
    let heal = |s: u32| {
        let (r, h) = crop(&img, 400 + s, 300, 160, 160, 0.35);
        (r, h, 160, 160)
    };
    time(&fill, "candle, Remove", 0, &remove);
    time(&fill, "candle, heal", 0, &heal);
    fill.set_runtime(Runtime::Automatic);
    let t = Instant::now();
    while !fill.coreml_ready() {
        if t.elapsed().as_secs() > 600 {
            eprintln!("Core ML did not load");
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    println!("Core ML ready in {:.1} s", t.elapsed().as_secs_f64());
    time(&fill, "Core ML, Remove", 10, &remove);
    time(&fill, "Core ML, heal", 10, &heal);
}
