//! LaMa on Core ML against candle on the same crops (issue #268): what mirroring a crop up to the package's side does to
//! the fill, and how long each takes.
//!
//! `cargo run --release -p lightcraft-lama --example coreml_compare -- LAMA_DIR PORTRAITS_DIR OUT_DIR [WxH ...]`
//!
//! `LAMA_DIR` holds `big-lama.pt` and `coreml/lama.mlpackage` (`tools/convert-coreml-lama.py`; compiled beside it on
//! first use). `PORTRAITS_DIR` is the corpus's CC0 portraits (`cargo xtask corpus --download`, `corpus/portraits`). Six
//! fills (two Remove holes, two heal spots, two lines), each cropped from a portrait and scaled to every size given
//! (default `200x200 400x300 640x480`). For each: the fill on candle at the crop's own size (today's path), on candle
//! mirrored up to the package's side (what padding alone does), on candle with the padding as more hole instead, on
//! candle with the crop moved by 2 px (how much the fill moves anyway), and on Core ML (what ships); the largest and
//! mean difference inside the hole against the first, in 0..255. Writes the worst Core ML fill at 1:1 between candle's and
//! candle's of the crop moved by 2 px (how much the fill moves anyway) to `OUT_DIR/worst.png`.
use std::path::Path;
use std::time::Instant;

use lightcraft_lama::coreml::{CoreMlLama, DEFAULT_UNITS, side_for};
use lightcraft_lama::{Lama, best_device};

/// (name, photo, crop centre x, y as fractions of the photo, crop side as a fraction of its short side, hole: an ellipse's
/// box or a line's ends, as fractions of the crop)
type Case = (&'static str, &'static str, f32, f32, f32, (bool, f32, f32, f32, f32));
const CASES: [Case; 6] = [
    ("remove-person-steps", "p7-small-face-steps.jpg", 0.50, 0.62, 0.75, (true, 0.30, 0.10, 0.72, 0.95)),
    ("remove-leaf", "p5-woman-low-key.jpg", 0.18, 0.40, 0.70, (true, 0.25, 0.20, 0.65, 0.75)),
    ("heal-blemish-freckles", "p3-freckles-red-lips.jpg", 0.40, 0.42, 0.30, (true, 0.44, 0.44, 0.56, 0.56)),
    ("heal-blemish-wrinkles", "p4-elderly-man-wrinkles.jpg", 0.45, 0.42, 0.35, (true, 0.40, 0.45, 0.52, 0.57)),
    ("line-across-face", "p1-young-man-dark-skin.jpg", 0.45, 0.45, 0.90, (false, 0.05, 0.10, 0.95, 0.85)),
    ("line-across-hat", "p2-elderly-woman-glasses.jpg", 0.45, 0.25, 0.60, (false, 0.0, 0.70, 1.0, 0.30)),
];

fn main() {
    if let Err(e) = run() {
        eprintln!("coreml_compare: {e}");
        std::process::exit(1);
    }
}

