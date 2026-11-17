//! Relight a photo outside the app, to judge the light: the photo with the light added (a plain sRGB rendering
//! with a soft shoulder, not the app's tone curve), and pictures of the field.
//!
//! `cargo run --release -p lightcraft-relight --example relight -- photo.jpg <depth dump dir> out-prefix
//! [x y depth amount softness warmth darken wrap]` (the dump dir is `lightcraft-depth`'s example `--dump`: `depth.f32`
//! and `size.txt`). Writes `<prefix>-before.png`, `<prefix>-after.png`, `<prefix>-gain.png`, `<prefix>-normals.png`, `<prefix>-depth.png` (the field's depth, near light).

use lightcraft_raster::Rgb32f;
use lightcraft_relight::{DepthMap, Field, Kind, Light, warmth_tilt};

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn display(v: f32) -> u8 {
    // a soft shoulder above 0.8, then sRGB
    let v = v.max(0.0);
    let v = if v > 0.8 { 0.8 + 0.2 * (1.0 - (-(v - 0.8) / 0.2).exp()) } else { v };
    let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (e.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [photo, dump, prefix, rest @ ..] = &args[..] else {
        return Err("usage: relight photo dump-dir prefix [x y depth amount softness warmth darken wrap]".into());
    };
    let num = |i: usize, d: f64| rest.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let light = Light {
        x: num(0, 0.3),
        y: num(1, 0.3),
        depth: num(2, 0.2),
        amount: num(3, 0.7),
        softness: num(4, 0.5),
        warmth: num(5, 0.0),
        wrap: num(7, 0.4),
    };
    let darken = num(6, 0.0);
    let img = image::open(photo)?.to_rgb8();
    let long = 1600u32;
    let s = f64::from(long) / f64::from(img.width().max(img.height()));
    let (w, h) = (((f64::from(img.width()) * s).round() as u32).max(1), ((f64::from(img.height()) * s).round() as u32).max(1));
    let img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
    let (w, h) = (w as usize, h as usize);
    // sRGB primaries are close enough to the working space for a picture to judge
    let lin = Rgb32f { width: w, height: h, data: img.pixels().map(|p| [0, 1, 2].map(|c| srgb_to_linear(f32::from(p[c]) / 255.0))).collect() };
    let size = std::fs::read_to_string(std::path::Path::new(dump).join("size.txt"))?;
    let mut it = size.split_whitespace().map(str::parse::<usize>);
    let (dw, dh) = (it.next().ok_or("size")??, it.next().ok_or("size")??);
    let bytes = std::fs::read(std::path::Path::new(dump).join("depth.f32"))?;
    let depth: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    let t = std::time::Instant::now();
    let field = Field::prepare_model(DepthMap { width: dw, height: dh, values: &depth, kind: Kind::Depth }, &lin)?;
    eprintln!("field {w} × {h} in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = std::time::Instant::now();
    let rig = field.rig(&light, darken, 0.0);
    let probe = |u: f32, v: f32| field.depth_plane().data.get((v * h as f32) as usize * w + (u * w as f32) as usize).copied().unwrap_or(0.0);
    eprintln!("near {:.2} far {:.2}; under the light {:.2}", field.near, field.far, probe(light.x as f32, light.y as f32));
    let shading = field.shading(&rig);
    let t1 = t.elapsed().as_secs_f64() * 1e3;
    let planes = shading.planes(w, h, &|x, y| (x / w as f64, y / h as f64));
    eprintln!("shading {t1:.1} ms, planes {:.1} ms", t.elapsed().as_secs_f64() * 1e3 - t1);
    let tilt = warmth_tilt(light.warmth);
    let mut before = image::RgbImage::new(w as u32, h as u32);
    let mut after = image::RgbImage::new(w as u32, h as u32);
    let mut gain = image::GrayImage::new(w as u32, h as u32);
    let mut normals = image::RgbImage::new(w as u32, h as u32);
    let mut depth_pic = image::GrayImage::new(w as u32, h as u32);
    let (dnear, dfar) = (field.near, field.far.max(field.near + 1e-3));
    let ln_z = field.depth_plane().data;
    for (i, c) in lin.data.iter().enumerate() {
        let (x, y) = ((i % w) as u32, (i / w) as u32);
        let (st, sh) = (planes.stops[i], planes.share[i]);
        let g = st.exp2();
        let t = tilt * sh;
        let lum = |c: [f32; 3]| 0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2];
        let mut o = c.map(|v| v * g);
        let y0 = lum(o);
        o = [o[0] * (1.0 + 0.3 * t), o[1], o[2] * (1.0 - 0.3 * t).max(0.0)];
        let y1 = lum(o).max(1e-9);
        o = o.map(|v| v * y0 / y1);
        before.put_pixel(x, y, image::Rgb(c.map(display)));
        after.put_pixel(x, y, image::Rgb(o.map(display)));
        gain.put_pixel(x, y, image::Luma([((st + 2.0) / 4.0 * 255.0).clamp(0.0, 255.0) as u8]));
        let d = ln_z.get(i).copied().unwrap_or(0.0);
        depth_pic.put_pixel(x, y, image::Luma([((1.0 - (d - dnear) / (dfar - dnear)).clamp(0.0, 1.0) * 255.0) as u8]));
        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        let n = field.normal_at(u, v);
        normals.put_pixel(x, y, image::Rgb(n.map(|c| ((c * 0.5 + 0.5) * 255.0) as u8)));
    }
    before.save(format!("{prefix}-before.png"))?;
    after.save(format!("{prefix}-after.png"))?;
    gain.save(format!("{prefix}-gain.png"))?;
    normals.save(format!("{prefix}-normals.png"))?;
    depth_pic.save(format!("{prefix}-depth.png"))?;
    Ok(())
}
