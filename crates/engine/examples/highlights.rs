//! Tune the Highlights mode (fork feature, `fork/glare-highlights.md`) without building the app:
//!
//! ```text
//! cargo run --release -p lightcraft-engine --example highlights -- IN OUT [--mask M.png] [--box L,T,R,B]
//!     [--threshold N] [--expand N] [--classic] [--paint X,Y,R] [--erase X,Y,R] [--show MASK.png]
//! ```
//!
//! IN is a JPEG, PNG or TIFF; OUT a `.jpg` or `.png` of the result. `--mask` takes a picture of the photo's size
//! (white = the area); `--box` is the area in percent of the photo; `--paint` / `--erase` add one stroke point (X, Y in
//! percent, R the radius in percent of the long edge). `--show` writes the photo with the found glare in magenta. LaMa
//! is used when its model is installed (`ArtCraft/models/lama`), else (or with `--classic`) the Classic fill.
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use lightcraft_develop::{Glare, GlareStroke, ReflectionArea, SpotFill};
use lightcraft_engine::reflections::area::{self, Region};
use lightcraft_engine::reflections::highlights;
use lightcraft_geom::Point;
use lightcraft_raster::{Plane, Rgb32f, Rgba8};

fn read(path: &str) -> Result<Rgb32f, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let d = lightcraft_codecs::decode(&bytes, lightcraft_codecs::DecodeOptions::default())?;
    Ok(lightcraft_codecs::to_working(&d))
}

fn write(path: &str, img: &Rgb32f) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = area::to_srgb8(img);
    let rgba = Rgba8::from_pixels(img.width, img.height, bytes.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2], 255])).ok_or("bad size")?;
    let image = lightcraft_codecs::encode::EncodeImage::rgba8(&rgba);
    let meta = lightcraft_codecs::encode::EncodeMeta::default();
    let out = if path.ends_with(".png") {
        lightcraft_codecs::encode::encode_png(&image, &meta)?
    } else {
        lightcraft_codecs::encode::encode_jpeg(&image, 92, lightcraft_codecs::encode::ChromaSubsampling::S444, &meta)?
    };
    std::fs::write(path, out)?;
    Ok(())
}

fn numbers(s: &str) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    Ok(s.split(',').map(str::parse::<f64>).collect::<Result<_, _>>()?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "IN OUT [--mask M.png] [--box L,T,R,B] [--threshold N] [--expand N] [--classic] [--paint X,Y,R] [--erase X,Y,R] [--show MASK.png] [--model ID [--quality 384|768|1024]]";
    let (input, output) = (args.first().ok_or(usage)?, args.get(1).ok_or(usage)?);
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let src = read(input)?;
    let mut g = Glare::default();
    if let Some(t) = flag("--threshold") {
        g.threshold = t.parse()?;
    }
    if let Some(e) = flag("--expand") {
        g.expand = e.parse()?;
    }
    if args.iter().any(|a| a == "--classic") {
        g.fill = SpotFill::Classic;
    }
    for (i, a) in args.iter().enumerate() {
        if a == "--paint" || a == "--erase" {
            let v = numbers(args.get(i + 1).ok_or(usage)?)?;
            let [x, y, r] = v[..] else { return Err(usage.into()) };
            g.paint.push(GlareStroke { erase: a == "--erase", size: r / 100.0, points: vec![Point::new(x / 100.0, y / 100.0)] });
        }
    }
    let region = if let Some(m) = flag("--mask") {
        let mask = read(&m)?;
        let alpha: Plane = mask.map(|p| p[1].clamp(0.0, 1.0));
        let rect = area::bounds(&alpha, 0.01).ok_or("the mask is empty")?;
        Region { rect, alpha: Some(Arc::new(alpha)) }
    } else if let Some(b) = flag("--box") {
        let v = numbers(&b)?;
        let [l, t, r, bottom] = v[..] else { return Err(usage.into()) };
        let a = ReflectionArea::boxed(l, t, r, bottom).ok_or("invalid box")?;
        let (l, t, r, b) = a.fractions();
        Region { rect: [l, t, r, b], alpha: None }
    } else {
        Region::whole()
    };
    if let Some(model) = flag("--model") {
        // a reflection model on the area (issue #73) instead of Highlights
        let edge: u32 = flag("--quality").map(|q| q.parse()).transpose()?.unwrap_or(768);
        let start = Instant::now();
        let dir = std::env::temp_dir().join(format!("lc-area-example-{}", std::process::id()));
        let key = Some(lightcraft_preview::Hash128(1));
        let mut tiles_path = None;
        let r = area_make(&src, key, edge, &model, &region, &dir, &mut tiles_path)?;
        eprintln!("{model} at {edge} on the area: {:.1} s", start.elapsed().as_secs_f32());
        let out = lightcraft_engine::reflections::apply(&Arc::new(src.clone()), &r, key, None)?;
        write(output, &out)?;
        if let Some(p) = tiles_path {
            let _ = std::fs::remove_file(p);
        }
        return Ok(());
    }
    let learned = if g.fill == SpotFill::Lama {
        let session = lightcraft_engine::Session::with_demo().with_default_lama_model();
        let installed = session.lama.installed();
        eprintln!("LaMa {}", if installed { "installed" } else { "missing: Classic fill" });
        // the session installed its fill as the pipeline's; it stays installed after the session goes
        lightcraft_pipeline::learned_fill::current().filter(|_| installed)
    } else {
        None
    };
    let start = Instant::now();
    let made = highlights::make(&src, &g, &region, learned, &AtomicBool::new(false))?;
    eprintln!(
        "{} tile(s), {:.2} % of the area changed, fill {:?}, {:.1} s",
        made.tiles.len(),
        made.coverage * 100.0,
        made.fill,
        start.elapsed().as_secs_f32()
    );
    let mut out = src.clone();
    for t in &made.tiles {
        let Some([x0, y0, x1, y1]) = t.pixel_box(src.width, src.height) else { continue };
        let grid = lightcraft_raster::resample::resize(&t.grid, x1 - x0, y1 - y0, lightcraft_raster::resample::Filter::Bilinear);
        for (k, d) in grid.data.iter().enumerate() {
            let i = (y0 + k / grid.width) * src.width + x0 + k % grid.width;
            if let Some(p) = out.data.get_mut(i) {
                *p = [0, 1, 2].map(|c| (p[c] - d[c]).max(0.0));
            }
        }
    }
    write(output, &out)?;
    if let Some(show) = flag("--show") {
        // what changed, in magenta over the photo
        let marked = Rgb32f::from_fn(src.width, src.height, |x, y| {
            let (p, q) = (src.get(x, y), out.get(x, y));
            let a = ((0..3).map(|c| (p[c] - q[c]).abs()).fold(0.0f32, f32::max) * 8.0).clamp(0.0, 1.0);
            [p[0] + (1.0 - p[0]) * a, p[1] * (1.0 - a), p[2] + (1.0 - p[2]) * a]
        });
        write(&show, &marked)?;
    }
    Ok(())
}

/// [`area::make`] saving into `dir` instead of the app's cache folder.
fn area_make(
    src: &Rgb32f,
    key: Option<lightcraft_preview::Hash128>,
    edge: u32,
    model: &str,
    region: &Region,
    dir: &std::path::Path,
    path: &mut Option<String>,
) -> Result<lightcraft_develop::Reflection, String> {
    let made = area::run_model(src, edge, model, region)?;
    let r = area::save_in(dir, &[made], key, lightcraft_develop::Reflection { amount: 100.0, ..Default::default() })?;
    *path = Some(r.correction.clone());
    Ok(r)
}
