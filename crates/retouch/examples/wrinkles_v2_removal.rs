//! Wrinkles v2, step 1: does a hard wrinkle mask plus a real replacement remove deep wrinkles? A quality experiment,
//! not a feature: it compares removal methods on one face, side by side, before any detector is built.
//!
//! `cargo run -p lightcraft-retouch --release --example wrinkles_v2_removal -- in.png faces.json outdir name
//! [--crop x0,y0,x1,y1] [--lines "x,y;x,y|x,y;x,y"] [--line-width 0.02] [--no-auto] [--lama] [--hi 0.05] [--lo 0.025]
//! [--grow 1.2] [--soft 0.35] [--min-len 0.025] [--sigmas 0.25,0.5,1,1.8]`
//!
//! `faces.json` is what `lightcraft-parsing`'s `face_input` example writes. The first face is used. In `outdir` it
//! writes, with the picture's `name` in front:
//!
//! - `-before.png`, `-v1.png` (Wrinkles v1, all four sliders at 100), `-banded.png` (banded heal), `-mask.png` (the
//!   wrinkle mask), `-overlay.png` (the mask drawn on the photo), and, for the LaMa step, `-hole.png` and `-src.png`
//!   (what the `lama` crate's `wrinkles_lama` example reads);
//! - with `--lama`, also `-lama.png`: LaMa's fill (`-lama-raw.png`, written by `wrinkles_lama`) with only its low band
//!   kept and the fine band put back;
//! - with the environment variable `WV2_DEBUG=dir`, each width's ridge response and core as `resp-*.png`, `core-*.png`;
//! - `-times.txt`: the milliseconds each method took on this machine (one run, release).
//!
//! **The mask**: valleys (darker than their surroundings) of the log luminance at four widths (a Hessian ridge
//! filter), kept where they connect to a strong response, grown by `--grow` line widths so the shaded flanks go too,
//! cut by Facial Skin eroded a little, then softened by `--soft` percent of the face. `--lines` instead (or also,
//! unless `--no-auto`) draws hand-placed strokes in fractions of the picture, `--line-width` wide (a fraction of the
//! face).
//!
//! **Banded heal**, in log light per channel. Inside the mask: the fine band (pores, under 0.2 % of the face) is
//! taken from the skin beside the mask (the same band, shifted), the low band is rebuilt from the skin around the
//! mask (normalized convolution at growing widths, the finest with enough skin winning), and the mid band, the
//! crease and its flanks, is not put back at all.
//!
//! **LaMa**: the mask is the hole; the fill runs at the face crop's resolution, is blurred to its low band (0.4 % of
//! the face) and gets the same fine band back, so LaMa's own texture is never used.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::morph::{max_filter, min_filter};
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Scene, Stage, WholePhoto};

const USAGE: &str = "usage: wrinkles_v2_removal in.png faces.json outdir name [--crop x0,y0,x1,y1] [--lines \"x,y;x,y|x,y;x,y\"] \
                     [--line-width f] [--no-auto] [--lama] [--hi v] [--lo v] [--grow v] [--soft v] [--min-len v] [--sigmas a,b,c,d]";

