//! AI Denoise on macOS Colour (`files::macos`): the denoised picture comes from LightCraft's own develop of the
//! denoised mosaic, which the Mac's decoder cannot read, so both the plain and the denoised picture are finished
//! through one per-photo colour transfer from LightCraft's white-balanced camera RGB to the Mac's linear base:
//! `apple ≈ gain · M · camera`, `M` a 3×3 whose rows sum to 1 (white stays white). It is fitted at run time on this
//! photo's own pair, block-averaged to about [`BLOCKS`] blocks so the two demosaics' noise averages out, robustly
//! (relative weights, Huber re-weighting, clipped and dark blocks left out). Plan `macos-base-plan.md` §6.6: on 68
//! A7 V photos the median colour error against the Mac's base was 0.23 ΔE00, skin 0.20.
//!
//! Pure: nothing here calls the Mac. A fit the gates refuse ([`Refused`]) leaves the photo on Camera Matching for both
//! pictures, so the AI Denoise Amount slider always blends two pictures of the same colour.
use lightcraft_color::{Mat3, REC2020, SRGB};

use super::Rgb32f;

/// About how many blocks the pair is averaged to (48-pixel blocks at 2560 px).
pub(crate) const BLOCKS: usize = 1900;
/// A block holding a camera sample at or above this (sensor level, before white balance) is left out: clipped.
const CLIP: f32 = 0.95;
/// A block darker than this (the Mac's linear luminance) is left out: noisy.
const DARK: f64 = 0.01;
/// The relative weights' floor: `1 / (Y + FLOOR)²`.
const FLOOR: f64 = 0.02;
/// Huber re-weighting rounds.
const ROUNDS: usize = 8;
/// Fewest usable blocks for a fit (the 68-photo sample's minimum was 716).
pub(crate) const MIN_BLOCKS: usize = 500;
/// Largest magnitude of a matrix entry (the sample's median matrix has entries up to 1.51).
const MAX_ENTRY: f64 = 3.0;
/// The gain's range (the sample's was about 1.27).
const GAIN: std::ops::RangeInclusive<f64> = 0.2..=5.0;
/// Largest median ΔE00 of the fit on its own blocks.
pub(crate) const MAX_ERROR: f64 = 1.5;

/// White-balanced camera RGB → the Mac's linear Rec.2020: `gain · matrix · rgb`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transfer {
    pub matrix: Mat3,
    pub gain: f32,
    /// Blocks the fit used, and its median ΔE00 on them.
    pub blocks: usize,
    pub error: f64,
}

/// Why a fit was refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Refused {
    /// Fewer usable blocks than [`MIN_BLOCKS`] (or pictures of different grids).
    Blocks(usize),
    /// The scene's colours do not tell the matrix apart (one colour, or nearly).
    Singular,
    /// A matrix entry beyond [`MAX_ENTRY`], or not finite.
    Matrix,
    /// The gain outside its range.
    Gain,
    /// The median ΔE00 on the fit's own blocks above [`MAX_ERROR`].
    Error(f64),
}

/// The block grid (columns, rows) for a `width` × `height` picture: about [`BLOCKS`] blocks, never more than pixels.
pub(crate) fn grid(width: usize, height: usize) -> (usize, usize) {
    let (w, h) = (width.max(1), height.max(1));
    let cols = ((BLOCKS as f64 * w as f64 / h as f64).sqrt().round() as usize).clamp(1, w);
    let rows = ((BLOCKS as f64 / cols as f64).round() as usize).clamp(1, h);
    (cols, rows)
}

/// The mean of each block of `img` on a `cols` × `rows` grid; NaN for a block with any pixel `usable` rejects (by its
/// index and value).
fn block_means(img: &Rgb32f, cols: usize, rows: usize, usable: impl Fn(usize, [f32; 3]) -> bool) -> Rgb32f {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let n = cols.saturating_mul(rows);
    let mut sum = vec![[0.0f64; 3]; n];
    let mut count = vec![0u32; n];
    let mut bad = vec![false; n];
    let (w, h) = (img.width.max(1), img.height.max(1));
    for (i, p) in img.data.iter().enumerate() {
        let (x, y) = (i % w, i / w);
        let b = (y * rows / h).min(rows - 1) * cols + (x * cols / w).min(cols - 1);
        let (Some(s), Some(c), Some(bad)) = (sum.get_mut(b), count.get_mut(b), bad.get_mut(b)) else { continue };
        if usable(i, *p) {
            for k in 0..3 {
                s[k] += f64::from(p[k]);
            }
            *c += 1;
        } else {
            *bad = true;
        }
    }
    let data = (0..n)
        .map(|b| match (sum[b], count[b], bad[b]) {
            (s, c, false) if c > 0 => s.map(|v| (v / f64::from(c)) as f32),
            _ => [f32::NAN; 3],
        })
        .collect();
    Rgb32f { width: cols, height: rows, data }
}

