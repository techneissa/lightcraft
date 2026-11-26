//! The chart of a run: one band a job, one panel a metric, one bar a method, each bar named and numbered (so colour
//! only repeats what the row already says). Also the small drawing helpers the sheets share.

use std::path::Path;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgb, RgbImage};

use super::Row;
use super::report::{Group, groups};

pub const SURFACE: [u8; 3] = [252, 252, 251];
pub const INK: [u8; 3] = [11, 11, 11];
pub const MUTED: [u8; 3] = [82, 81, 78];
const GRID: [u8; 3] = [225, 224, 217];
pub const BLUE: [u8; 3] = [42, 120, 214];
pub const RED: [u8; 3] = [227, 73, 72];

static REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
static BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

pub struct Fonts {
    pub regular: FontRef<'static>,
    pub bold: FontRef<'static>,
}

impl Fonts {
    pub fn load() -> Result<Fonts, String> {
        let load = |bytes| FontRef::try_from_slice(bytes).map_err(|e| format!("chart font: {e}"));
        Ok(Fonts { regular: load(REGULAR)?, bold: load(BOLD)? })
    }
}

pub fn fill(img: &mut RgbImage, x: i64, y: i64, w: i64, h: i64, colour: [u8; 3]) {
    for py in y.max(0)..(y + h).min(i64::from(img.height())) {
        for px in x.max(0)..(x + w).min(i64::from(img.width())) {
            img.put_pixel(px as u32, py as u32, Rgb(colour));
        }
    }
}

/// Draw `s` with its baseline at `y`.
pub fn text(img: &mut RgbImage, font: &FontRef<'static>, size: f32, x: f32, y: f32, colour: [u8; 3], s: &str) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut at = x;
    let mut last = None;
    for c in s.chars() {
        let id = scaled.glyph_id(c);
        if let Some(l) = last {
            at += scaled.kern(l, id);
        }
        last = Some(id);
        let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(at, y));
        at += scaled.h_advance(id);
        let Some(outline) = font.outline_glyph(glyph) else { continue };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, cover| {
            let (px, py) = (bounds.min.x as i64 + i64::from(gx), bounds.min.y as i64 + i64::from(gy));
            if px < 0 || py < 0 || px >= i64::from(img.width()) || py >= i64::from(img.height()) {
                return;
            }
            let p = img.get_pixel_mut(px as u32, py as u32);
            let a = cover.clamp(0.0, 1.0);
            for (v, &c) in p.0.iter_mut().zip(&colour) {
                *v = (f32::from(*v) * (1.0 - a) + f32::from(c) * a).round() as u8;
            }
        });
    }
}

/// A bar from `x`, square at the baseline, its far end rounded by 4 px.
fn bar(img: &mut RgbImage, x: i64, y: i64, w: i64, h: i64, colour: [u8; 3]) {
    let r = 4.min(w).min(h / 2);
    for row in 0..h {
        let edge = (r - row).max(row - (h - 1 - r)).max(0) as f64;
        let cut = if r > 0 { r - ((r * r) as f64 - edge * edge).max(0.0).sqrt().round() as i64 } else { 0 };
        fill(img, x, y + row, w - cut, 1, colour);
    }
}

fn label(job: &str, method: &str, clicks: u32) -> (String, [u8; 3]) {
    let (name, colour) = match method {
        "birefnet" => ("BiRefNet alone", BLUE),
        "matting" => ("+ Matting edge (default)", [235, 104, 52]),
        "vision" => ("+ Apple Vision edge", [27, 175, 122]),
        "sam3" => ("SAM 3", [98, 80, 214]),
        other => (other, MUTED),
    };
    match (job, clicks) {
        (_, 0) => (name.to_string(), colour),
        (_, 1) => (format!("{name}, 1 click"), colour),
        (_, n) => (format!("{name}, {n} clicks"), colour),
    }
}

