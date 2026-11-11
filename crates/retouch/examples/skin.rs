//! Try the skin feature on a picture without building the app:
//!
//! `cargo run -p lightcraft-retouch --release --example skin -- in.jpg out.jpg [--amount 50] [--smoothness 50]
//! [--texture 0] [--shine 0] [--mask facialSkin-0.png] [--face x0,y0,x1,y1] [--fit 1936] [--runs 1] [--drag smoothness]`
//!
//! - `--amount` sets Smoothness (as every feature's example does); `--smoothness`, `--texture` and `--shine` set
//!   each slider (0…100; Texture −100…100).
//! - `--mask` is a grey picture of the Facial Skin mask over the whole photo, at any size (white = skin): the
//!   people parser's example writes one per person (`cargo run -p lightcraft-parsing --release --example parts --
//!   in.jpg outdir/`, then `outdir/masks/facialSkin-0.png`). Repeat it, each after its `--face`, for more faces.
//! - `--face` is the face's box in fractions of the photo; without one, a mask's face is the box of its pixels
//!   above half.
//! - `--fit` shrinks the photo to that long edge first (1936 is the draft's size of a 3:2 photo, about 2.5 MP);
//!   `--runs` repeats the step and prints the fastest run. With `--drag smoothness` (or `texture`, `shine`), the
//!   runs are the ticks of a drag of that slider up to its value: it prints the first (which makes the kept work)
//!   and the fastest of the others.
//!
//! The picture is worked on in linear light (sRGB primaries; the app works in linear Rec.2020).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::process::ExitCode;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::{Plane, Rgb32f};
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

const USAGE: &str = "usage: skin in.jpg out.jpg [--amount N] [--smoothness N] [--texture N] [--shine N] [--face x0,y0,x1,y1] \
                     [--mask mask.png] [--fit EDGE] [--runs N] [--drag SLIDER]";

fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut global = Features::default();
    let mut boxes: Vec<NormRect> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    let (mut fit, mut runs, mut drag) = (0usize, 1usize, None::<String>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| USAGE.to_string());
        match a.as_str() {
            "--amount" | "--smoothness" | "--texture" | "--shine" => {
                let field = if a == "--amount" { "smoothness" } else { a.trim_start_matches('-') };
                let v: f64 = value()?.parse().map_err(|_| USAGE.to_string())?;
                if !global.set(&format!("retouch.skin.{field}"), v) {
                    return Err(format!("no slider retouch.skin.{field}"));
                }
            }
            "--face" => {
                let v: Vec<f64> = value()?.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                let [x0, y0, x1, y1] = v.as_slice() else { return Err(USAGE.into()) };
                boxes.push(NormRect::new(*x0, *y0, *x1, *y1));
            }
            "--mask" => {
                let path = value()?;
                let grey = image::open(path).map_err(|e| format!("{path}: {e}"))?.to_luma8();
                let (w, h) = (grey.width() as usize, grey.height() as usize);
                let plane = Plane { width: w, height: h, data: grey.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect() };
                let bounds = if boxes.len() > faces.len() { boxes.pop() } else { box_of(&plane, 0.5) }.ok_or(format!("{path} is empty"))?;
                // cut to where it is above 0, as the app's masks are (each covers its face, not the photo)
                let area = box_of(&plane, 0.0).ok_or(format!("{path} is empty"))?;
                let (x0, y0) = ((area.x0 * w as f64) as usize, (area.y0 * h as f64) as usize);
                let (cw, ch) = ((area.width() * w as f64).round() as usize, (area.height() * h as f64).round() as usize);
                let plane = Plane::from_fn(cw, ch, |x, y| plane.data.get((y0 + y) * w + x0 + x).copied().unwrap_or(0.0));
                let mask = PartMask { plane, area };
                faces.push(Face { masks: vec![(Part::FacialSkin, mask)], ..Face::boxed(bounds) });
            }
            "--fit" => fit = value()?.parse().map_err(|_| USAGE.to_string())?,
            "--runs" => runs = value()?.parse::<usize>().map_err(|_| USAGE.to_string())?.max(1),
            "--drag" => drag = Some(format!("retouch.skin.{}", value()?)),
            _ => files.push(a.clone()),
        }
    }
    // boxes given without a mask: faces with no masks (the feature leaves them alone)
    faces.extend(boxes.into_iter().map(Face::boxed));
    let [input, output] = files.as_slice() else { return Err(USAGE.into()) };
    let mut rgb = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    if fit > 0 && rgb.width().max(rgb.height()) as usize > fit {
        let k = fit as f64 / f64::from(rgb.width().max(rgb.height()));
        let (w, h) = ((f64::from(rgb.width()) * k).round() as u32, (f64::from(rgb.height()) * k).round() as u32);
        rgb = image::imageops::resize(&rgb, w.max(1), h.max(1), image::imageops::FilterType::Triangle);
    }
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut src = Rgb32f::new(w, h);
    for (px, p) in src.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    let end = drag.as_deref().and_then(|id| global.get(id));
    let mut r = Retouch::with_global(global);
    let input = Input::new(faces);
    let view = WholePhoto { width: w, height: h };
    let (mut first, mut best) = (0.0, f64::INFINITY);
    let mut img = src.clone();
    for i in 0..runs {
        if let (Some(id), Some(end)) = (&drag, end) {
            r.global.set(id, end * (i + 1) as f64 / runs as f64);
        }
        img = src.clone();
        let t = std::time::Instant::now();
        lightcraft_retouch::run::apply(&mut img, &r, &input, &view, Stage::Retouch);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if i == 0 {
            first = ms;
        }
        if i > 0 || runs == 1 {
            best = best.min(ms);
        }
    }
    let (mp, n) = ((w * h) as f64 / 1e6, input.faces.len());
    match &drag {
        Some(id) => {
            eprintln!("skin: {w}×{h} ({mp:.1} MP), {n} faces, drag of {id} in {runs} ticks: first {first:.1} ms, then {best:.1} ms (fastest)")
        }
        None => eprintln!("skin: {w}×{h} ({mp:.1} MP), {n} faces, {best:.1} ms (fastest of {runs})"),
    }
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("the picture's size changed")?;
    out.save(output).map_err(|e| format!("{output}: {e}"))
}

/// The box of the mask's pixels above `over`, in fractions of the photo.
fn box_of(m: &Plane, over: f32) -> Option<NormRect> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, v) in m.data.iter().enumerate() {
        if *v > over {
            let (x, y) = (i % m.width.max(1), i / m.width.max(1));
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    let (w, h) = (m.width as f64, m.height as f64);
    (x0 < x1).then(|| NormRect::new(x0 as f64 / w, y0 as f64 / h, x1 as f64 / w, y1 as f64 / h))
}
