//! SCRATCH for issue #438, never merged: render each case with f32 masks and with smaller ones, on the GPU and the CPU,
//! and compare. `u16_masks <photo> <cases.json> <outdir>`; `FORMATS=u16,u8,f16`; `ONLY=<word>`.
//! cases.json: `[{"name": "...", "settings": {partial develop settings}}]`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::media::{RenderJob, SourceRef};
use lightcraft_pipeline::mask_store::{self, Format};
use lightcraft_pipeline::{DeepSamples, OutputDepth, OutputSpace, Rendered};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq)]
enum Path_ {
    Gpu8,
    GpuF,
    Cpu8,
    Cpu16,
}

impl Path_ {
    fn name(self) -> &'static str {
        match self {
            Path_::Gpu8 => "GPU 8-bit",
            Path_::GpuF => "GPU float",
            Path_::Cpu8 => "CPU 8-bit",
            Path_::Cpu16 => "CPU 16-bit",
        }
    }
    fn gpu(self) -> bool {
        matches!(self, Path_::Gpu8 | Path_::GpuF)
    }
    fn depth(self) -> OutputDepth {
        match self {
            Path_::Gpu8 | Path_::Cpu8 => OutputDepth::U8,
            Path_::GpuF => OutputDepth::F32Hdr,
            Path_::Cpu16 => OutputDepth::U16,
        }
    }
}

