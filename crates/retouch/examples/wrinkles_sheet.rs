//! Judge Wrinkles on a real portrait: each area at several slider steps, cut at 1:1, with the numbers that say how much
//! of the lines went and whether the skin's light drifted.
//!
//! `cargo run -p lightcraft-retouch --release --example wrinkles_sheet -- photo.jpg faces.json OUT_DIR [--name p4]
//! [--steps 0,25,50,75,100] [--areas forehead,crowsFeet,smileLines,neck]`
//!
//! `faces.json` is what `cargo run -p lightcraft-parsing --release --example face_input -- photo.jpg dir/` writes. The
//! search runs first (no LaMa: the app's fallback), then each area alone at each step on the whole photo. Writes
//! `OUT_DIR/<name>-<area>-<step>.png` (the area's box at 1:1) and prints one line per area and step:
//!
//! - `depth`: the mean dark side of the band between 0.25 % and 3 % of the face (log light), over the area's skin;
//!   `removed` is the share of the step-0 depth gone;
//! - `touched`: the share of the line pixels (band below −0.03 at step 0) whose light changed by more than 1 %;
//! - `L*`: the mean lightness of the area's skin and of the skin around it (the box, outside the area), and their medians.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;
use std::process::ExitCode;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::wrinkles::{Area, Crop, Source, areas};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Scene, Stage, WholePhoto};