type Metric = (&'static str, fn(&Group) -> f64, bool);

/// (title, value, lower is better)
fn metrics(job: &str) -> [Metric; 3] {
    let iou: Metric = ("IoU", |g| g.iou, false);
    let boundary: Metric = ("Boundary F", |g| g.boundary_f, false);
    match job {
        "hair" => [("SAD in the edge band", |g| g.sad, true), ("MSE in the edge band ×1000", |g| g.mse * 1000.0, true), boundary],
        _ => [iou, boundary, ("Edge-band IoU", |g| g.band_iou, false)],
    }
}

fn title(job: &str) -> &'static str {
    match job {
        "subject" => "Subject (DIS-VD)",
        "hair" => "Hair edge (P3M-500-NP and AIM-500)",
        "click" => "Click to select (COCO val2017)",
        "sky" => "Sky (COCO-Stuff val2017)",
        _ => "Masks",
    }
}

const LABEL_WIDTH: i64 = 260;
const PANEL: i64 = 380;
const BAR_ROOM: i64 = 290;
const ROW: i64 = 28;
const HEAD: i64 = 86;
const MARGIN: i64 = 24;

pub fn save(path: &Path, rows: &[Row]) -> Result<(), String> {
    let fonts = Fonts::load()?;
    let all: Vec<Group> = groups(rows).into_iter().filter(|g| g.set == "all").collect();
    let mut jobs: Vec<&str> = all.iter().map(|g| g.job.as_str()).collect();
    jobs.dedup();
    let height: i64 = MARGIN * 2 + jobs.iter().map(|j| HEAD + ROW * all.iter().filter(|g| g.job == *j).count() as i64).sum::<i64>();
    let width = MARGIN * 2 + LABEL_WIDTH + PANEL * 3;
    let mut img = RgbImage::from_pixel(width as u32, height.max(1) as u32, Rgb(SURFACE));
    let mut top = MARGIN;
    for job in jobs {
        let of: Vec<&Group> = all.iter().filter(|g| g.job == job).collect();
        let photos = of.first().map_or(0, |g| g.photos);
        let heading = format!("{} — {photos} photos", title(job));
        text(&mut img, &fonts.bold, 20.0, MARGIN as f32, (top + 24) as f32, INK, &heading);
        for (k, (name, value, lower)) in metrics(job).into_iter().enumerate() {
            let x = MARGIN + LABEL_WIDTH + PANEL * k as i64;
            text(&mut img, &fonts.bold, 15.0, x as f32, (top + 52) as f32, INK, name);
            let better = if lower { "lower is better" } else { "higher is better" };
            text(&mut img, &fonts.regular, 13.0, x as f32, (top + 70) as f32, MUTED, better);
            // (overlap scores share the 0-to-1 scale; errors scale to the panel's largest)
            let largest = of.iter().map(|g| value(g)).fold(0.0, f64::max);
            let scale = if lower { largest.max(f64::MIN_POSITIVE) } else { 1.0 };
            fill(&mut img, x - 1, top + HEAD - 4, 1, ROW * of.len() as i64 + 4, GRID);
            for (i, g) in of.iter().enumerate() {
                let y = top + HEAD + ROW * i as i64;
                let (_, colour) = label(job, &g.method, g.clicks);
                let long = ((value(g) / scale).clamp(0.0, 1.0) * BAR_ROOM as f64).round() as i64;
                bar(&mut img, x, y + 4, long, 18, colour);
                let shown = if lower { format!("{:.2}", value(g)) } else { format!("{:.3}", value(g)) };
                text(&mut img, &fonts.regular, 14.0, (x + long + 8) as f32, (y + 18) as f32, INK, &shown);
            }
        }
        for (i, g) in of.iter().enumerate() {
            let y = top + HEAD + ROW * i as i64;
            let (name, _) = label(job, &g.method, g.clicks);
            text(&mut img, &fonts.regular, 14.0, MARGIN as f32, (y + 18) as f32, INK, &name);
        }
        top += HEAD + ROW * of.len() as i64;
    }
    img.save(path).map_err(|e| format!("{}: {e}", path.display()))
}