/// Detector and mask settings. Widths are percent of the face's size (the longer side of its box).
struct Params {
    /// Line widths (the Gaussian sigma of a line), percent of the face.
    sigmas: [f32; 4],
    /// A ridge response (log-light depth) that starts a wrinkle, and the lower one it may continue through.
    hi: f32,
    lo: f32,
    /// How far the mask grows past a line, in line widths.
    grow: f32,
    /// The mask's edge softness, percent of the face.
    soft: f32,
    /// Smallest wrinkle, as a fraction of the face: a connected stretch of response with fewer pixels than
    /// `(min_len · face)²` is dropped.
    min_len: f32,
    lines: Vec<Vec<(f64, f64)>>,
    line_width: f32,
    auto: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            sigmas: [0.25, 0.5, 1.0, 1.8],
            hi: 0.05,
            lo: 0.025,
            grow: 1.2,
            soft: 0.35,
            min_len: 0.025,
            lines: Vec::new(),
            line_width: 0.02,
            auto: true,
        }
    }
}

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (mut files, mut p, mut crop, mut use_lama) = (Vec::new(), Params::default(), None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut num = || -> Result<f32, String> { it.next().and_then(|v| v.parse().ok()).ok_or(USAGE.to_string()) };
        match a.as_str() {
            "--hi" => p.hi = num()?,
            "--lo" => p.lo = num()?,
            "--grow" => p.grow = num()?,
            "--soft" => p.soft = num()?,
            "--line-width" => p.line_width = num()?,
            "--min-len" => p.min_len = num()?,
            "--sigmas" => {
                let v: Vec<f32> = it.next().ok_or(USAGE)?.split(',').filter_map(|v| v.parse().ok()).collect();
                p.sigmas = v.try_into().map_err(|_| USAGE.to_string())?;
            }
            "--no-auto" => p.auto = false,
            "--lama" => use_lama = true,
            "--crop" => crop = Some(ints(it.next().ok_or(USAGE)?, 4)?),
            "--lines" => p.lines = it.next().ok_or(USAGE)?.split('|').map(points).collect::<Result<_, _>>()?,
            _ => files.push(a.clone()),
        }
    }
    let [input, faces_json, outdir, name] = files.as_slice() else { return Err(USAGE.into()) };
    let rgb = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut src = Rgb32f::new(w, h);
    for (px, p) in src.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    let faces = read_faces(Path::new(faces_json))?;
    let input = Input::new(faces.into_iter().take(1).collect());
    let view = WholePhoto { width: w, height: h };
    let scene = Scene::new(&input, &view, w, h);
    let face = scene.faces.first().ok_or("no face")?;
    let size = face.size as f32;
    let skin = scene.mask_plane(0, Part::FacialSkin).ok_or("the face has no Facial Skin mask")?;
    let out = Path::new(outdir);
    std::fs::create_dir_all(out).map_err(|e| format!("{outdir}: {e}"))?;
    let save_to = |img: &Rgb32f, suffix: &str| save(img, &out.join(format!("{name}-{suffix}.png")), crop.as_deref());
    let mut times = String::new();

    // v1, for reference: the app's step, all four sliders at 100
    let mut v1 = src.clone();
    let mut r = Retouch::default();
    for f in ["forehead", "crowsFeet", "smileLines", "neck"] {
        r.global.set(&format!("retouch.wrinkles.{f}"), 100.0);
    }
    let t = Instant::now();
    lightcraft_retouch::run::apply(&mut v1, &r, &input, &view, Stage::Retouch);
    times += &format!("v1 (Wrinkles 100, CPU step): {:.0} ms\n", ms(t));

    // the mask
    let t = Instant::now();
    let mask = wrinkle_mask(&src, &skin, size, &p);
    times += &format!("mask (ridge filter, 4 widths, hysteresis, grow): {:.0} ms\n", ms(t));
    let m_cover = mask.data.iter().zip(&skin.data).filter(|(m, s)| **m > 0.5 && **s > 0.5).count() as f32
        / skin.data.iter().filter(|s| **s > 0.5).count().max(1) as f32;
    times += &format!("mask covers {:.0} % of Facial Skin\n", m_cover * 100.0);

    // the banded heal
    let lg = log_image(&src);
    let t = Instant::now();
    let (low_h, fine_new) = (rebuild_low(&lg, &mask, &skin, size), fine_from_neighbours(&lg, &mask, &skin, size));
    let banded = compose(&src, &lg, &mask, &low_h, &fine_new);
    times += &format!("banded heal (low rebuild + fine band): {:.0} ms\n", ms(t));

    save_to(&src, "before")?;
    save_to(&v1, "v1")?;
    save_to(&banded, "banded")?;
    save_plane(&mask, &out.join(format!("{name}-mask.png")), crop.as_deref())?;
    save_to(&overlay(&src, &mask), "overlay")?;
    save(&src, &out.join(format!("{name}-src.png")), None)?;
    let hole = mask.map(|m| if m > 0.5 { 1.0 } else { 0.0 });
    save_plane(&hole, &out.join(format!("{name}-hole.png")), None)?;

    if use_lama {
        let raw = image::open(out.join(format!("{name}-lama-raw.png"))).map_err(|e| format!("run wrinkles_lama first: {e}"))?.to_rgb8();
        if (raw.width() as usize, raw.height() as usize) != (w, h) {
            return Err("the LaMa picture's size differs from the photo's".into());
        }
        let mut fill = Rgb32f::new(w, h);
        for (px, q) in fill.data.iter_mut().zip(raw.pixels()) {
            *px = [decode_srgb8(q.0[0]), decode_srgb8(q.0[1]), decode_srgb8(q.0[2])];
        }
        let t = Instant::now();
        let low_l = gaussian(&log_image(&fill), 0.004 * size);
        let lama = compose(&src, &lg, &mask, &low_l, &fine_new);
        times += &format!("LaMa low band + fine band (the fill itself is timed by wrinkles_lama): {:.0} ms\n", ms(t));
        save_to(&lama, "lama")?;
    }
    print!("{times}");
    std::fs::write(out.join(format!("{name}-times.txt")), &times).map_err(|e| format!("times: {e}"))
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn ints(s: &str, n: usize) -> Result<Vec<usize>, String> {
    let v: Vec<usize> = s.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    (v.len() == n).then_some(v).ok_or(USAGE.to_string())
}

