//! Try Teeth and Lips on a picture without building the app:
//!
//! ```text
//! cargo run --release -p lightcraft-retouch --example teeth_lips -- in.jpg out.jpg --face x0,y0,x1,y1 \
//!     --mask lips=lips.png --mask teeth=teeth.png [--mesh x,y;x,y;…] [--teeth 0..100] [--lip-color 0..100] [--color r,g,b] \
//!     [--hue -100..100] [--saturation -100..100] [--size WxH] [--time]
//! ```
//!
//! A `--mask` is a grey picture over the whole photo (any size; white is the part), for the `--face` given last.
//! `--size` scales the picture first (the masks are in fractions, so they follow), `--time` prints the median of 15
//! runs. The picture is worked on as the app does: linear Rec.2020.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::process::ExitCode;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::teeth_lips::{rec2020_to_srgb, srgb_to_rec2020};
use lightcraft_retouch::{Face, Features, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn numbers(s: &str) -> Vec<f64> {
    s.split(',').filter_map(|n| n.trim().parse().ok()).collect()
}

/// A grey picture as a mask, cut to the box of its non-zero pixels (as a parser's masks cover only their area).
fn grey(path: &str) -> Result<PartMask, String> {
    let g = image::open(path).map_err(|e| format!("{path}: {e}"))?.to_luma8();
    let (w, h) = (g.width(), g.height());
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in g.enumerate_pixels() {
        if p.0[0] > 0 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return Err(format!("{path} is empty"));
    }
    let data = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| f32::from(g.get_pixel(x, y).0[0]) / 255.0).collect();
    let n = |v: u32, of: u32| f64::from(v) / f64::from(of);
    Ok(PartMask {
        plane: Plane { width: (x1 - x0) as usize, height: (y1 - y0) as usize, data },
        area: NormRect::new(n(x0, w), n(y0, h), n(x1, w), n(y1, h)),
    })
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = || {
        "usage: teeth_lips in.jpg out.jpg --face x0,y0,x1,y1 --mask lips=a.png [--teeth N] [--lip-color N] [--color r,g,b] [--hue N] [--saturation N] [--size WxH] [--time]".to_string()
    };
    let mut files = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    let mut set: Vec<(&str, f64)> = Vec::new();
    let (mut size, mut time) = (None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(usage);
        match a.as_str() {
            "--face" => {
                let [x0, y0, x1, y1] = numbers(value()?)[..] else { return Err(usage()) };
                faces.push(Face::boxed(NormRect::new(x0, y0, x1, y1)));
            }
            "--mesh" => {
                // the face mesh's 478 points, `x,y;x,y;…`, after five stand-ins for YuNet's (as the app makes them)
                let pts: Vec<(f64, f64)> =
                    value()?.split(';').filter_map(|p| p.split_once(',')).filter_map(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?))).collect();
                let face = faces.last_mut().ok_or("--mesh comes after its --face")?;
                // (any stand-in missing: no landmarks at all, as the app makes them)
                let five: Option<Vec<(f64, f64)>> = [468, 473, 1, 61, 291].iter().map(|i| pts.get(*i).copied()).collect();
                face.landmarks = five.map(|f| f.into_iter().chain(pts.iter().copied()).collect()).unwrap_or_default();
            }
            "--mask" => {
                let (part, path) = value()?.split_once('=').ok_or_else(usage)?;
                let part = match part {
                    "lips" => Part::Lips,
                    "teeth" => Part::Teeth,
                    _ => return Err(format!("unknown part {part}")),
                };
                let face = faces.last_mut().ok_or("--mask comes after its --face")?;
                face.masks.push((part, grey(path)?));
            }
            "--teeth" => set.push(("retouch.teethLips.teethWhitening", value()?.parse().map_err(|_| usage())?)),
            "--lip-color" => set.push(("retouch.teethLips.lipColorAmount", value()?.parse().map_err(|_| usage())?)),
            "--hue" => set.push(("retouch.teethLips.lipHue", value()?.parse().map_err(|_| usage())?)),
            "--saturation" => set.push(("retouch.teethLips.lipSaturation", value()?.parse().map_err(|_| usage())?)),
            "--color" => {
                let [r, g, b] = numbers(value()?)[..] else { return Err(usage()) };
                set.extend([("retouch.teethLips.lipColorR", r), ("retouch.teethLips.lipColorG", g), ("retouch.teethLips.lipColorB", b)]);
            }
            "--size" => {
                let (w, h) = value()?.split_once('x').ok_or_else(usage)?;
                size = Some((w.parse::<u32>().map_err(|_| usage())?, h.parse::<u32>().map_err(|_| usage())?));
            }
            "--time" => time = true,
            _ => files.push(a.clone()),
        }
    }
    let [input, output] = files.as_slice() else { return Err(usage()) };
    let mut pic = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    if let Some((w, h)) = size {
        pic = image::imageops::resize(&pic, w, h, image::imageops::FilterType::Triangle);
    }
    let (w, h) = (pic.width() as usize, pic.height() as usize);
    let mut img = Rgb32f::new(w, h);
    for (px, p) in img.data.iter_mut().zip(pic.pixels()) {
        *px = srgb_to_rec2020([decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])]);
    }
    let mut global = Features::default();
    for (id, v) in set {
        global.set(id, v);
    }
    let r = Retouch { global, faces: Vec::new() };
    let input = Input::new(faces);
    let view = WholePhoto { width: w, height: h };
    if time {
        let mut ms = Vec::new();
        for _ in 0..15 {
            let mut copy = img.clone();
            let t = std::time::Instant::now();
            lightcraft_retouch::run::apply(&mut copy, &r, &input, &view, Stage::Retouch);
            ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
        ms.sort_by(f64::total_cmp);
        eprintln!("teeth and lips: {w}×{h}, median {:.2} ms (min {:.2})", ms.get(7).copied().unwrap_or(0.0), ms.first().copied().unwrap_or(0.0));
    }
    lightcraft_retouch::run::apply(&mut img, &r, &input, &view, Stage::Retouch);
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| rec2020_to_srgb(*p).map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("the picture's size changed")?;
    out.save(output).map_err(|e| format!("{output}: {e}"))
}
