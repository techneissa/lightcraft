//! Wrinkles v2, step 1, the LaMa half (`crates/retouch/examples/wrinkles_v2_removal.rs` makes the inputs): fill the
//! wrinkle mask with LaMa at the face crop's resolution and time it.
//!
//! usage: wrinkles_lama <big-lama.pt> <name-src.png> <name-hole.png> <name-lama-raw.png> [work px, default 768]
//! Appends the timings to `<name-lama-raw>`'s folder's `<name>-times.txt` (named as the output without `-lama-raw.png`).
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use lightcraft_lama::region::Params;

fn to_linear(v: u8) -> f32 {
    let v = f32::from(v) / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (e * 255.0).round() as u8
}

fn main() {
    if let Err(e) = run() {
        eprintln!("wrinkles_lama: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let a: Vec<String> = std::env::args().collect();
    let arg = |i: usize| a.get(i).ok_or_else(|| "usage: wrinkles_lama <big-lama.pt> <src.png> <hole.png> <out.png> [work]".to_string());
    let work: usize = a.get(5).and_then(|v| v.parse().ok()).unwrap_or(768);
    let img = image::open(arg(2)?).map_err(|e| format!("photo: {e}"))?.to_rgb8();
    let hole_img = image::open(arg(3)?).map_err(|e| format!("hole: {e}"))?.to_luma8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if (hole_img.width() as usize, hole_img.height() as usize) != (w, h) {
        return Err("the hole's size differs from the photo's".into());
    }
    let rgb: Vec<f32> = img.pixels().flat_map(|p| p.0.map(to_linear)).collect();
    let hole: Vec<f32> = hole_img.pixels().map(|p| if p.0[0] > 127 { 1.0 } else { 0.0 }).collect();
    let t = Instant::now();
    let model = lightcraft_lama::Lama::load(Path::new(arg(1)?), &lightcraft_lama::best_device()).map_err(|e| format!("model: {e}"))?;
    let load = t.elapsed().as_secs_f64() * 1e3;
    let params = Params { work, context: 0.15, grow: 2 };
    let run_once = || model.fill_region(&rgb, &hole, w, h, &params, &|| false);
    run_once().map_err(|e| format!("fill: {e}"))?; // a first run warms the device (shaders, buffers)
    let t = Instant::now();
    let out = run_once().map_err(|e| format!("fill: {e}"))?;
    let fill = t.elapsed().as_secs_f64() * 1e3;
    let bytes: Vec<u8> = out.iter().map(|v| to_srgb8(*v)).collect();
    let o = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("size")?;
    o.save(arg(4)?).map_err(|e| format!("save: {e}"))?;
    let line = format!(
        "LaMa fill at {work} px ({w}×{h} photo, hole {} px): {fill:.0} ms (second run; model load {load:.0} ms)\n",
        hole.iter().filter(|v| **v > 0.0).count()
    );
    print!("{line}");
    let times = Path::new(arg(4)?).to_string_lossy().replace("-lama-raw.png", "-times.txt");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&times).map_err(|e| format!("{times}: {e}"))?;
    f.write_all(line.as_bytes()).map_err(|e| format!("{times}: {e}"))
}
