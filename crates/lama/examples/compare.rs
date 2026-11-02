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
            let at = |m: &[f32], j: usize| m.get(j).copied().unwrap_or(0.0);
            let edge = outline.is_some_and(|m| {
                at(m, i) > 0.0
                    && (x == 0
                        || y == 0
                        || x + 1 == w
                        || y + 1 == h
                        || at(m, i - 1) == 0.0
                        || at(m, i + 1) == 0.0
                        || at(m, i - w) == 0.0
                        || at(m, i + w) == 0.0)
            });
            let px = o.get_pixel_mut(x as u32, y as u32);
            for (c, out) in px.0.iter_mut().enumerate() {
                *out = if edge { [255, 0, 255][c] } else { (linear_to_srgb(at(rgb, i * 3 + c)) * 255.0).round() as u8 };
            }
        }
    }
    o.save(path).unwrap_or_default();
}

fn main() {
    if let Err(e) = run() {
        eprintln!("compare: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let a: Vec<String> = std::env::args().collect();
    let arg = |i: usize| a.get(i).ok_or_else(|| "usage: compare <big-lama.pt> <in.jpg> <x0,y0,x1,y1> <out-prefix> [cpu]".to_string());
    let dev = if a.get(5).map(String::as_str) == Some("cpu") { candle_core::Device::Cpu } else { lightcraft_lama::best_device() };
    let model = lightcraft_lama::Lama::load(Path::new(arg(1)?), &dev).map_err(|e| format!("model: {e}"))?;
    let img = image::open(arg(2)?).map_err(|e| format!("photo: {e}"))?.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let r: Vec<usize> = arg(3)?.split(',').map(|v| v.parse().map_err(|_| format!("`{v}` is not a number"))).collect::<Result<_, _>>()?;
    let [rx0, ry0, rx1, ry1] = r[..] else { return Err("the rectangle is x0,y0,x1,y1".into()) };
    let out = arg(4)?;
    let rgb: Vec<f32> = img.pixels().flat_map(|p| p.0.map(|v| srgb_to_linear(v as f32 / 255.0))).collect();
    let mut hole = vec![0f32; w * h];
    for y in ry0.min(h)..ry1.min(h) {
        for x in rx0.min(w)..rx1.min(w) {
            if let Some(v) = hole.get_mut(y * w + x) {
                *v = 1.0;
            }
        }
    }
    // the crop both fills see: the hole and half its size around it
    let side = rx1.saturating_sub(rx0).max(ry1.saturating_sub(ry0));
    let margin = (side / 2).max(8);
    let (cx0, cy0, cx1, cy1) =
        (rx0.saturating_sub(margin), ry0.saturating_sub(margin), rx1.saturating_add(margin).min(w), ry1.saturating_add(margin).min(h));
    let (cw, ch) = (cx1.saturating_sub(cx0), cy1.saturating_sub(cy0));
    if cw == 0 || ch == 0 {
        return Err("the rectangle is outside the photo".into());
    }
    let crop = |v: &[f32], n: usize| -> Vec<f32> {
        (cy0..cy1).flat_map(|y| (cx0..cx1).flat_map(move |x| v.get((y * w + x) * n..(y * w + x) * n + n).unwrap_or(&[]).to_vec())).collect()
    };
    let (crgb, chole) = (crop(&rgb, 3), crop(&hole, 1));
    let bool_hole: Vec<bool> = chole.iter().map(|v| *v > 0.0).collect();
    // classic: the fill the Remove tool uses today
    let t = Instant::now();
    let classic = lightcraft_inpaint::complete(cw, ch, 3, &crgb, &bool_hole, &lightcraft_inpaint::CompleteParams::default())
        .unwrap_or_else(|| lightcraft_inpaint::membrane_fill(cw, ch, 3, &crgb, &bool_hole));
    let classic_ms = t.elapsed().as_secs_f64() * 1e3;
    // LaMa: once to warm up the device, then timed
    let _ = model.fill_region(&crgb, &chole, cw, ch, &Params::default(), &|| false).map_err(|e| format!("lama: {e}"))?;
    let t = Instant::now();
    let lama = model.fill_region(&crgb, &chole, cw, ch, &Params::default(), &|| false).map_err(|e| format!("lama: {e}"))?;
    let lama_ms = t.elapsed().as_secs_f64() * 1e3;
    save(format!("{out}-original.png"), &crgb, cw, ch, Some(&chole));
    save(format!("{out}-classic.png"), &classic, cw, ch, None);
    save(format!("{out}-lama.png"), &lama, cw, ch, None);
    println!(
        "{{\"photo\":\"{}\",\"photo_px\":\"{w}x{h}\",\"hole_px\":\"{}x{}\",\"crop_px\":\"{cw}x{ch}\",\"classic_ms\":{classic_ms:.0},\"lama_ms\":{lama_ms:.0}}}",
        arg(2)?,
        rx1.saturating_sub(rx0),
        ry1.saturating_sub(ry0)
    );
    Ok(())
}
