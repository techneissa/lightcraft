//! Photo statistics: what the encoder can't see. DINOv2 is trained to ignore brightness and colour casts, which
//! are exactly what Exposure, Temp and Tint answer to, so each photo also gets [`PHOTO_DIMS`] measured numbers:
//! its brightness percentiles and histogram, clipping, colour casts in the shadows and highlights, how much of
//! each mixer hue it holds, local contrast, and where its light sits in the frame.
//!
//! All of them come from the photo developed at its default settings (never the edited one: that would hand the
//! answer to the learner), as 8-bit sRGB of any size.

use crate::{Error, Result};

/// Numbers per photo.
pub const PHOTO_DIMS: usize = 47;
/// Largest photo measured; bigger ones are an error (callers pass previews of a few hundred pixels).
const MAX_PIXELS: usize = 40_000_000;
/// Pixels sampled for the statistics (a regular grid over the photo).
const SAMPLES: usize = 160 * 160;
/// Hue centres (degrees, CIELAB hue angle) of the colour mixer's eight bands, red to magenta.
const BANDS: [f32; 8] = [25.0, 55.0, 85.0, 135.0, 195.0, 255.0, 295.0, 335.0];

/// Weights of an antialiased bicubic resampling (a = −0.5, the kernel widened by the scale when shrinking) from
/// `n_in` samples to `n_out`, per output sample: `(input index, weight)`, weights summing to 1.
pub fn resample_weights(n_in: usize, n_out: usize) -> Vec<Vec<(usize, f32)>> {
    let (n_in, n_out) = (n_in.max(1), n_out.max(1));
    let scale = n_in as f64 / n_out as f64;
    let support = 2.0 * scale.max(1.0);
    let inv = 1.0 / scale.max(1.0);
    let cubic = |x: f64| {
        let (x, a) = (x.abs(), -0.5);
        if x < 1.0 {
            ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
        } else if x < 2.0 {
            (((x - 5.0) * x + 8.0) * x - 4.0) * a
        } else {
            0.0
        }
    };
    (0..n_out)
        .map(|o| {
            let centre = (o as f64 + 0.5) * scale;
            let lo = (centre - support).floor().max(0.0) as usize;
            let hi = ((centre + support).ceil() as usize).min(n_in);
            let mut w: Vec<(usize, f64)> = (lo..hi).map(|i| (i, cubic((i as f64 + 0.5 - centre) * inv))).filter(|(_, w)| *w != 0.0).collect();
            let sum: f64 = w.iter().map(|(_, w)| w).sum();
            if sum.abs() < 1e-12 {
                let nearest = (centre as usize).min(n_in - 1);
                w = vec![(nearest, 1.0)];
            } else {
                w.iter_mut().for_each(|(_, v)| *v /= sum);
            }
            w.into_iter().map(|(i, v)| (i, v as f32)).collect()
        })
        .collect()
}

fn check(width: usize, height: usize, rgb: &[u8]) -> Result<()> {
    let n = width.checked_mul(height).filter(|n| *n > 0 && *n <= MAX_PIXELS).ok_or_else(|| Error::Input(format!("a {width} × {height} photo can't be measured")))?;
    if n.checked_mul(3) != Some(rgb.len()) {
        return Err(Error::Input(format!("{} bytes for a {width} × {height} RGB photo", rgb.len())));
    }
    Ok(())
}

/// `rgb` resampled to `ow × oh`, as interleaved RGB in 0..1 (still sRGB-encoded).
pub fn resample_rgb(width: usize, height: usize, rgb: &[u8], ow: usize, oh: usize) -> Result<Vec<f32>> {
    check(width, height, rgb)?;
    let (wx, wy) = (resample_weights(width, ow), resample_weights(height, oh));
    // rows first: width → ow for every input row
    let mut rows = vec![0f32; height * ow * 3];
    for y in 0..height {
        for (x, taps) in wx.iter().enumerate() {
            for c in 0..3 {
                let v: f32 = taps.iter().map(|&(i, w)| w * rgb.get((y * width + i) * 3 + c).copied().unwrap_or(0) as f32).sum();
                if let Some(slot) = rows.get_mut((y * ow + x) * 3 + c) {
                    *slot = v / 255.0;
                }
            }
        }
    }
    let mut out = vec![0f32; oh * ow * 3];
    for (y, taps) in wy.iter().enumerate() {
        for x in 0..ow {
            for c in 0..3 {
                let v: f32 = taps.iter().map(|&(i, w)| w * rows.get((i * ow + x) * 3 + c).copied().unwrap_or(0.0)).sum();
                if let Some(slot) = out.get_mut((y * ow + x) * 3 + c) {
                    *slot = v.clamp(0.0, 1.0);
                }
            }
        }
    }
    Ok(out)
}

fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// One sampled pixel in CIELAB (D65) with its linear luminance.
#[derive(Clone, Copy)]
struct Px {
    y: f32,
    l: f32,
    a: f32,
    b: f32,
    max: u8,
    min: u8,
    /// Position in the frame, 0..1.
    fx: f32,
    fy: f32,
}

fn lab_f(t: f32) -> f32 {
    if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 }
}

fn sample(width: usize, height: usize, rgb: &[u8], lut: &[f32; 256]) -> Vec<Px> {
    let step = ((width * height) as f64 / SAMPLES as f64).sqrt().max(1.0);
    let (nx, ny) = (((width as f64 / step).floor() as usize).max(1), ((height as f64 / step).floor() as usize).max(1));
    let mut out = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        let y = (((j as f64 + 0.5) * step) as usize).min(height - 1);
        for i in 0..nx {
            let x = (((i as f64 + 0.5) * step) as usize).min(width - 1);
            let o = (y * width + x) * 3;
            let (Some(&r8), Some(&g8), Some(&b8)) = (rgb.get(o), rgb.get(o + 1), rgb.get(o + 2)) else { continue };
            let (r, g, b) = (lut[r8 as usize], lut[g8 as usize], lut[b8 as usize]);
            let xx = 0.4124 * r + 0.3576 * g + 0.1805 * b;
            let yy = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let zz = 0.0193 * r + 0.1192 * g + 0.9505 * b;
            let (fx, fy, fz) = (lab_f(xx / 0.95047), lab_f(yy), lab_f(zz / 1.08883));
            out.push(Px {
                y: yy,
                l: 116.0 * fy - 16.0,
                a: 500.0 * (fx - fy),
                b: 200.0 * (fy - fz),
                max: r8.max(g8).max(b8),
                min: r8.min(g8).min(b8),
                fx: (x as f32 + 0.5) / width as f32,
                fy: (y as f32 + 0.5) / height as f32,
            });
        }
    }
    out
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f32 * p).round() as usize;
    sorted.get(i.min(sorted.len() - 1)).copied().unwrap_or(0.0)
}

fn mean_of(px: &[Px], f: impl Fn(&Px) -> f32) -> f32 {
    if px.is_empty() { 0.0 } else { px.iter().map(f).sum::<f32>() / px.len() as f32 }
}