/// RGB samples in the output's own steps: 8-bit levels, or 65535ths of the display encoding.
struct Out {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

fn srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn samples(r: &Rendered, p: Path_) -> Out {
    match p {
        Path_::Gpu8 | Path_::Cpu8 => {
            Out { w: r.image.width, h: r.image.height, v: r.image.data.iter().flat_map(|c| [c[0], c[1], c[2]]).map(f32::from).collect() }
        }
        _ => {
            let d = r.deep.as_ref().expect("a deep render");
            let v = match &d.samples {
                DeepSamples::U16(v) => v.iter().map(|x| f32::from(*x)).collect(),
                // (linear, SDR white = 1: encoded as an sRGB display would, in 16-bit steps, not rounded)
                DeepSamples::F32(v) => v.iter().map(|x| srgb(x.max(0.0)) * 65535.0).collect(),
            };
            Out { w: d.width, h: d.height, v }
        }
    }
}

struct Diff {
    max: f32,
    changed: f64,
    /// the pixel with the largest difference
    at: (usize, usize),
    /// pixels more than 1 step away
    over1: f64,
    /// pixels beyond the proposed bound: 1 level (8-bit), 64 of 65535 (16-bit)
    beyond: f64,
}

fn compare(a: &Out, b: &Out) -> Diff {
    assert_eq!((a.w, a.h), (b.w, b.h));
    let bound = if a.v.iter().copied().fold(0f32, f32::max) > 256.0 { 64.5 } else { 1.5 };
    let mut beyond = 0usize;
    let (mut max, mut changed, mut over1, mut at) = (0f32, 0usize, 0usize, (0, 0));
    for (i, (x, y)) in a.v.chunks_exact(3).zip(b.v.chunks_exact(3)).enumerate() {
        let d = (0..3).map(|c| (x[c] - y[c]).abs()).fold(0f32, f32::max);
        // (the float render: a change counts from half a 16-bit step)
        if d >= 0.5 {
            changed += 1;
        }
        if d > 1.5 {
            over1 += 1;
        }
        if d > bound {
            beyond += 1;
        }
        if d > max {
            max = d;
            at = (i % a.w, i / a.w);
        }
    }
    let n = (a.w * a.h) as f64;
    Diff { max, changed: 100.0 * changed as f64 / n, at, over1: 100.0 * over1 as f64 / n, beyond: 100.0 * beyond as f64 / n }
}

/// f32 | smaller | difference × `factor`, a `side` px crop around `at`, shown 2×.
fn picture(a: &Out, b: &Out, at: (usize, usize), file: &Path) -> f32 {
    // `CROP=N`: the crop's side in photo pixels, shown 400 px wide
    let side = std::env::var("CROP").ok().and_then(|v| v.parse().ok()).unwrap_or(200usize).min(a.w).min(a.h);
    let zoom = (400 / side).max(1);
    let x0 = at.0.saturating_sub(side / 2).min(a.w - side);
    let y0 = at.1.saturating_sub(side / 2).min(a.h - side);
    let to8 = if a.v.iter().copied().fold(0f32, f32::max) > 256.0 { 255.0 / 65535.0 } else { 1.0 };
    let mut top = 0f32;
    for y in y0..y0 + side {
        for x in x0..x0 + side {
            for c in 0..3 {
                let i = 3 * (y * a.w + x) + c;
                top = top.max((a.v[i] - b.v[i]).abs() * to8);
            }
        }
    }
    // the largest difference shows as bright: factor on 8-bit levels, rounded down to a plain number
    let factor = if top <= 0.0 { 1.0 } else { nice(230.0 / top) };
    let gap = 8;
    let (pw, ph) = (side * zoom, side * zoom);
    let mut img = image::RgbImage::from_pixel((pw * 3 + gap * 2) as u32, ph as u32, image::Rgb([255, 0, 255]));
    for y in 0..ph {
        for x in 0..pw {
            let i = 3 * ((y0 + y / zoom) * a.w + x0 + x / zoom);
            let px = |o: &Out, c: usize| (o.v[i + c] * to8).round().clamp(0.0, 255.0) as u8;
            img.put_pixel(x as u32, y as u32, image::Rgb([px(a, 0), px(a, 1), px(a, 2)]));
            img.put_pixel((x + pw + gap) as u32, y as u32, image::Rgb([px(b, 0), px(b, 1), px(b, 2)]));
            let d = |c: usize| ((a.v[i + c] - b.v[i + c]).abs() * to8 * factor).round().clamp(0.0, 255.0) as u8;
            img.put_pixel((x + 2 * (pw + gap)) as u32, y as u32, image::Rgb([d(0), d(1), d(2)]));
        }
    }
    img.save(file).unwrap();
    factor
}

fn nice(f: f32) -> f32 {
    let steps = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0, 50000.0, 100000.0];
    steps.iter().copied().filter(|s| *s <= f).fold(1.0, f32::max)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (photo, cases, out) = (&args[0], &args[1], Path::new(&args[2]));
    std::fs::create_dir_all(out).unwrap();
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(cases).unwrap()).unwrap();
    let only = std::env::var("ONLY").unwrap_or_default();
    let formats: Vec<(Format, &str)> = std::env::var("FORMATS")
        .unwrap_or_else(|_| "u16".into())
        .split(',')
        .map(|f| match f {
            "u16" => (Format::U16, "u16"),
            "u16x" => (Format::U16ButDecontam, "u16x"),
            "u8" => (Format::U8, "u8"),
            "f16" => (Format::F16, "f16"),
            _ => panic!("format {f}"),
        })
        .collect();
    let stem = Path::new(photo).file_stem().unwrap().to_string_lossy().to_string();

    let mut s = Session::new().with_fs().with_system_clock();
    let r = s.execute("library.import", &json!({"paths": [std::fs::canonicalize(photo).unwrap()]})).unwrap();
    let id = r["imported"][0].as_u64().expect("imported");
    s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    s.execute("app.gpu", &json!({"enabled": true, "wait": true})).unwrap();

    // the photo's own size (an export's box larger than the photo would enlarge it); `SIZE=N`: that long edge
    let long = {
        let ph = s.catalog.photo(PhotoId(id)).expect("photo");
        std::env::var("SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(ph.width.max(ph.height) as usize)
    };
    let mut source = None;
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        if !name.contains(&only) {
            continue;
        }
        s.execute("develop.reset", &json!({})).unwrap();
        s.execute("develop.merge", &json!({"settings": case["settings"]})).unwrap();
        // `PATHS=GPU 8-bit,…`: only those
        let paths = std::env::var("PATHS").ok();
        for path in [Path_::Gpu8, Path_::GpuF, Path_::Cpu8, Path_::Cpu16]
            .into_iter()
            .filter(|p| paths.as_ref().is_none_or(|w| w.split(',').any(|w| w == p.name())))
        {
            let mut render = |format: Format| -> (Rendered, Option<String>) {
                mask_store::set(format);
                lightcraft_gpu::set_enabled(path.gpu());
                let mut job: RenderJob = s.export_job(PhotoId(id), long, long, OutputSpace::Srgb, path.depth()).unwrap();
                match &source {
                    Some(src) => job.source = SourceRef::Loaded(Box::new(Clone::clone(src))),
                    None => {
                        let src = job.source.load_source().unwrap();
                        source = Some(src.clone());
                        job.source = SourceRef::Loaded(Box::new(src));
                    }
                }
                job.cache = None;
                job.view_cache = None;
                job.stages = None;
                let r = job.run();
                let fell = r.gpu_fallback.clone();
                (r.rendered.unwrap(), fell)
            };
            let (reference, fell) = render(Format::F32);
            if let Some(why) = &fell {
                println!("{stem}\t{name}\t{}\tGPU FALLBACK: {why}", path.name());
            }
            let reference = samples(&reference, path);
            if std::env::var_os("BASELINE").is_some() {
                let again = samples(&render(Format::F32).0, path);
                let d = compare(&reference, &again);
                println!("BASE\t{stem}\t{name}\t{}\t{:.2}\t{:.4}\t{}x{}", path.name(), d.max, d.changed, again.w, again.h);
            }
            for &(format, fname) in &formats {
                let (r, fell) = render(format);
                if let Some(why) = &fell {
                    println!("{stem}\t{name}\t{}\tGPU FALLBACK: {why}", path.name());
                }
                let got = samples(&r, path);
                let d = compare(&reference, &got);
                if std::env::var_os("VALUES").is_some() {
                    let i = 3 * (d.at.1 * got.w + d.at.0);
                    println!("VAL\t{name}\t{}\t{fname}\tf32 {:?} smaller {:?}", path.name(), &reference.v[i..i + 3], &got.v[i..i + 3]);
                }
                println!(
                    "ROW\t{stem}\t{name}\t{}\t{fname}\t{:.2}\t{:.4}\t{:.5}\t{}x{}\t{},{}\t{:.5}",
                    path.name(),
                    d.max,
                    d.changed,
                    d.over1,
                    got.w,
                    got.h,
                    d.at.0,
                    d.at.1,
                    d.beyond
                );
                if d.max > 0.0 && matches!(path, Path_::Gpu8 | Path_::GpuF | Path_::Cpu16) {
                    let tag = match path {
                        Path_::Gpu8 => "gpu8",
                        Path_::GpuF => "gpuf",
                        _ => "cpu16",
                    };
                    let file = out.join(format!("{stem}-{name}-{fname}-{tag}.png"));
                    let factor = picture(&reference, &got, d.at, &file);
                    println!("PIC\t{}\t{factor}\t{:.2}", file.display(), d.max);
                }
            }
        }
    }
    lightcraft_gpu::set_enabled(true);
}
