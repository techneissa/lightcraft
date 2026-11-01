//! Developer tool: fill a rectangle of a picture with LaMa and time it.
//! usage: spike <model.pt> <in.png> <x0,y0,x1,y1> <out.png> [cpu|metal] [raw.f32 reference input dump]
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dev = if a.get(5).map(|s| s.as_str()) == Some("cpu") { candle_core::Device::Cpu } else { lightcraft_lama::best_device() };
    println!("device {:?}", dev.location());
    let t = std::time::Instant::now();
    let m = lightcraft_lama::Lama::load(Path::new(&a[1]), &dev).unwrap();
    println!("load {:?}", t.elapsed());
    let img = image::open(&a[2]).unwrap().to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let r: Vec<usize> = a[3].split(',').map(|v| v.parse().unwrap()).collect();
    let mut rgb = vec![0f32; w * h * 3];
    let mut hole = vec![0f32; w * h];
    for y in 0..h { for x in 0..w {
        let p = img.get_pixel(x as u32, y as u32);
        for c in 0..3 { rgb[(y * w + x) * 3 + c] = p[c] as f32 / 255.0; }
        if x >= r[0] && x < r[2] && y >= r[1] && y < r[3] { hole[y * w + x] = 1.0; }
    }}
    for i in 0..2 {
        let t = std::time::Instant::now();
        let out = m.inpaint(&rgb, &hole, w, h, &|| false).unwrap();
        println!("inpaint {w}x{h} run {i}: {:?}", t.elapsed());
        if i == 1 {
            let mut o = image::RgbImage::new(w as u32, h as u32);
            for y in 0..h { for x in 0..w { for c in 0..3 { o.get_pixel_mut(x as u32, y as u32)[c] = (out[(y * w + x) * 3 + c] * 255.0).round().clamp(0.0, 255.0) as u8; } } }
            o.save(&a[4]).unwrap();
        }
    }
}
