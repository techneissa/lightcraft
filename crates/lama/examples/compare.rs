//! Developer tool: fill a rectangle of a picture with the classic PatchMatch fill and with LaMa, side by side.
//!
//! usage: compare <big-lama.pt> <in.jpg> <x0,y0,x1,y1> <out-prefix> [cpu]
//! Writes `<out-prefix>-original.png`, `-classic.png`, `-lama.png` (the crop around the hole) and prints the timings.
use std::path::Path;
use std::time::Instant;

use lightcraft_lama::region::Params;

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn save(path: String, rgb: &[f32], w: usize, h: usize, outline: Option<&[f32]>) {
    let mut o = image::RgbImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let edge = outline.is_some_and(|m| {
                m[i] > 0.0
                    && (x == 0 || y == 0 || x + 1 == w || y + 1 == h || m[i - 1] == 0.0 || m[i + 1] == 0.0 || m[i - w] == 0.0 || m[i + w] == 0.0)
            });
            for c in 0..3 {
                o.get_pixel_mut(x as u32, y as u32)[c] = if edge { [255, 0, 255][c] } else { (linear_to_srgb(rgb[i * 3 + c]) * 255.0).round() as u8 };
            }
        }
    }
    o.save(path).unwrap_or_default();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dev = if a.get(5).map(String::as_str) == Some("cpu") { candle_core::Device::Cpu } else { lightcraft_lama::best_device() };
    let model = lightcraft_lama::Lama::load(Path::new(&a[1]), &dev).expect("model");
    let img = image::open(&a[2]).expect("photo").to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let r: Vec<usize> = a[3].split(',').map(|v| v.parse().expect("number")).collect();
    let rgb: Vec<f32> = img.pixels().flat_map(|p| p.0.map(|v| srgb_to_linear(v as f32 / 255.0))).collect();
    let mut hole = vec![0f32; w * h];
    for y in r[1].min(h)..r[3].min(h) {
        for x in r[0].min(w)..r[2].min(w) {
            hole[y * w + x] = 1.0;
        }
    }
    // the crop both fills see: the hole and half its size around it
    let side = (r[2] - r[0]).max(r[3] - r[1]);
    let margin = (side / 2).max(8);
    let (cx0, cy0, cx1, cy1) = (r[0].saturating_sub(margin), r[1].saturating_sub(margin), (r[2] + margin).min(w), (r[3] + margin).min(h));
    let (cw, ch) = (cx1 - cx0, cy1 - cy0);
    let crop = |v: &[f32], n: usize| -> Vec<f32> {
        (cy0..cy1).flat_map(|y| (cx0..cx1).flat_map(move |x| v[(y * w + x) * n..(y * w + x) * n + n].to_vec())).collect()
    };
    let (crgb, chole) = (crop(&rgb, 3), crop(&hole, 1));
    let bool_hole: Vec<bool> = chole.iter().map(|v| *v > 0.0).collect();
    // classic: the fill the Remove tool uses today
    let t = Instant::now();
    let classic = lightcraft_inpaint::complete(cw, ch, 3, &crgb, &bool_hole, &lightcraft_inpaint::CompleteParams::default())
        .unwrap_or_else(|| lightcraft_inpaint::membrane_fill(cw, ch, 3, &crgb, &bool_hole));
    let classic_ms = t.elapsed().as_secs_f64() * 1e3;
    // LaMa: once to warm up the device, then timed
    let _ = model.fill_region(&crgb, &chole, cw, ch, &Params::default(), &|| false).expect("lama");
    let t = Instant::now();
    let lama = model.fill_region(&crgb, &chole, cw, ch, &Params::default(), &|| false).expect("lama");
    let lama_ms = t.elapsed().as_secs_f64() * 1e3;
    save(format!("{}-original.png", a[4]), &crgb, cw, ch, Some(&chole));
    save(format!("{}-classic.png", a[4]), &classic, cw, ch, None);
    save(format!("{}-lama.png", a[4]), &lama, cw, ch, None);
    println!(
        "{{\"photo\":\"{}\",\"photo_px\":\"{w}x{h}\",\"hole_px\":\"{}x{}\",\"crop_px\":\"{cw}x{ch}\",\"classic_ms\":{classic_ms:.0},\"lama_ms\":{lama_ms:.0}}}",
        a[2],
        r[2] - r[0],
        r[3] - r[1]
    );
}