/// The mean of each block of `img` on a `cols` × `rows` grid; NaN for a block with a value that is not finite.
pub(crate) fn means(img: &Rgb32f, cols: usize, rows: usize) -> Rgb32f {
    block_means(img, cols, rows, |_, p| p.iter().all(|v| v.is_finite()))
}

/// The camera side of the pair: block means of LightCraft's developed camera RGB `img` (sensor level, before
/// highlight reconstruction and white balance; `mask` the binned clip mask, when binned), white-balanced by `wb`.
/// Clipped blocks are NaN.
pub(crate) fn camera_blocks(img: &Rgb32f, mask: Option<&[u8]>, wb: [f32; 3], cols: usize, rows: usize) -> Rgb32f {
    let usable = |i: usize, p: [f32; 3]| p.iter().all(|v| v.is_finite() && *v < CLIP) && mask.is_none_or(|m| m.get(i).copied().unwrap_or(0) == 0);
    let mut blocks = block_means(img, cols, rows, usable);
    for p in &mut blocks.data {
        *p = std::array::from_fn(|k| p[k] * wb[k]);
    }
    blocks
}

/// The Mac's side of the pair: block means of its linear base; dark blocks are NaN.
pub(crate) fn apple_blocks(img: &Rgb32f, cols: usize, rows: usize) -> Rgb32f {
    let mut blocks = means(img, cols, rows);
    for p in &mut blocks.data {
        let y = luma(p.map(f64::from));
        if y.is_nan() || y < DARK {
            *p = [f32::NAN; 3];
        }
    }
    blocks
}

fn luma(p: [f64; 3]) -> f64 {
    (0..3).map(|i| p[i] * f64::from(lightcraft_color::LUMA_2020[i])).sum()
}

/// Fit the transfer from `camera` ([`camera_blocks`], upright) to `apple` ([`apple_blocks`]), on the same grid, and
/// check it against the gates.
pub(crate) fn fit_transfer(camera: &Rgb32f, apple: &Rgb32f) -> Result<Transfer, Refused> {
    if (camera.width, camera.height) != (apple.width, apple.height) {
        return Err(Refused::Blocks(0));
    }
    let pairs: Vec<([f64; 3], [f64; 3])> = camera
        .data
        .iter()
        .zip(&apple.data)
        .filter(|(c, a)| c.iter().chain(a.iter()).all(|v| v.is_finite()))
        .map(|(c, a)| (c.map(f64::from), a.map(f64::from)))
        .collect();
    if pairs.len() < MIN_BLOCKS {
        return Err(Refused::Blocks(pairs.len()));
    }
    let base: Vec<f64> = pairs.iter().map(|(_, a)| 1.0 / (luma(*a).max(0.0) + FLOOR).powi(2)).collect();
    let mut weight = base.clone();
    let (mut m, mut s) = (Mat3::IDENTITY, 1.0f64);
    for _ in 0..ROUNDS {
        m = solve_rows(&pairs, &weight, s)?;
        s = solve_gain(&pairs, &weight, &m);
        // Huber on the relative residual
        let rel: Vec<f64> = pairs
            .iter()
            .map(|(c, a)| {
                let p = m.apply(*c);
                let r = (0..3).map(|k| (a[k] - s * p[k]).powi(2)).sum::<f64>().sqrt();
                r / (luma(*a).max(0.0) + FLOOR)
            })
            .collect();
        let threshold = 1.345 * 1.4826 * median(rel.clone()) + 1e-9;
        weight = base.iter().zip(&rel).map(|(w, r)| if *r <= threshold { *w } else { w * threshold / r.max(1e-12) }).collect();
    }
    if !m.0.iter().flatten().all(|v| v.is_finite() && v.abs() <= MAX_ENTRY) {
        return Err(Refused::Matrix);
    }
    if !(s.is_finite() && GAIN.contains(&s)) {
        return Err(Refused::Gain);
    }
    let error = in_sample_error(&pairs, &m, s);
    if error.is_nan() || error > MAX_ERROR {
        return Err(Refused::Error(error));
    }
    Ok(Transfer { matrix: m, gain: s as f32, blocks: pairs.len(), error })
}

