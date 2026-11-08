//! Match one picture's look to another's without the app, for tuning:
//!
//! `cargo run --release -p lightcraft-look --example match -- target.jpg reference.jpg out.jpg [--sheet sheet.jpg]
//! [--size 256] [--budget 320]`
//!
//! Prints the fitted sliders, the loss before and after, the renders used and the time, and writes the target
//! rendered with them. `--sheet` also writes the before, the reference and the after side by side.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use lightcraft_codecs::{ChromaSubsampling, DecodeOptions, EncodeImage, EncodeMeta, encode_jpeg};
use lightcraft_develop::DevelopSettings;
use lightcraft_look::{MatchOptions, RegionPair, Target, match_look, reference};
use lightcraft_pipeline::{RenderRequest, SourceInfo};
use lightcraft_raster::Rgba8;
use lightcraft_raster::resample::{Filter, resize};

fn load_working(path: &str, edge: u32) -> lightcraft_raster::Rgb32f {
    let bytes = std::fs::read(path).expect("read target");
    let d = lightcraft_codecs::decode(&bytes, DecodeOptions::fit(edge, edge)).expect("decode target");
    d.to_working().into_oriented(lightcraft_geom::Orientation::from_exif(d.orientation))
}

fn fit_height(img: &Rgba8, h: usize) -> Rgba8 {
    let w = (img.width as f64 * h as f64 / img.height.max(1) as f64).round().max(1.0) as usize;
    // through linear light, so the sheet's halves average like the photo does
    let lin = img.map(|p| [p[0], p[1], p[2]].map(lightcraft_color::transfer::decode_srgb8));
    resize(&lin, w, h, Filter::Mitchell).map(|p| {
        let [r, g, b] = p.map(lightcraft_color::transfer::encode_srgb8);
        [r, g, b, 255]
    })
}

fn sheet(parts: &[&Rgba8], h: usize) -> Rgba8 {
    let parts: Vec<Rgba8> = parts.iter().map(|p| fit_height(p, h)).collect();
    let gap = 12;
    let w = parts.iter().map(|p| p.width).sum::<usize>() + gap * (parts.len() - 1);
    let mut out = Rgba8::filled(w, h, [24, 24, 24, 255]);
    let mut x0 = 0;
    for p in &parts {
        for y in 0..h {
            for x in 0..p.width {
                out.set(x0 + x, y, p.get(x, y));
            }
        }
        x0 += p.width + gap;
    }
    out
}

fn write_jpeg(path: &str, img: &Rgba8) {
    let bytes = encode_jpeg(&EncodeImage::rgba8(img), 92, ChromaSubsampling::S444, &EncodeMeta::default()).expect("encode");
    std::fs::write(path, bytes).expect("write");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let mut opts = MatchOptions::default();
    let mut sheet_path = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--size" => opts.size = it.next().and_then(|v| v.parse().ok()).expect("--size N"),
            "--budget" => opts.budget = it.next().and_then(|v| v.parse().ok()).expect("--budget N"),
            "--sheet" => sheet_path = it.next().cloned(),
            _ => pos.push(a.clone()),
        }
    }
    let [target, refpath, out] = pos.as_slice() else {
        eprintln!("usage: match target.jpg reference.jpg|https://… out.jpg [--sheet sheet.jpg] [--size N] [--budget N]");
        std::process::exit(2);
    };
    let src = load_working(target, 512);
    let info = SourceInfo::default();
    let settings = DevelopSettings::default();
    let source = if refpath.starts_with("https://") || refpath.starts_with("http://") {
        reference::Source::Url(refpath.clone())
    } else {
        reference::Source::File(refpath.into())
    };
    let cancel = AtomicBool::new(false);
    let refimg = match reference::load(&source, None, &cancel) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let t0 = Instant::now();
    let r = match_look(&Target { src: &src, info: &info, settings: &settings }, &refimg, &[RegionPair::whole()], &opts, &cancel, &|_, _| {})
        .expect("match");
    let secs = t0.elapsed().as_secs_f64();
    for c in &r.changes {
        println!("{:<26} {:>9.2} → {:>9.2}", c.id, c.from, c.to);
    }
    if let Some(c) = &r.curve {
        println!("curve.master {:?}", c.iter().map(|p| format!("({:.3}, {:.3})", p.x, p.y)).collect::<Vec<_>>());
    }
    println!("loss {:.5} → {:.5}, {} renders, {:.2} s", r.loss_before, r.loss_after, r.evals, secs);
    if std::env::var_os("LOOK_DEBUG").is_some() {
        let show = |name: &str, img: &Rgba8| {
            let s = lightcraft_look::stats::measure(img, None);
            println!(
                "{name}: L {:?}\n  zones ab {:?} w {:?}\n  c mean {:.3} q {:?}",
                s.l_q.map(|v| (v * 100.0).round() / 100.0),
                s.zone_ab.map(|z| z.map(|v| (v * 1000.0).round() / 1000.0)),
                s.zone_w,
                s.c_mean,
                s.c_q
            );
            for (i, b) in s.bands.iter().enumerate() {
                if b.share > 0.01 {
                    println!("  band {i}: share {:.3} L {:.2} a {:.3} b {:.3}", b.share, b.l, b.a, b.b);
                }
            }
        };
        let req = RenderRequest::fit(256, 256);
        show("reference", &refimg);
        show("before", &lightcraft_pipeline::render(&src, &info, &settings, &req).image);
        show("after", &lightcraft_pipeline::render(&src, &info, &r.settings, &req).image);
    }
    let big = load_working(target, 1600);
    let after = lightcraft_pipeline::render(&big, &info, &r.settings, &RenderRequest::fit(1600, 1600)).image;
    write_jpeg(out, &after);
    if let Some(p) = sheet_path {
        let before = lightcraft_pipeline::render(&big, &info, &settings, &RenderRequest::fit(1600, 1600)).image;
        write_jpeg(&p, &sheet(&[&before, &refimg, &after], 480));
    }
}