/// The case's crop at `w × h` (gamma-encoded RGB in 0..1, interleaved) and its hole.
fn make(case: &Case, dir: &Path, w: usize, h: usize, shift: f32) -> Result<(Vec<f32>, Vec<f32>), String> {
    let (_, file, cx, cy, side, (ellipse, a, b, c, d)) = *case;
    let im = image::open(dir.join(file)).map_err(|e| format!("{file}: {e}"))?.to_rgb8();
    let (pw, ph) = (im.width() as f32, im.height() as f32);
    // the crop has the size's aspect, `side` of the photo's short side on its longer side
    let long = side * pw.min(ph);
    let (sw, sh) = if w >= h { (long, long * h as f32 / w as f32) } else { (long * w as f32 / h as f32, long) };
    let x0 = (cx * pw - sw / 2.0 + shift * sw / w as f32).clamp(0.0, pw - sw);
    let y0 = (cy * ph - sh / 2.0).clamp(0.0, ph - sh);
    let crop = image::imageops::crop_imm(&im, x0 as u32, y0 as u32, sw as u32, sh as u32).to_image();
    let crop = image::imageops::resize(&crop, w as u32, h as u32, image::imageops::FilterType::Lanczos3);
    let rgb: Vec<f32> = crop.as_raw().iter().map(|v| f32::from(*v) / 255.0).collect();
    let thick = (w.min(h) as f32 / 80.0).max(3.0) / 2.0;
    let hole = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let inside = if ellipse {
                let (ex, ey, rx, ry) = ((a + c) / 2.0 * w as f32, (b + d) / 2.0 * h as f32, (c - a) / 2.0 * w as f32, (d - b) / 2.0 * h as f32);
                ((x - ex) / rx).powi(2) + ((y - ey) / ry).powi(2) <= 1.0
            } else {
                let (p, q) = ((a * w as f32, b * h as f32), (c * w as f32, d * h as f32));
                let (dx, dy) = (q.0 - p.0, q.1 - p.1);
                let t = (((x - p.0) * dx + (y - p.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                ((x - p.0 - t * dx).powi(2) + (y - p.1 - t * dy).powi(2)).sqrt() <= thick
            };
            if inside { 1.0 } else { 0.0 }
        })
        .collect();
    Ok((rgb, hole))
}

/// `v` (`w × h`, `k` values a pixel) mirrored at the right and bottom up to `s × s`.
fn mirror(v: &[f32], w: usize, h: usize, s: usize, k: usize) -> Vec<f32> {
    let m = |i: usize, n: usize| {
        let j = i % (2 * n);
        if j < n { j } else { 2 * n - 1 - j }
    };
    let mut out = Vec::with_capacity(s * s * k);
    for y in 0..s {
        for x in 0..s {
            let at = (m(y, h) * w + m(x, w)) * k;
            out.extend_from_slice(&v[at..at + k]);
        }
    }
    out
}

/// `rgb` and `hole` (`w × h`) on an `s × s` canvas whose rest is hole (black under it).
fn pad_as_hole(rgb: &[f32], hole: &[f32], w: usize, h: usize, s: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut r, mut m) = (vec![0f32; s * s * 3], vec![1f32; s * s]);
    for y in 0..h {
        for x in 0..w {
            m[y * s + x] = hole[y * w + x];
            r[(y * s + x) * 3..(y * s + x) * 3 + 3].copy_from_slice(&rgb[(y * w + x) * 3..(y * w + x) * 3 + 3]);
        }
    }
    (r, m)
}

/// The top-left `w × h` of an `s`-wide picture.
fn cut(v: &[f32], s: usize, w: usize, h: usize) -> Vec<f32> {
    (0..h).flat_map(|y| v[y * s * 3..(y * s + w) * 3].iter().copied()).collect()
}

/// Largest and mean difference of `a` and `b` inside the hole, 0..255.
fn diff(a: &[f32], b: &[f32], hole: &[f32]) -> (f32, f32) {
    let (mut max, mut sum, mut n) = (0f32, 0f64, 0usize);
    for (i, m) in hole.iter().enumerate() {
        if *m > 0.0 {
            for c in 0..3 {
                let d = (a[i * 3 + c] - b[i * 3 + c]).abs() * 255.0;
                max = max.max(d);
                sum += f64::from(d);
                n += 1;
            }
        }
    }
    (max, (sum / n.max(1) as f64) as f32)
}

fn png(v: &[f32], w: usize, h: usize) -> image::RgbImage {
    let px: Vec<u8> = v.iter().map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    image::RgbImage::from_raw(w as u32, h as u32, px).unwrap_or_default()
}