/// The rows of `M` (each summing to 1) for `apple / s ≈ M · camera`, weighted least squares: per output channel `k`,
/// `a_k / s − c_2 = α (c_0 − c_2) + β (c_1 − c_2)`.
fn solve_rows(pairs: &[([f64; 3], [f64; 3])], weight: &[f64], s: f64) -> Result<Mat3, Refused> {
    let mut rows = [[0.0; 3]; 3];
    for (k, row) in rows.iter_mut().enumerate() {
        let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for ((c, a), w) in pairs.iter().zip(weight) {
            let (z1, z2, t) = (c[0] - c[2], c[1] - c[2], a[k] / s - c[2]);
            a11 += w * z1 * z1;
            a12 += w * z1 * z2;
            a22 += w * z2 * z2;
            b1 += w * z1 * t;
            b2 += w * z2 * t;
        }
        let det = a11 * a22 - a12 * a12;
        if !(det.is_finite() && det > 1e-9 * (a11 + a22).powi(2) && det > 0.0) {
            return Err(Refused::Singular);
        }
        let (alpha, beta) = ((b1 * a22 - b2 * a12) / det, (a11 * b2 - a12 * b1) / det);
        *row = [alpha, beta, 1.0 - alpha - beta];
    }
    Ok(Mat3(rows))
}

/// The gain: the weighted geometric mean of the luminance ratios.
fn solve_gain(pairs: &[([f64; 3], [f64; 3])], weight: &[f64], m: &Mat3) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for ((c, a), w) in pairs.iter().zip(weight) {
        num += w * (luma(*a).max(1e-6) / luma(m.apply(*c)).max(1e-6)).ln();
        den += w;
    }
    if den > 0.0 { (num / den).exp() } else { 1.0 }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return f64::NAN;
    }
    let mid = v.len() / 2;
    let (_, m, _) = v.select_nth_unstable_by(mid, f64::total_cmp);
    *m
}

/// The fit's median ΔE00 on its own blocks, both sides through one display mapping: scaled so the Mac's median
/// luminance is 0.18, sRGB, clipped.
fn in_sample_error(pairs: &[([f64; 3], [f64; 3])], m: &Mat3, s: f64) -> f64 {
    let k = 0.18 / median(pairs.iter().map(|(_, a)| luma(*a)).collect()).max(1e-6);
    let e = pairs
        .iter()
        .map(|(c, a)| {
            let p = m.apply(*c).map(|v| (v * s).max(0.0));
            de2000(display_lab(*a, k), display_lab(p, k))
        })
        .collect();
    median(e)
}

/// CIELAB (D65) of linear Rec.2020 `rgb` scaled by `k`, clipped to sRGB as a display would.
pub(crate) fn display_lab(rgb: [f64; 3], k: f64) -> [f64; 3] {
    let to_srgb = REC2020.to_space(&SRGB);
    let srgb = to_srgb.apply(rgb.map(|v| v * k)).map(|v| v.clamp(0.0, 1.0));
    let xyz = SRGB.to_xyz().apply(srgb);
    let white = lightcraft_color::D65.to_xyz();
    let f = |t: f64| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let [fx, fy, fz] = [f(xyz[0] / white[0]), f(xyz[1] / white[1]), f(xyz[2] / white[2])];
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference.
pub(crate) fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let [l1, a1, b1] = a;
    let [l2, a2, b2] = b;
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let g = 0.5 * (1.0 - (c_bar.powi(7) / (c_bar.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = (a1 * (1.0 + g), a2 * (1.0 + g));
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| if b == 0.0 && a == 0.0 { 0.0 } else { b.atan2(a).rem_euclid(2.0 * PI) };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > PI {
            d - 2.0 * PI
        } else if d < -PI {
            d + 2.0 * PI
        } else {
            d
        }
    };
    let dh_big = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).sin();
    let l_bar = (l1 + l2) / 2.0;
    let c_bar_p = (c1p + c2p) / 2.0;
    let h_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= PI {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 2.0 * PI {
        (h1p + h2p + 2.0 * PI) / 2.0
    } else {
        (h1p + h2p - 2.0 * PI) / 2.0
    };
    let t = 1.0 - 0.17 * (h_bar - PI / 6.0).cos() + 0.24 * (2.0 * h_bar).cos() + 0.32 * (3.0 * h_bar + PI / 30.0).cos()
        - 0.20 * (4.0 * h_bar - 63.0 * PI / 180.0).cos();
    let d_theta = (30.0 * PI / 180.0) * (-((h_bar * 180.0 / PI - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (c_bar_p.powi(7) / (c_bar_p.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (l_bar - 50.0).powi(2) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * c_bar_p;
    let sh = 1.0 + 0.015 * c_bar_p * t;
    let rt = -(2.0 * d_theta).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dh_big / sh).powi(2) + rt * (dc / sc) * (dh_big / sh)).sqrt()
}

#[cfg(test)]
pub(crate) mod tests;