const USAGE: &str = "usage: wrinkles_sheet photo.jpg faces.json OUT_DIR [--name N] [--steps 0,50,100] [--areas forehead,crowsFeet]";

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn key(area: Area) -> &'static str {
    match area {
        Area::Forehead => "forehead",
        Area::CrowsFeet => "crowsFeet",
        Area::SmileLines => "smileLines",
        Area::Neck => "neck",
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let (mut name, mut steps, mut which) = (None::<String>, vec![0.0, 25.0, 50.0, 75.0, 100.0], Area::ALL.to_vec());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" => name = it.next().cloned(),
            "--steps" => steps = it.next().ok_or(USAGE)?.split(',').filter_map(|v| v.trim().parse().ok()).collect(),
            "--areas" => {
                let list = it.next().ok_or(USAGE)?;
                which = Area::ALL.iter().copied().filter(|a| list.split(',').any(|k| k == key(*a))).collect();
            }
            _ => files.push(a.clone()),
        }
    }
    let [photo, faces_json, out] = files.as_slice() else { return Err(USAGE.into()) };
    let name = name.unwrap_or_else(|| Path::new(photo).file_stem().map_or("photo".into(), |s| s.to_string_lossy().to_string()));
    std::fs::create_dir_all(out).map_err(|e| format!("{out}: {e}"))?;
    let rgb = image::open(photo).map_err(|e| format!("{photo}: {e}"))?.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut src = Rgb32f::new(w, h);
    for (px, p) in src.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    let mut faces = read_faces(Path::new(faces_json))?;
    // the largest face only: the one the sheets are about
    faces.sort_by(|a, b| (b.bounds.width() * b.bounds.height()).total_cmp(&(a.bounds.width() * a.bounds.height())));
    faces.truncate(1);
    let t = std::time::Instant::now();
    let faces = lightcraft_retouch::wrinkles::with_fixes_from(&Exact(&src), faces, &mut lightcraft_retouch::wrinkles::NoFill);
    println!("{name}: search {:.0} ms, change {:?}", t.elapsed().as_secs_f64() * 1e3, faces[0].wrinkles.as_ref().map(|f| (f.width, f.height)));
    let input = Input::new(faces);
    let view = WholePhoto { width: w, height: h };
    let scene = Scene::new(&input, &view, w, h);
    let Some(ctx) = scene.faces.first() else { return Err("no face".into()) };
    let size = ctx.size;
    println!("{name}: face {size:.0} px");
    for area in which {
        let regions = areas::regions(ctx, area);
        if regions.is_empty() {
            continue;
        }
        // the box: the area's first region (one eye's fan, one fold) or the whole area, grown a little
        let shown: Vec<&areas::Region> = match area {
            Area::CrowsFeet => regions.iter().take(2).collect(),
            Area::SmileLines => regions.iter().take(2).collect(),
            _ => regions.iter().collect(),
        };
        let r = shown.iter().map(|g| g.bounds()).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |u, b| {
            [u[0].min(b[0]), u[1].min(b[1]), u[2].max(b[2]), u[3].max(b[3])]
        });
        let m = 0.03 * size;
        let px = |v: f64, hi: usize| (v.max(0.0) as usize).min(hi);
        let b = [px(r[0] - m, w), px(r[1] - m, h), px(r[2] + m, w), px(r[3] + m, h)];
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        if bw < 8 || bh < 8 {
            continue;
        }
        let weight = Plane::from_fn(bw, bh, |x, y| {
            let p = ((b[0] + x) as f64 + 0.5, (b[1] + y) as f64 + 0.5);
            regions.iter().map(|g| g.weight(p)).fold(0.0, f32::max)
        });
        // the skin: the area's mask, and for the face the glasses' part too, without the frame (much darker or lighter
        // than the skin)
        let skin = Plane::from_fn(bw, bh, |x, y| {
            let own = scene.mask_at(0, area.part(), b[0] + x, b[1] + y);
            if area == Area::Neck {
                return own;
            }
            let p = src.get(b[0] + x, b[1] + y);
            let l = lstar(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]);
            let glasses =
                scene.mask_at(0, Part::Others, b[0] + x, b[1] + y) * if (35.0..92.0).contains(&l) && p[0] > p[2] * 1.15 { 1.0 } else { 0.0 };
            own.max(glasses)
        });
        let inside = weight.zip_map(&skin, |a, s| if a > 0.5 && s > 0.5 { 1.0 } else { 0.0 });
        let around = weight.zip_map(&skin, |a, s| if a < 0.05 && s > 0.5 { 1.0 } else { 0.0 });
        let mut first: Option<(Plane, Plane, f32, f32)> = None;
        for step in &steps {
            let mut r = Retouch::default();
            r.global.set(&format!("retouch.wrinkles.{}", key(area)), *step);
            let mut img = src.clone();
            lightcraft_retouch::run::apply(&mut img, &r, &input, &view, Stage::Retouch);
            let crop = Rgb32f::from_fn(bw, bh, |x, y| img.get(b[0] + x, b[1] + y));
            save(&crop, &format!("{out}/{name}-{}-{}.png", key(area), *step as i64))?;
            let l = crop.map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).max(1e-4).ln());
            let band = band(&l, &skin, size);
            let depth = mean(&band.map(|v| (-v).max(0.0)), &inside);
            let lines = mean(&line_depth(&l, size), &inside);
            let lstar = l.map(|v| lstar(v.exp()));
            let (li, la) = (mean(&lstar, &inside), mean(&lstar, &around));
            let (mi, ma) = (median(&lstar, &inside), median(&lstar, &around));
            let (removed, touched, line_removed, valleys) = match &first {
                None => (0.0, 0.0, 0.0, 0.0),
                Some((l0, b0, d0, v0)) => {
                    let lines_px = lines_mask(b0, &inside);
                    let moved = l.zip_map(l0, |a, b| if (a - b).abs() > 0.01 { 1.0 } else { 0.0 });
                    let dark = |b: &Plane| mean(&b.map(|v| (-v).max(0.0)), &lines_px);
                    (
                        1.0 - depth / d0.max(1e-9),
                        mean(&moved, &lines_mask(b0, &inside)),
                        1.0 - dark(&band) / dark(b0).max(1e-9),
                        1.0 - lines / v0.max(1e-9),
                    )
                }
            };
            println!(
                "{name} {:<10} {:>3}: lines {lines:.4} removed {:>3.0} % | band {depth:.4} removed {:>3.0} % (on dark px {:>3.0} %) touched {:>3.0} % | L* area {li:.1} around {la:.1} (median {mi:.1} / {ma:.1})",
                key(area),
                step,
                valleys * 100.0,
                removed * 100.0,
                line_removed * 100.0,
                touched * 100.0
            );
            if first.is_none() {
                first = Some((l, band, depth, lines));
            }
        }
    }
    Ok(())
}

/// The photo as the app's search sees it: a crop rendered at exactly the size asked for (the app renders it from the
/// original), not reduced by a whole factor as the library's fallback picture is.
struct Exact<'a>(&'a Rgb32f);

