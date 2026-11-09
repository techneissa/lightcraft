//! Try the skin tone feature on a picture without building the app:
//!
//! `cargo run -p lightcraft-retouch --release --example skin_tone -- in.jpg out.jpg [--even 50] [--redness 50]
//! [--light 30] [--masks DIR --person 0]... [--face x0,y0,x1,y1] [--fit 1936] [--runs 1]`
//!
//! - `--amount` sets Even Tone (as every feature's example does); `--even`, `--redness` and `--light` set each
//!   slider (Face Light is −100…+100).
//! - `--masks DIR --person K` reads person `K`'s masks from the people parser's example output
//!   (`cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/`, then `outdir/masks`):
//!   `facialSkin-K.png`, `bodySkin-K.png`, `eyebrows-K.png` … (grey, white = the part), each missing one skipped.
//!   The face's box is Facial Skin's pixels above half, unless a `--face` comes before `--masks`. Repeat both for
//!   more faces.
//! - `--fit` shrinks the photo to that long edge first (1936 is the draft's size of a 3:2 photo, about 2.5 MP);
//!   `--runs` repeats the step and prints the fastest run.
//!
//! The picture is worked on in linear light (sRGB primaries; the app works in linear Rec.2020).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;
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

const USAGE: &str = "usage: skin_tone in.jpg out.jpg [--amount N] [--even N] [--redness N] [--light N] [--face x0,y0,x1,y1] \
                     [--masks DIR --person K] [--fit EDGE] [--runs N]";

/// The parts the feature reads, with the file names the parser's example gives them.
const PARTS: [(&str, Part); 7] = [
    ("facialSkin", Part::FacialSkin),
    ("bodySkin", Part::BodySkin),
    ("eyebrows", Part::Eyebrows),
    ("eyeWhites", Part::EyeWhites),
    ("iris", Part::Iris),
    ("lips", Part::Lips),
    ("teeth", Part::Teeth),
];

fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut global = Features::default();
    let mut boxes: Vec<NormRect> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    let mut dir: Option<String> = None;
    let (mut fit, mut runs) = (0usize, 1usize);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| USAGE.to_string());
        match a.as_str() {
            "--amount" | "--even" | "--redness" | "--light" => {
                let field = match a.as_str() {
                    "--redness" => "redness",
                    "--light" => "faceLight",
                    _ => "evenTone",
                };
                let v: f64 = value()?.parse().map_err(|_| USAGE.to_string())?;
                if !global.set(&format!("retouch.skinTone.{field}"), v) {
                    return Err(format!("no slider retouch.skinTone.{field}"));
                }
            }
            "--face" => {
                let v: Vec<f64> = value()?.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                let [x0, y0, x1, y1] = v.as_slice() else { return Err(USAGE.into()) };
                boxes.push(NormRect::new(*x0, *y0, *x1, *y1));
            }
            "--masks" => dir = Some(value()?.clone()),
            "--person" => {
                let k = value()?.clone();
                let d = dir.clone().ok_or("--masks DIR comes before --person")?;
                let given = if boxes.len() > faces.len() { boxes.pop() } else { None };
                faces.push(person(Path::new(&d), &k, given)?);
            }
            "--fit" => fit = value()?.parse().map_err(|_| USAGE.to_string())?,
            "--runs" => runs = value()?.parse::<usize>().map_err(|_| USAGE.to_string())?.max(1),
            _ => files.push(a.clone()),
        }
    }
    faces.extend(boxes.into_iter().map(Face::boxed));
    let [input, output] = files.as_slice() else { return Err(USAGE.into()) };
    let mut rgb = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    if fit > 0 && rgb.width().max(rgb.height()) as usize > fit {
        let s = fit as f64 / rgb.width().max(rgb.height()) as f64;
        let (w, h) = ((rgb.width() as f64 * s).round() as u32, (rgb.height() as f64 * s).round() as u32);
        rgb = image::imageops::resize(&rgb, w.max(1), h.max(1), image::imageops::FilterType::Triangle);
    }
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut original = Rgb32f::new(w, h);
    for (px, p) in original.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    let r = Retouch::with_global(global);
    let input = Input::new(faces);
    let view = WholePhoto { width: w, height: h };
    let mut best = f64::INFINITY;
    let mut img = original.clone();
    for _ in 0..runs {
        img = original.clone();
        let t = std::time::Instant::now();
        lightcraft_retouch::run::apply(&mut img, &r, &input, &view, Stage::Retouch);
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!("skin_tone: {w}×{h}, {} faces, {best:.1} ms", input.faces.len());
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("the picture's size changed")?;
    out.save(output).map_err(|e| format!("{output}: {e}"))
}

/// Person `k`'s masks from `dir`, each cut to where it is above 0 (as the app's masks cover their face, not the
/// photo).
fn person(dir: &Path, k: &str, bounds: Option<NormRect>) -> Result<Face, String> {
    let mut masks = Vec::new();
    let mut skin_box = None;
    for (name, part) in PARTS {
        let path = dir.join(format!("{name}-{k}.png"));
        if !path.exists() {
            continue;
        }
        let grey = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_luma8();
        let (w, h) = (grey.width() as usize, grey.height() as usize);
        let plane = Plane { width: w, height: h, data: grey.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect() };
        if part == Part::FacialSkin {
            skin_box = box_of(&plane, 0.5);
        }
        let Some(area) = box_of(&plane, 0.0) else { continue };
        let (x0, y0) = ((area.x0 * w as f64).round() as usize, (area.y0 * h as f64).round() as usize);
        let (cw, ch) = ((area.width() * w as f64).round() as usize, (area.height() * h as f64).round() as usize);
        let plane = Plane::from_fn(cw, ch, |x, y| plane.data.get((y0 + y) * w + x0 + x).copied().unwrap_or(0.0));
        masks.push((part, PartMask { plane, area }));
    }
    let bounds = bounds.or(skin_box).ok_or(format!("no facialSkin-{k}.png in {}", dir.display()))?;
    Ok(Face { masks, ..Face::boxed(bounds) })
}

/// The box (in fractions of the picture) of the pixels above `t`.
fn box_of(p: &Plane, t: f32) -> Option<NormRect> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..p.height {
        for x in 0..p.width {
            if p.data.get(y * p.width + x).is_some_and(|v| *v > t) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    (x0 < x1).then(|| {
        let (w, h) = (p.width as f64, p.height as f64);
        NormRect::new(x0 as f64 / w, y0 as f64 / h, x1 as f64 / w, y1 as f64 / h)
    })
}
