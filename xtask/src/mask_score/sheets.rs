//! 1:1 sheets of each method's three worst masks and its median one: the whole photo with the wrong pixels marked,
//! and the square with the most of them at full size, beside the truth and the mask.

use std::path::Path;

use image::imageops::{FilterType, crop_imm, overlay, resize};
use image::{Rgb, RgbImage};
use rayon::prelude::*;

use super::Row;
use super::chart::{BLUE, Fonts, INK, MUTED, RED, SURFACE, fill, text};
use super::report::badness;

const PANEL: u32 = 480;
const GAP: u32 = 12;
const HEAD: u32 = 76;
const WORST: usize = 3;

/// The photo with the wrong pixels tinted: blue where the mask misses, red where it adds.
fn errors(photo: &RgbImage, truth: &image::GrayImage, pred: &image::GrayImage, ignore: Option<&[bool]>) -> (RgbImage, Vec<bool>) {
    let mut out = photo.clone();
    let mut wrong = vec![false; (photo.width() * photo.height()) as usize];
    for (i, (x, y, p)) in out.enumerate_pixels_mut().enumerate() {
        let (t, m) = (truth.get_pixel(x, y).0[0] >= 128, pred.get_pixel(x, y).0[0] >= 128);
        if t == m || ignore.is_some_and(|v| v.get(i).copied().unwrap_or(false)) {
            continue;
        }
        wrong[i] = true;
        let tint = if t { BLUE } else { RED };
        for (v, c) in p.0.iter_mut().zip(tint) {
            *v = ((u16::from(*v) + 3 * u16::from(c)) / 4) as u8;
        }
    }
    (out, wrong)
}

/// The top left of the `side_w × side_h` window with the most difference between truth and mask (soft values count).
fn busiest(truth: &image::GrayImage, pred: &image::GrayImage, side_w: u32, side_h: u32) -> (u32, u32) {
    let (w, h) = truth.dimensions();
    // a summed-area table of the differences
    let stride = w as usize + 1;
    let mut sum = vec![0u64; stride * (h as usize + 1)];
    for y in 0..h as usize {
        let mut row = 0;
        for x in 0..w as usize {
            row += u64::from(truth.get_pixel(x as u32, y as u32).0[0].abs_diff(pred.get_pixel(x as u32, y as u32).0[0]));
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + row;
        }
    }
    let at = |x: u32, y: u32| sum[y as usize * stride + x as usize];
    let mut best = (0, 0, 0);
    for y in (0..=h - side_h).step_by(16) {
        for x in (0..=w - side_w).step_by(16) {
            let total = at(x + side_w, y + side_h) + at(x, y) - at(x + side_w, y) - at(x, y + side_h);
            if total > best.0 {
                best = (total, x, y);
            }
        }
    }
    (best.1, best.2)
}

fn grey_rgb(g: &image::GrayImage) -> RgbImage {
    RgbImage::from_fn(g.width(), g.height(), |x, y| Rgb([g.get_pixel(x, y).0[0]; 3]))
}

fn frame(img: &mut RgbImage, x: i64, y: i64, w: i64, h: i64) {
    for (fx, fy, fw, fh) in [(x, y, w, 3), (x, y + h - 3, w, 3), (x, y, 3, h), (x + w - 3, y, 3, h)] {
        fill(img, fx, fy, fw, fh, [255, 214, 10]);
    }
}

