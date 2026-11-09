//! Try the background feature on a picture without building the app:
//!
//! ```text
//! cargo run -p lightcraft-retouch --release --example background -- in.jpg out.jpg --mask person.png \
//!   [--hair hair.png] [--blur 0..100] [--swap 0..100] [--clean 0..100] [--colour #RRGGBB | --picture other.jpg]
//!   [--depth depth.png] [--repeat N]
//! ```
//!
//! `--mask` is the person's matte as a grey picture (white = person), e.g. `outdir/masks/entirePerson-0.png` from
//! `cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/`; repeat it for more people.
//! `--hair` is the first person's Hair mask (`hair-0.png`): small gaps in the person's matte are filled only in it.
//! `--depth` is a grey depth map (white = near). The picture is worked on in linear light (sRGB primaries; the app
//! works in linear Rec.2020, so a swapped colour comes out slightly more saturated here than in the app).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::process::ExitCode;
use std::sync::Arc;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::background::{Analysis, Backdrop, Background, Depth, Swap};
use lightcraft_retouch::{Face, Features, Input, NormRect, Part, PartMask, Retouch, Stage, View, WholePhoto};

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn read_rgb(path: &str) -> Result<Rgb32f, String> {
    let rgb = image::open(path).map_err(|e| format!("{path}: {e}"))?.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut img = Rgb32f::new(w, h);
    for (px, p) in img.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    Ok(img)
}

fn read_grey(path: &str) -> Result<Plane, String> {
    let g = image::open(path).map_err(|e| format!("{path}: {e}"))?.to_luma8();
    let (w, h) = (g.width() as usize, g.height() as usize);
    Ok(Plane { width: w, height: h, data: g.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect() })
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = || {
        "usage: background in.jpg out.jpg --mask person.png [--hair hair.png] [--blur N] [--swap N] [--clean N] [--colour #RRGGBB | --picture file] [--depth depth.png] [--repeat N]"
            .to_string()
    };
    let mut files = Vec::new();
    let mut b = Background::default();
    let (mut masks, mut picture, mut depth, mut repeat, mut hair) = (Vec::new(), None, None, 1usize, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(usage);
        match a.as_str() {
            "--blur" => b.blur = value()?.parse().map_err(|_| usage())?,
            "--swap" => b.swap = value()?.parse().map_err(|_| usage())?,
            "--clean" => b.clean = value()?.parse().map_err(|_| usage())?,
            "--colour" => b.with = Swap::from_hex(&value()?).ok_or_else(usage)?,
            "--picture" => {
                let p = value()?;
                b.with = Swap::Picture { hash: "example".into() };
                picture = Some(p);
            }
            "--mask" => masks.push(value()?),
            "--hair" => hair = Some(value()?),
            "--depth" => depth = Some(value()?),
            "--repeat" => repeat = value()?.parse().map_err(|_| usage())?,
            _ => files.push(a.clone()),
        }
    }
    let [input, output] = files.as_slice() else { return Err(usage()) };
    let mut img = read_rgb(input)?;
    let (w, h) = (img.width, img.height);
    let mut faces = Vec::new();
    let whole = NormRect::new(0.0, 0.0, 1.0, 1.0);
    for (i, m) in masks.iter().enumerate() {
        let mut parts = vec![(Part::EntirePerson, PartMask { plane: read_grey(m)?, area: whole })];
        if let Some(h) = hair.as_ref().filter(|_| i == 0) {
            parts.push((Part::Hair, PartMask { plane: read_grey(h)?, area: whole }));
        }
        faces.push(Face { masks: parts, ..Face::boxed(NormRect::new(0.4, 0.1, 0.6, 0.3)) });
    }
    let mut input = Input::new(faces);
    if let Some(p) = &picture {
        input = input.with_backdrop(Some(Backdrop { image: Arc::new(read_rgb(p)?), key: 1 }));
    }
    if let Some(d) = &depth {
        input = input.with_depth(Some(Depth { plane: Arc::new(read_grey(d)?), key: 2 }));
    }
    let r = Retouch { global: Features { background: b, ..Features::default() }, faces: Vec::new() };
    // (the analysis image is the picture itself; the app's is the whole frame at 1600 pixels)
    let analysis = Analysis::new(Arc::new(img.clone()), 1);
    let view = Analysed { whole: WholePhoto { width: w, height: h }, analysis: &analysis };
    let t = std::time::Instant::now();
    lightcraft_retouch::run::apply(&mut img, &r, &input, &view, Stage::Background);
    let first = t.elapsed();
    // renders of the same picture: what a slider drag costs, with the work kept (the fastest of `--repeat`)
    let source = read_rgb(input_path(&files))?;
    let mut best = f64::INFINITY;
    for _ in 0..repeat.max(1) {
        let mut again = source.clone();
        let t = std::time::Instant::now();
        lightcraft_retouch::run::apply(&mut again, &r, &input, &view, Stage::Background);
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!(
        "background: {w}×{h}, {} people, {:.0} ms, then {best:.1} ms with the work kept (fastest of {repeat})",
        masks.len(),
        first.as_secs_f64() * 1e3
    );
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("the picture's size changed")?;
    out.save(output).map_err(|e| format!("{output}: {e}"))
}

/// The whole picture, with its analysis image (so the work is kept between renders, as in the app).
struct Analysed<'a> {
    whole: WholePhoto,
    analysis: &'a Analysis,
}

impl View for Analysed<'_> {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        self.whole.to_norm(x, y)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        self.whole.to_px(nx, ny)
    }

    fn analysis(&self) -> Option<&Analysis> {
        Some(self.analysis)
    }
}

fn input_path(files: &[String]) -> &str {
    files.first().map_or("", String::as_str)
}