/// The [`PHOTO_DIMS`] statistics of an sRGB photo (`width × height × 3` bytes).
pub fn photo_stats(width: usize, height: usize, rgb: &[u8]) -> Result<Vec<f32>> {
    check(width, height, rgb)?;
    let mut lut = [0f32; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = linear(i as u8);
    }
    let px = sample(width, height, rgb, &lut);
    if px.is_empty() {
        return Err(Error::Input("no pixels to measure".into()));
    }
    let n = px.len() as f32;
    let mut out = Vec::with_capacity(PHOTO_DIMS);
    // brightness: the log-average (the scene's key), then L* percentiles and spread
    out.push(mean_of(&px, |p| (p.y + 1e-4).log2()) / 8.0);
    let mut ls: Vec<f32> = px.iter().map(|p| p.l).collect();
    ls.sort_by(f32::total_cmp);
    for p in [0.01, 0.05, 0.10, 0.25, 0.50, 0.75, 0.90, 0.95, 0.99] {
        out.push(percentile(&ls, p) / 100.0);
    }
    let lmean = mean_of(&px, |p| p.l);
    out.push((mean_of(&px, |p| (p.l - lmean).powi(2))).sqrt() / 100.0);
    // clipping
    out.push(px.iter().filter(|p| p.max >= 250).count() as f32 / n);
    out.push(px.iter().filter(|p| p.min <= 4 && p.max <= 6).count() as f32 / n);
    // L* histogram in eight bins
    let mut hist = [0f32; 8];
    for p in &px {
        hist[((p.l / 12.5).floor().clamp(0.0, 7.0)) as usize] += 1.0;
    }
    out.extend(hist.iter().map(|h| h / n));
    // colour casts: everything, the highlights (above the 90th percentile) and the shadows (below the 25th)
    let (p90, p25) = (percentile(&ls, 0.90), percentile(&ls, 0.25));
    let hi: Vec<Px> = px.iter().copied().filter(|p| p.l >= p90).collect();
    let lo: Vec<Px> = px.iter().copied().filter(|p| p.l <= p25).collect();
    for set in [&px, &hi, &lo] {
        out.push(mean_of(set, |p| p.a) / 50.0);
        out.push(mean_of(set, |p| p.b) / 50.0);
    }
    // chroma: its mean and 90th percentile, then how much of each mixer band the photo holds
    let mut cs: Vec<f32> = px.iter().map(|p| p.a.hypot(p.b)).collect();
    out.push(cs.iter().sum::<f32>() / n / 50.0);
    cs.sort_by(f32::total_cmp);
    out.push(percentile(&cs, 0.90) / 50.0);
    let mut bands = [0f32; 8];
    let mut skin = 0f32;
    for p in &px {
        let c = p.a.hypot(p.b);
        if c < 4.0 {
            continue;
        }
        let h = p.b.atan2(p.a).to_degrees().rem_euclid(360.0);
        // soft assignment to the two nearest band centres
        let dist = |centre: f32| {
            let d = (h - centre).abs();
            d.min(360.0 - d)
        };
        let mut best = [(f32::MAX, 0usize); 2];
        for (i, &centre) in BANDS.iter().enumerate() {
            let d = dist(centre);
            if d < best[0].0 {
                best[1] = best[0];
                best[0] = (d, i);
            } else if d < best[1].0 {
                best[1] = (d, i);
            }
        }
        let total = (best[0].0 + best[1].0).max(1e-3);
        bands[best[0].1] += c * best[1].0 / total;
        bands[best[1].1] += c * best[0].0 / total;
        if (20.0..60.0).contains(&h) && (10.0..45.0).contains(&c) && (30.0..88.0).contains(&p.l) {
            skin += 1.0;
        }
    }
    out.extend(bands.iter().map(|b| b / n / 50.0));
    out.push(skin / n);
    // local contrast: the mean difference to the neighbouring sample, and its 90th percentile
    let step = ((width * height) as f64 / SAMPLES as f64).sqrt().max(1.0);
    let nx = ((width as f64 / step).floor() as usize).max(1);
    let mut grads: Vec<f32> = Vec::with_capacity(px.len());
    for (i, p) in px.iter().enumerate() {
        let right = if (i + 1) % nx != 0 { px.get(i + 1) } else { None };
        let down = px.get(i + nx);
        let g = right.map_or(0.0, |q| (q.l - p.l).abs()) + down.map_or(0.0, |q| (q.l - p.l).abs());
        grads.push(g);
    }
    out.push(grads.iter().sum::<f32>() / n / 20.0);
    grads.sort_by(f32::total_cmp);
    out.push(percentile(&grads, 0.90) / 40.0);
    // where the light is: top third against bottom third, centre against the border, left against right
    let region = |f: &dyn Fn(&Px) -> bool| {
        let sel: Vec<f32> = px.iter().filter(|p| f(p)).map(|p| p.l).collect();
        if sel.is_empty() { lmean } else { sel.iter().sum::<f32>() / sel.len() as f32 }
    };
    out.push((region(&|p| p.fy < 1.0 / 3.0) - region(&|p| p.fy > 2.0 / 3.0)) / 100.0);
    let centre = |p: &Px| (p.fx - 0.5).abs() < 0.25 && (p.fy - 0.5).abs() < 0.25;
    out.push((region(&centre) - region(&|p| !centre(p))) / 100.0);
    out.push((region(&|p| p.fx < 0.5) - region(&|p| p.fx >= 0.5)) / 100.0);
    // blue in the top third (sky), and the darkest and brightest tenth's share of the frame
    let top: Vec<Px> = px.iter().copied().filter(|p| p.fy < 1.0 / 3.0).collect();
    out.push(top.iter().filter(|p| p.b < -8.0 && p.l > 40.0).count() as f32 / top.len().max(1) as f32);
    out.push(px.iter().filter(|p| p.l < 15.0).count() as f32 / n);
    out.push(px.iter().filter(|p| p.l > 85.0).count() as f32 / n);
    out.push(width as f32 / height as f32 / 2.0);
    if out.len() != PHOTO_DIMS || out.iter().any(|v| !v.is_finite()) {
        return Err(Error::Input("the photo's statistics are not finite".into()));
    }
    Ok(out)
}