fn points(s: &str) -> Result<Vec<(f64, f64)>, String> {
    s.split(';')
        .map(|p| {
            let v: Vec<f64> = p.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            match v.as_slice() {
                [x, y] => Ok((*x, *y)),
                _ => Err(USAGE.to_string()),
            }
        })
        .collect()
}

/// Natural log of linear light, per channel (floored so black stays finite).
fn log_image(img: &Rgb32f) -> Rgb32f {
    img.map(|p| p.map(|v| v.max(1e-4).ln()))
}

fn luminance_log(img: &Rgb32f) -> Plane {
    img.map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).max(1e-4).ln())
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------------------------------------------------------------------------------------------------------------
// The mask

fn wrinkle_mask(src: &Rgb32f, skin: &Plane, size: f32, p: &Params) -> Plane {
    let (w, h) = (src.width, src.height);
    let mut union = Plane::new(w, h);
    if p.auto {
        let l = luminance_log(src);
        for s in p.sigmas {
            let sigma = (s / 100.0 * size).max(1.0);
            let r = valley_response(&l, sigma);
            if let Ok(dir) = std::env::var("WV2_DEBUG") {
                let _ = save_plane(&r.map(|v| v * 5.0), &Path::new(&dir).join(format!("resp-{s}.png")), None);
            }
            let r = crest(&r, sigma);
            let core = hysteresis(&r, p.lo, p.hi, ((p.min_len * size) * (p.min_len * size)) as usize);
            if let Ok(dir) = std::env::var("WV2_DEBUG") {
                let _ = save_plane(&core, &Path::new(&dir).join(format!("core-{s}.png")), None);
            }
            let grown = max_filter(&core, (p.grow * sigma).ceil() as usize + 1);
            union.data.iter_mut().zip(&grown.data).for_each(|(u, g)| *u = u.max(*g));
        }
    }
    if !p.lines.is_empty() {
        let half = p.line_width * size / 2.0;
        draw_lines(&mut union, &p.lines, half);
    }
    // cut by Facial Skin, eroded so the brows', hairline's and lips' edges stay out
    let core_skin = min_filter(&skin.map(|s| smoothstep(0.5, 0.9, s)), (0.006 * size).ceil() as usize);
    let soft = gaussian(&union, (p.soft / 100.0 * size).max(0.5)).map(|v| smoothstep(0.15, 0.85, v));
    soft.zip_map(&core_skin, |m, s| m * s)
}

/// How much a valley (dark line) of width `sigma` there is at each pixel, in log-light depth: the larger Hessian
/// eigenvalue of the blurred picture times `sigma²`, damped where the second one is as large (a dark spot).
fn valley_response(l: &Plane, sigma: f32) -> Plane {
    let g = gaussian(l, sigma);
    let (w, h) = (g.width, g.height);
    let s2 = sigma * sigma;
    Plane::from_fn(w, h, |x, y| {
        let at = |dx: isize, dy: isize| g.get_clamped(x as isize + dx, y as isize + dy);
        let c = at(0, 0);
        let (a, d) = (at(1, 0) - 2.0 * c + at(-1, 0), at(0, 1) - 2.0 * c + at(0, -1));
        let b = (at(1, 1) - at(-1, 1) - at(1, -1) + at(-1, -1)) / 4.0;
        let (mean, spread) = ((a + d) / 2.0, (((a - d) / 2.0).powi(2) + b * b).sqrt());
        let (l1, l2) = (mean + spread, mean - spread);
        if l1 <= 0.0 {
            return 0.0;
        }
        let ratio = l2 / l1;
        s2 * l1 * (-(ratio * ratio) / (2.0 * 0.5 * 0.5)).exp()
    })
}

/// `r` kept only near the crest of each line: where it is at least 60 % of its largest value within a line width, so
/// a line is thin and two neighbouring lines stay apart when the mask grows.
fn crest(r: &Plane, sigma: f32) -> Plane {
    let local = max_filter(r, sigma.ceil() as usize);
    r.zip_map(&local, |v, m| if v >= 0.6 * m { v } else { 0.0 })
}

