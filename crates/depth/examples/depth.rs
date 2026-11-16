//! The depth of a photo, as a picture (near light, far dark) and optionally as raw files for a comparison with the
//! reference implementation.
//!
//! `cargo run --release -p lightcraft-depth --example depth -- <models>/da3-mono-large photo.jpg out.png [--long N]
//! [--dump dir]` (`--dump` writes `input.f32` `[3, h, w]`, `depth.f32` and `sky.f32` `[h, w]`, little-endian, and
//! `size.txt` `w h`).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, photo, out, rest @ ..] = &args[..] else {
        return Err("usage: depth <model dir> <photo> <out.png> [--long N] [--dump dir]".into());
    };
    let mut long = lightcraft_depth::image::PROCESS_LONG;
    let mut dump = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--long" => long = it.next().ok_or("--long N")?.parse()?,
            "--dump" => dump = Some(std::path::PathBuf::from(it.next().ok_or("--dump dir")?)),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let img = image::open(photo)?.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let t = std::time::Instant::now();
    let model = lightcraft_depth::DepthModel::load(std::path::Path::new(dir))?;
    eprintln!("loaded in {:.2} s", t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    let e = model.estimate(img.as_raw(), w, h, long)?;
    eprintln!("{} × {} estimated in {:.2} s", e.width, e.height, t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    let _ = model.estimate(img.as_raw(), w, h, long)?;
    eprintln!("again in {:.2} s", t.elapsed().as_secs_f64());
    if let Some(d) = dump {
        std::fs::create_dir_all(&d)?;
        let raw = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let input = lightcraft_depth::image::preprocess(img.as_raw(), w, h, (e.width, e.height))?;
        std::fs::write(d.join("input.f32"), raw(&input))?;
        std::fs::write(d.join("depth.f32"), raw(&e.depth))?;
        std::fs::write(d.join("sky.f32"), raw(&e.sky))?;
        std::fs::write(d.join("size.txt"), format!("{} {}", e.width, e.height))?;
    }
    // near light, far dark: inverse depth stretched between its 1st and 99th percentiles
    let mut inv: Vec<f32> = e.depth.iter().map(|d| 1.0 / d).collect();
    let mut sorted = inv.clone();
    sorted.sort_by(f32::total_cmp);
    let at = |q: f64| sorted.get(((sorted.len() - 1) as f64 * q) as usize).copied().unwrap_or(0.0);
    let (lo, hi) = (at(0.01), at(0.99));
    inv.iter_mut().for_each(|v| *v = ((*v - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0));
    let pix: Vec<u8> = inv.iter().map(|v| (v * 255.0).round() as u8).collect();
    image::GrayImage::from_raw(e.width as u32, e.height as u32, pix).ok_or("bad size")?.save(out)?;
    Ok(())
}