fn run() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [lama, portraits, out, sizes @ ..] = &a[..] else {
        return Err("usage: coreml_compare LAMA_DIR PORTRAITS_DIR OUT_DIR [WxH ...]".into());
    };
    let (lama, portraits, out) = (Path::new(lama), Path::new(portraits), Path::new(out));
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let sizes: Vec<(usize, usize)> = if sizes.is_empty() {
        vec![(200, 200), (400, 300), (640, 480)]
    } else {
        sizes.iter().filter_map(|s| s.split_once('x')).filter_map(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).collect()
    };
    let t = Instant::now();
    let candle = Lama::load(&lama.join("big-lama.pt"), &best_device()).map_err(|e| e.to_string())?;
    println!("candle load {:.2} s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let coreml = CoreMlLama::load(lama, DEFAULT_UNITS).map_err(|e| e.to_string())?;
    println!("Core ML load {:.2} s, sides {:?}", t.elapsed().as_secs_f64(), coreml.sides());
    let never = || false;
    let mut worst: Option<(f32, String, usize, usize, [Vec<f32>; 3])> = None;
    for &(w, h) in &sizes {
        let side = side_for(w, h).ok_or(format!("no side holds {w}×{h}"))?;
        for case in &CASES {
            let (rgb, hole) = make(case, portraits, w, h, 0.0)?;
            let (shifted, _) = make(case, portraits, w, h, 2.0)?;
            let t = Instant::now();
            let own = candle.inpaint(&rgb, &hole, w, h, &never).map_err(|e| e.to_string())?;
            let t_candle = t.elapsed().as_secs_f64() * 1e3;
            let padded =
                candle.inpaint(&mirror(&rgb, w, h, side, 3), &mirror(&hole, w, h, side, 1), side, side, &never).map_err(|e| e.to_string())?;
            let padded = cut(&padded, side, w, h);
            // the padding as more hole (the model makes it up too), and the crop moved by 2 px: how much the fill moves anyway
            let (zero, more_hole) = pad_as_hole(&rgb, &hole, w, h, side);
            let as_hole = cut(&candle.inpaint(&zero, &more_hole, side, side, &never).map_err(|e| e.to_string())?, side, w, h);
            let moved = candle.inpaint(&shifted, &hole, w, h, &never).map_err(|e| e.to_string())?;
            let mut times = Vec::new();
            let mut cm = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                cm = coreml.inpaint(&rgb, &hole, w, h).map_err(|e| e.to_string())?;
                times.push(format!("{:.0}", t.elapsed().as_secs_f64() * 1e3));
            }
            let (pm, pa) = diff(&padded, &own, &hole);
            let (cmx, cma) = diff(&cm, &own, &hole);
            let (hm, ha) = diff(&as_hole, &own, &hole);
            let (sm, sa) = diff(&moved, &own, &hole);
            println!(
                "{w}x{h} -> {side}  {:24} mirror {pm:5.1}/{pa:5.2} | as hole {hm:5.1}/{ha:5.2} | moved 2 px {sm:5.1}/{sa:5.2} | Core ML {cmx:5.1}/{cma:5.2} | candle {t_candle:.0} ms, Core ML {} ms",
                case.0,
                times.join("/")
            );
            if worst.as_ref().is_none_or(|w| cma > w.0) {
                worst = Some((cma, format!("{} {w}x{h}", case.0), w, h, [own, cm, moved]));
            }
        }
    }
    if let Some((mean, name, w, h, fills)) = worst {
        let mut sheet = image::RgbImage::from_pixel((3 * w + 16) as u32, h as u32, image::Rgb([255, 255, 255]));
        for (i, f) in fills.iter().enumerate() {
            image::imageops::replace(&mut sheet, &png(f, w, h), (i * (w + 8)) as i64, 0);
        }
        let path = out.join("worst.png");
        sheet.save(&path).map_err(|e| e.to_string())?;
        println!("worst (mean {mean:.2}): {name}; candle, Core ML, candle on the crop moved 2 px: {}", path.display());
    }
    Ok(())
}