impl Source for Exact<'_> {
    fn size(&self) -> (usize, usize) {
        (self.0.width, self.0.height)
    }

    fn crop(&self, rect: NormRect, max_side: usize) -> Option<Crop> {
        let (w, h) = self.size();
        if !rect.is_valid() || w == 0 || h == 0 {
            return None;
        }
        let b = [
            ((rect.x0 * w as f64).floor().max(0.0) as usize).min(w),
            ((rect.y0 * h as f64).floor().max(0.0) as usize).min(h),
            ((rect.x1 * w as f64).ceil().max(0.0) as usize).min(w),
            ((rect.y1 * h as f64).ceil().max(0.0) as usize).min(h),
        ];
        let (cw, ch) = (b[2].checked_sub(b[0])?, b[3].checked_sub(b[1])?);
        if cw == 0 || ch == 0 {
            return None;
        }
        let part = Rgb32f::from_fn(cw, ch, |x, y| self.0.get(b[0] + x, b[1] + y));
        let k = (max_side as f64 / cw.max(ch) as f64).min(1.0);
        let (ow, oh) = (((cw as f64 * k).round() as usize).max(1), ((ch as f64 * k).round() as usize).max(1));
        let img = if (ow, oh) == (cw, ch) { part } else { resize(&part, ow, oh, Filter::Lanczos3) };
        let rect = NormRect::new(b[0] as f64 / w as f64, b[1] as f64 / h as f64, b[2] as f64 / w as f64, b[3] as f64 / h as f64);
        Some(Crop { img, rect })
    }
}

/// The pixels of `inside` where the band is darker than 3 %.
fn lines_mask(band: &Plane, inside: &Plane) -> Plane {
    band.map(|v| if v < -0.03 { 1.0 } else { 0.0 }).zip_map(inside, |a, b| a * b)
}

/// How deep a line (a long, narrow valley) runs through each pixel of the log light `l`, log light: the larger Hessian
/// curvature times σ² over 0.385 (a Gaussian line's depth at its best width), at widths from 0.15 % to 0.6 % of the face,
/// damped where the valley is round (a spot, a pore) and never below 0. It knows nothing of the search's zones, lengths or
/// thresholds.
fn line_depth(l: &Plane, size: f64) -> Plane {
    let mut best = Plane::new(l.width, l.height);
    for frac in [0.0015, 0.0025, 0.004, 0.006] {
        let sigma = ((frac * size) as f32).max(1.5);
        let g = gaussian(l, sigma);
        let r = Plane::from_fn(l.width, l.height, |x, y| {
            let at = |dx: isize, dy: isize| g.get_clamped(x as isize + dx, y as isize + dy);
            let c = at(0, 0);
            let (a, d) = (at(1, 0) - 2.0 * c + at(-1, 0), at(0, 1) - 2.0 * c + at(0, -1));
            let b = (at(1, 1) - at(-1, 1) - at(1, -1) + at(-1, -1)) / 4.0;
            let (m, s) = ((a + d) / 2.0, (((a - d) / 2.0).powi(2) + b * b).sqrt());
            let (l1, l2) = (m + s, m - s);
            if !(l1 > 0.0 && l1.is_finite()) {
                return 0.0;
            }
            let ratio = l2 / l1;
            sigma * sigma * l1 / 0.385 * (-(ratio * ratio) / (2.0 * 0.4 * 0.4)).exp()
        });
        best = best.zip_map(&r, f32::max);
    }
    best
}

/// The band between 0.25 % and 3 % of the face of the log light `l`, each blur reading the skin only.
fn band(l: &Plane, skin: &Plane, size: f64) -> Plane {
    let blur = |sigma: f64| {
        let s = (sigma * size) as f32;
        let num = gaussian(&l.zip_map(skin, |a, b| a * b), s);
        let den = gaussian(skin, s);
        num.zip_map(&den, |n, d| n / d.max(1e-4))
    };
    blur(0.0025).zip_map(&blur(0.03), |a, b| a - b)
}

fn mean(v: &Plane, m: &Plane) -> f32 {
    let (s, n) = v.data.iter().zip(&m.data).fold((0.0f64, 0.0f64), |(s, n), (a, b)| (s + f64::from(*a * *b), n + f64::from(*b)));
    (s / n.max(1e-9)) as f32
}

fn median(v: &Plane, m: &Plane) -> f32 {
    let mut x: Vec<f32> = v.data.iter().zip(&m.data).filter(|(_, b)| **b > 0.5).map(|(a, _)| *a).collect();
    if x.is_empty() {
        return f32::NAN;
    }
    let mid = x.len() / 2;
    *x.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// CIE L* of a relative luminance.
fn lstar(y: f32) -> f32 {
    if y > 0.008856 { 116.0 * y.cbrt() - 16.0 } else { 903.3 * y }
}

fn save(img: &Rgb32f, path: &str) -> Result<(), String> {
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(img.width as u32, img.height as u32, bytes).ok_or("the picture's size changed")?;
    out.save(path).map_err(|e| format!("{path}: {e}"))
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