fn sheet(fonts: &Fonts, row: &Row, rank: &str, path: &Path) -> Result<(), String> {
    let open = |p: &Path| image::open(p).map_err(|e| format!("{}: {e}", p.display()));
    let photo = open(&row.item.photo)?.into_rgb8();
    let truth = open(&row.item.truth)?.into_luma8();
    let pred = open(&row.pred)?.into_luma8();
    if truth.dimensions() != photo.dimensions() || pred.dimensions() != photo.dimensions() {
        return Err(format!("{}: sizes differ", row.item.id));
    }
    let ignore = super::ignore_of(&row.item)?;
    let (marked, _) = errors(&photo, &truth, &pred, ignore.as_deref());
    let (w, h) = photo.dimensions();
    let (cw, ch) = (w.min(PANEL), h.min(PANEL));
    let (cx, cy) = busiest(&truth, &pred, cw, ch);
    let width = PANEL * 4 + GAP * 5;
    let mut out = RgbImage::from_pixel(width, HEAD + PANEL + GAP, Rgb(SURFACE));
    let s = &row.scores;
    let clicks = if row.clicks == 0 { String::new() } else { format!(", {} clicks", row.clicks) };
    let title = format!("{} · {}{clicks} · {rank} · {}", row.job, row.method, row.item.id);
    text(&mut out, &fonts.bold, 20.0, GAP as f32, 28.0, INK, &title);
    let numbers = format!(
        "IoU {:.3} · boundary F {:.3} · SAD {:.2} · MSE×1000 {:.1}    blue: missed · red: extra · yellow: the 1:1 square",
        s.iou,
        s.boundary_f,
        s.sad,
        s.mse * 1000.0
    );
    text(&mut out, &fonts.regular, 15.0, GAP as f32, 52.0, MUTED, &numbers);
    // the whole photo, fitted
    let k = f64::from(PANEL) / f64::from(w.max(h));
    let (fw, fh) = (((f64::from(w) * k) as u32).max(1), ((f64::from(h) * k) as u32).max(1));
    let whole = resize(&marked, fw, fh, FilterType::Triangle);
    overlay(&mut out, &whole, i64::from(GAP), i64::from(HEAD));
    let scaled = |v: u32| (f64::from(v) * k) as i64;
    frame(&mut out, i64::from(GAP) + scaled(cx), i64::from(HEAD) + scaled(cy), scaled(cw).max(7), scaled(ch).max(7));
    let square = |img: &RgbImage| crop_imm(img, cx, cy, cw, ch).to_image();
    let panels = [("photo, 1:1", square(&photo)), ("truth", square(&grey_rgb(&truth))), ("this mask", square(&grey_rgb(&pred)))];
    for (i, (name, img)) in panels.into_iter().enumerate() {
        let x = i64::from(GAP + (PANEL + GAP) * (i as u32 + 1));
        overlay(&mut out, &img, x, i64::from(HEAD));
        text(&mut out, &fonts.regular, 14.0, x as f32, (HEAD - 6) as f32, INK, name);
    }
    out.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save(dir: &Path, rows: &[Row]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // (an earlier scoring's sheets, named after their job)
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && super::JOBS.iter().any(|(job, _)| name.starts_with(&format!("{job}-"))) {
            std::fs::remove_file(entry.path()).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    let fonts = Fonts::load()?;
    // (clicks: the sheets of the last count)
    let last = rows.iter().map(|r| r.clicks).max().unwrap_or(0);
    let mut keys: Vec<(&str, &str)> = Vec::new();
    for r in rows {
        if !keys.contains(&(r.job.as_str(), r.method.as_str())) {
            keys.push((&r.job, &r.method));
        }
    }
    let mut chosen: Vec<(&Row, String)> = Vec::new();
    for (job, method) in keys {
        let mut of: Vec<&Row> = rows.iter().filter(|r| r.job == job && r.method == method && (r.clicks == 0 || r.clicks == last)).collect();
        of.sort_by(|a, b| badness(b).total_cmp(&badness(a)));
        chosen.extend(of.iter().take(WORST).enumerate().map(|(i, r)| (*r, format!("worst{}", i + 1))));
        chosen.extend(of.get(of.len() / 2).map(|r| (*r, "median".to_string())));
    }
    chosen.into_par_iter().try_for_each(|(r, rank)| {
        let path = dir.join(format!("{}-{}-{rank}-{}.png", r.job, r.method, r.item.id));
        sheet(&fonts, r, &rank, &path)
    })
}