/// Pixels above `lo` that connect (8 ways) to one above `hi`, in stretches of at least `min_px` pixels.
fn hysteresis(r: &Plane, lo: f32, hi: f32, min_px: usize) -> Plane {
    let (w, h) = (r.width, r.height);
    let mut seen = vec![false; w * h];
    let mut out = Plane::new(w, h);
    let mut stack = Vec::new();
    let mut comp = Vec::new();
    for start in 0..w * h {
        if seen[start] || r.data[start] <= lo {
            continue;
        }
        comp.clear();
        let mut strong = false;
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            comp.push(i);
            strong |= r.data[i] >= hi;
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if !seen[j] && r.data[j] > lo {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        if strong && comp.len() >= min_px {
            comp.iter().for_each(|i| out.data[*i] = 1.0);
        }
    }
    out
}

/// Strokes of `half` pixels' radius along the polylines (fractions of the picture) into `plane` (1 on a stroke).
fn draw_lines(plane: &mut Plane, lines: &[Vec<(f64, f64)>], half: f32) {
    let (w, h) = (plane.width, plane.height);
    for line in lines {
        for seg in line.windows(2) {
            let (a, b) = ((seg[0].0 as f32 * w as f32, seg[0].1 as f32 * h as f32), (seg[1].0 as f32 * w as f32, seg[1].1 as f32 * h as f32));
            let (x0, x1) = ((a.0.min(b.0) - half - 1.0).max(0.0) as usize, ((a.0.max(b.0) + half + 1.0) as usize).min(w));
            let (y0, y1) = ((a.1.min(b.1) - half - 1.0).max(0.0) as usize, ((a.1.max(b.1) + half + 1.0) as usize).min(h));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len2 = (dx * dx + dy * dy).max(1e-6);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f32 + 0.5 - a.0, y as f32 + 0.5 - a.1);
                    let t = ((px * dx + py * dy) / len2).clamp(0.0, 1.0);
                    let (ex, ey) = (px - t * dx, py - t * dy);
                    if ex * ex + ey * ey <= half * half {
                        plane.data[y * w + x] = 1.0;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The banded heal

/// Fine band (pores) width: sigma, percent of the face.
const FINE: f32 = 0.2;

/// The low band inside the mask, rebuilt from the skin around it: each channel's log light, blurred a little (so no
/// pores), then filled by normalized convolution at growing widths, the finest one with enough skin there winning.
fn rebuild_low(lg: &Rgb32f, mask: &Plane, skin: &Plane, size: f32) -> Rgb32f {
    let smooth = gaussian(lg, FINE / 100.0 * size);
    // the skin the fill may read: skin, and not in the mask
    let support = mask.zip_map(skin, |m, s| (1.0 - m).powi(2) * smoothstep(0.5, 0.9, s));
    let weighted = Rgb32f::from_fn(lg.width, lg.height, |x, y| smooth.get(x, y).map(|v| v * support.get(x, y)));
    let mut out: Option<Rgb32f> = None;
    for pct in [20.0f32, 10.0, 5.0, 2.5, 1.25, 0.6] {
        let sigma = pct / 100.0 * size;
        let (num, den) = (gaussian(&weighted, sigma), gaussian(&support, sigma));
        let level = Rgb32f::from_fn(lg.width, lg.height, |x, y| {
            let d = den.get(x, y).max(1e-6);
            num.get(x, y).map(|v| v / d)
        });
        out = Some(match out {
            None => level,
            Some(coarse) => Rgb32f::from_fn(lg.width, lg.height, |x, y| {
                let c = smoothstep(0.02, 0.35, den.get(x, y));
                let (a, b) = (level.get(x, y), coarse.get(x, y));
                std::array::from_fn(|k| a[k] * c + b[k] * (1.0 - c))
            }),
        });
    }
    out.unwrap_or_else(|| smooth.clone())
}

/// Shifts, as fractions of the face, tried in order for the skin a pixel's pores are copied from.
const SHIFTS: [(f32, f32); 12] = [
    (0.07, 0.0),
    (-0.07, 0.0),
    (0.0, 0.07),
    (0.0, -0.07),
    (0.05, 0.05),
    (-0.05, -0.05),
    (0.05, -0.05),
    (-0.05, 0.05),
    (0.14, 0.0),
    (-0.14, 0.0),
    (0.0, 0.14),
    (0.0, -0.14),
];

/// The fine band (log light minus its slight blur) at each pixel, from the nearest of the shifts whose source is
/// skin outside the mask; zero (flat) where none is.
fn fine_from_neighbours(lg: &Rgb32f, mask: &Plane, skin: &Plane, size: f32) -> Rgb32f {
    let smooth = gaussian(lg, FINE / 100.0 * size);
    let fine = lg.zip_map(&smooth, |a, b| std::array::from_fn(|k| a[k] - b[k]));
    let (w, h) = (lg.width as isize, lg.height as isize);
    Rgb32f::from_fn(lg.width, lg.height, |x, y| {
        for (sx, sy) in SHIFTS {
            let (nx, ny) = (x as isize + (sx * size) as isize, y as isize + (sy * size) as isize);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if mask.get(nx, ny) < 0.15 && skin.get(nx, ny) > 0.9 {
                return fine.get(nx, ny);
            }
        }
        [0.0; 3]
    })
}

/// `src` with the mask's pixels replaced by `low + fine` (in log light), fading in with the mask.
fn compose(src: &Rgb32f, lg: &Rgb32f, mask: &Plane, low: &Rgb32f, fine: &Rgb32f) -> Rgb32f {
    Rgb32f::from_fn(src.width, src.height, |x, y| {
        let m = mask.get(x, y);
        let (l, lo, f, s) = (lg.get(x, y), low.get(x, y), fine.get(x, y), src.get(x, y));
        std::array::from_fn(|k| if m <= 0.0 { s[k] } else { (l[k] + m * (lo[k] + f[k] - l[k])).exp() })
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Pictures

fn overlay(src: &Rgb32f, mask: &Plane) -> Rgb32f {
    src.zip_map(mask, |p, m| std::array::from_fn(|k| p[k] * (1.0 - 0.7 * m) + [1.0, 0.05, 0.05][k] * 0.5 * m))
}

fn crop_box(w: usize, h: usize, crop: Option<&[usize]>) -> (usize, usize, usize, usize) {
    match crop {
        Some([x0, y0, x1, y1]) if x1 > x0 && y1 > y0 && *x1 <= w && *y1 <= h => (*x0, *y0, x1 - x0, y1 - y0),
        _ => (0, 0, w, h),
    }
}

fn save(img: &Rgb32f, path: &Path, crop: Option<&[usize]>) -> Result<(), String> {
    let (x0, y0, cw, ch) = crop_box(img.width, img.height, crop);
    let img = img.crop(x0, y0, cw, ch);
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(cw as u32, ch as u32, bytes).ok_or("the picture's size changed")?;
    out.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn save_plane(p: &Plane, path: &Path, crop: Option<&[usize]>) -> Result<(), String> {
    let (x0, y0, cw, ch) = crop_box(p.width, p.height, crop);
    let p = p.crop(x0, y0, cw, ch);
    let bytes: Vec<u8> = p.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let out = image::GrayImage::from_raw(cw as u32, ch as u32, bytes).ok_or("the mask's size changed")?;
    out.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The faces `face_input` wrote: per face a box, landmarks and part masks (grey pictures beside the file).
fn read_faces(path: &Path) -> Result<Vec<Face>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let rect = |v: &serde_json::Value| -> Option<NormRect> {
        let a: Vec<f64> = v.as_array()?.iter().filter_map(serde_json::Value::as_f64).collect();
        let [x0, y0, x1, y1] = a.as_slice() else { return None };
        Some(NormRect::new(*x0, *y0, *x1, *y1))
    };
    let mut faces = Vec::new();
    for f in json["faces"].as_array().ok_or("no faces list")? {
        let bounds = rect(&f["box"]).ok_or("a face without a box")?;
        let landmarks = f["landmarks"]
            .as_array()
            .map(|l| l.iter().filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?))).collect())
            .unwrap_or_default();
        let mut masks = Vec::new();
        for part in Part::ALL {
            let id = serde_json::to_value(part).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let Some(m) = f["masks"].get(&id) else { continue };
            let (Some(png), Some(area)) = (m["png"].as_str(), rect(&m["area"])) else { continue };
            let grey = image::open(dir.join(png)).map_err(|e| format!("{png}: {e}"))?.to_luma8();
            let plane = Plane {
                width: grey.width() as usize,
                height: grey.height() as usize,
                data: grey.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect(),
            };
            masks.push((part, PartMask { plane, area }));
        }
        faces.push(Face { landmarks, masks, ..Face::boxed(bounds) });
    }
    Ok(faces)
}
