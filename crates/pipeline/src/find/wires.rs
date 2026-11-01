//! Wires: long, thin, dark, gently curved lines against smooth bright areas (power lines and cables across a sky).
//!
//! Classical and pure Rust. SAM 3 was tried for this and cannot do it: its masks are 288 cells across the photo, one
//! cell is five or more pixels of a 1600 px render, and a wire is two or three (see `fork/find-people-wires.md`).
//!
//! 1. Ridges: the Hessian of the blurred luminance at three scales; a dark line has one strongly positive curvature
//!    (across it) and a near-zero one (along it). Non-maximum suppression across the line thins it to one pixel.
//! 2. Chains: ridge pixels are linked (gaps of a pixel or two bridged) into components; weak ones are dropped unless
//!    they touch a strong pixel (hysteresis).
//! 3. A component is a wire when it is long, and a quadratic fits its centre line closely (a wire sags, it does not
//!    wander), its pixels run along that fitted line, it leaves few gaps, it is darker than the area on both sides, and
//!    the two sides are alike (an edge between sky and a roof is not a wire). A component that is almost one is split at
//!    its worst bend and the parts judged again (a wire kinks at a pole).

use lightcraft_raster::{Plane, Rgba8, blur::gaussian};

use super::shape::{Mask, components, dilate};
use super::{Candidate, Kind, cover};

/// Most wires proposed.
pub const MAX_WIRES: usize = 12;
/// Shortest wire, as a share of the long edge.
const MIN_LEN: f64 = 0.10;
/// Largest photo (pixels) the detector looks at; callers render at 1600 px.
const MAX_PIXELS: usize = 12 << 20;

/// Ridge strength of a dark line at `sigma`, from the Hessian of the blurred luminance.
struct Ridges {
    /// Response (`σ² · (λ1 − |λ2|)`, the best of the scales), 0 where there is no dark ridge.
    r: Vec<f32>,
    /// Unit vector across the ridge.
    n: Vec<(f32, f32)>,
}

fn ridges(l: &Plane, long: f64) -> Ridges {
    let (w, h) = (l.width, l.height);
    let mut best = Ridges { r: vec![0.0; w * h], n: vec![(0.0, 1.0); w * h] };
    let k = (long / 1600.0).max(0.5);
    for sigma in [0.9 * k, 1.5 * k, 2.4 * k] {
        let sigma = sigma.max(0.8) as f32;
        let b = gaussian(l, sigma);
        let at = |x: i64, y: i64| b.data[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize];
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let (xi, yi) = (x as i64, y as i64);
                let c = at(xi, yi);
                let lxx = at(xi + 1, yi) + at(xi - 1, yi) - 2.0 * c;
                let lyy = at(xi, yi + 1) + at(xi, yi - 1) - 2.0 * c;
                let lxy = (at(xi + 1, yi + 1) - at(xi - 1, yi + 1) - at(xi + 1, yi - 1) + at(xi - 1, yi - 1)) / 4.0;
                let mean = (lxx + lyy) / 2.0;
                let d = (((lxx - lyy) / 2.0).powi(2) + lxy * lxy).sqrt();
                let (l1, l2) = (mean + d, mean - d);
                let resp = sigma * sigma * (l1 - l2.abs());
                let i = y * w + x;
                if resp > best.r[i] {
                    best.r[i] = resp;
                    // eigenvector of the larger eigenvalue: across the ridge
                    let (vx, vy) = if lxy.abs() > 1e-9 {
                        (lxy, l1 - lxx)
                    } else if lxx >= lyy {
                        (1.0, 0.0)
                    } else {
                        (0.0, 1.0)
                    };
                    let m = (vx * vx + vy * vy).sqrt().max(1e-12);
                    best.n[i] = (vx / m, vy / m);
                }
            }
        }
    }
    best
}

/// Bilinear sample of `l` (0..1), clamped to the photo.
fn sample(l: &Plane, x: f64, y: f64) -> f32 {
    let (w, h) = (l.width, l.height);
    let fx = x.clamp(0.0, w as f64 - 1.001);
    let fy = y.clamp(0.0, h as f64 - 1.001);
    let (x0, y0) = (fx as usize, fy as usize);
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let g = |x: usize, y: usize| l.data.get(y * w + x).copied().unwrap_or(0.0);
    let top = g(x0, y0) * (1.0 - tx) + g(x0 + 1, y0) * tx;
    let bottom = g(x0, y0 + 1) * (1.0 - tx) + g(x0 + 1, y0 + 1) * tx;
    top * (1.0 - ty) + bottom * ty
}

/// A ridge pixel.
#[derive(Clone, Copy)]
struct Px {
    x: f64,
    y: f64,
    /// Unit vector across the line.
    n: (f64, f64),
    r: f32,
}

/// Least-squares `u ≈ c0 + c1·s + c2·s²`; `None` for a degenerate set.
fn fit_quadratic(pts: &[(f64, f64)]) -> Option<[f64; 3]> {
    let mut a = [[0.0f64; 4]; 3];
    for (s, u) in pts {
        let b = [1.0, *s, s * s];
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += b[i] * b[j];
            }
            a[i][3] += b[i] * u;
        }
    }
    for c in 0..3 {
        let p = (c..3).max_by(|x, y| a[*x][c].abs().total_cmp(&a[*y][c].abs()))?;
        if a[p][c].abs() < 1e-9 {
            return None;
        }
        a.swap(c, p);
        for r in 0..3 {
            if r != c {
                let f = a[r][c] / a[c][c];
                for k in c..4 {
                    a[r][k] -= f * a[c][k];
                }
            }
        }
    }
    Some([a[0][3] / a[0][0], a[1][3] / a[1][1], a[2][3] / a[2][2]])
}

/// What a sensitivity (0..100) means: the least contrast between a wire and the area on both sides (luminance, 0..1).
fn min_contrast(sensitivity: f32) -> f32 {
    0.060 - 0.045 * (sensitivity / 100.0).clamp(0.0, 1.0)
}

struct Found {
    path: Vec<(f64, f64)>,
    width: f64,
    score: f32,
}

/// Judges one chain of ridge pixels; on failure says where it bends worst (an index into `px`) so the caller can split.
fn judge(px: &[Px], l: &Plane, long: f64, c_min: f32, depth: u8) -> Result<Found, Option<usize>> {
    let n = px.len() as f64;
    if px.len() < 8 {
        return Err(None);
    }
    let (mx, my) = (px.iter().map(|p| p.x).sum::<f64>() / n, px.iter().map(|p| p.y).sum::<f64>() / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in px {
        let (dx, dy) = (p.x - mx, p.y - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (ax, ay) = (ang.cos(), ang.sin());
    let (bx, by) = (-ay, ax);
    let tu: Vec<(f64, f64)> = px.iter().map(|p| (((p.x - mx) * ax + (p.y - my) * ay), ((p.x - mx) * bx + (p.y - my) * by))).collect();
    let (t0, t1) = tu.iter().fold((f64::MAX, f64::MIN), |(a, b), (t, _)| (a.min(*t), b.max(*t)));
    let extent = t1 - t0;
    if extent < MIN_LEN * long * if depth > 0 { 0.8 } else { 1.0 } {
        return Err(None);
    }
    let half = extent / 2.0;
    let mid = (t0 + t1) / 2.0;
    let st: Vec<(f64, f64)> = tu.iter().map(|(t, u)| ((t - mid) / half, *u)).collect();
    let Some(c) = fit_quadratic(&st) else { return Err(None) };
    let fit = |s: f64| c[0] + c[1] * s + c[2] * s * s;
    let resid: Vec<f64> = st.iter().map(|(s, u)| u - fit(*s)).collect();
    let rms = (resid.iter().map(|r| r * r).sum::<f64>() / n).sqrt();
    let worst = resid.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i);
    // the sag: how far the middle is from the chord between the ends
    let sag = (fit(0.0) - (fit(-1.0) + fit(1.0)) / 2.0).abs();
    if rms > 0.0012 * long + 0.8 || sag > 0.12 * extent {
        return Err(worst);
    }
    // continuity: most of the length has a ridge pixel, and not a tangle of them
    let mut occupied = vec![false; extent.ceil() as usize + 2];
    for (t, _) in &tu {
        if let Some(o) = occupied.get_mut((t - t0) as usize) {
            *o = true;
        }
    }
    let filled = occupied.iter().filter(|o| **o).count() as f64 / (extent + 1.0);
    if filled < 0.8 || n > 2.5 * extent {
        return Err(worst);
    }
    // direction: the pixels run along the fitted line
    let slope = |s: f64| (c[1] + 2.0 * c[2] * s) / half; // du/dt
    let aligned = px
        .iter()
        .zip(&st)
        .filter(|(p, (s, _))| {
            // the line's direction in photo coordinates is a + slope·b; across it is perpendicular
            let (dx, dy) = (ax + slope(*s) * bx, ay + slope(*s) * by);
            let m = (dx * dx + dy * dy).sqrt().max(1e-12);
            (p.n.0 * dx + p.n.1 * dy).abs() / m < 0.5
        })
        .count() as f64
        / n;
    if aligned < 0.8 {
        return Err(worst);
    }
    // darker than the area on both sides, and the sides alike; the width comes from the same profiles
    let (mut good, mut widths, mut contrasts) = (0usize, Vec::new(), Vec::new());
    for p in px {
        let centre = [-1.0f64, 0.0, 1.0].into_iter().map(|d| sample(l, p.x + p.n.0 * d, p.y + p.n.1 * d)).fold(f32::MAX, f32::min);
        let side = |sign: f64| {
            let v: Vec<f32> = (0..4).map(|k| sample(l, p.x + sign * p.n.0 * (3.5 + k as f64), p.y + sign * p.n.1 * (3.5 + k as f64))).collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        let (a, b) = (side(1.0), side(-1.0));
        let contrast = (a + b) / 2.0 - centre;
        if contrast >= c_min && (a - b).abs() <= 0.5 * contrast + 0.015 {
            good += 1;
            contrasts.push(contrast);
            // width: steps along the normal until the profile is back above half the contrast
            let half_level = centre + contrast / 2.0;
            let reach = |sign: f64| {
                (0..12).find(|k| sample(l, p.x + sign * p.n.0 * *k as f64, p.y + sign * p.n.1 * *k as f64) >= half_level).unwrap_or(12) as f64
            };
            widths.push(reach(1.0) + reach(-1.0));
        }
    }
    if (good as f64) < 0.75 * n {
        return Err(worst);
    }
    widths.sort_by(f64::total_cmp);
    contrasts.sort_by(f32::total_cmp);
    let width = widths.get(widths.len() / 2).copied().unwrap_or(2.0).clamp(1.0, 14.0);
    let contrast = contrasts.get(contrasts.len() / 2).copied().unwrap_or(0.0);
    // the path: the median offset in bins along the line (it follows the real wire), gaps from the fit
    let bin = 6.0;
    let nb = (extent / bin).ceil() as usize + 1;
    let mut bins: Vec<Vec<f64>> = vec![Vec::new(); nb];
    for (t, u) in &tu {
        if let Some(b) = bins.get_mut(((t - t0) / bin) as usize) {
            b.push(*u);
        }
    }
    let mut path = Vec::new();
    for (i, b) in bins.iter_mut().enumerate() {
        let t = (t0 + (i as f64 + 0.5) * bin).min(t1);
        let u = if b.is_empty() {
            fit((t - mid) / half)
        } else {
            b.sort_by(f64::total_cmp);
            b[b.len() / 2]
        };
        path.push((mx + ax * t + bx * u, my + ay * t + by * u));
    }
    // the ends reach the end of the chain
    for (t, side) in [(t0, true), (t1, false)] {
        let u = fit((t - mid) / half);
        let p = (mx + ax * t + bx * u, my + ay * t + by * u);
        if side {
            path.insert(0, p);
        } else {
            path.push(p);
        }
    }
    let score = (contrast / (c_min * 3.0)).clamp(0.0, 1.0) * 0.5 + 0.5 * (good as f32 / n as f32);
    Ok(Found { path, width, score })
}

/// Judge a chain, splitting it at its worst bend when it is almost a wire.
fn analyse(px: &[Px], l: &Plane, long: f64, c_min: f32, depth: u8, out: &mut Vec<Found>) {
    match judge(px, l, long, c_min, depth) {
        Ok(f) => out.push(f),
        Err(Some(at)) if depth < 2 && px.len() > 24 => {
            // order along the chain's own axis is not kept; split by position of the worst pixel along x or y
            let p = px[at];
            let spread_x = px.iter().map(|q| q.x).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
            let spread_y = px.iter().map(|q| q.y).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
            let by_x = spread_x.1 - spread_x.0 >= spread_y.1 - spread_y.0;
            let (left, right): (Vec<Px>, Vec<Px>) = px.iter().partition(|q| if by_x { q.x < p.x } else { q.y < p.y });
            analyse(&left, l, long, c_min, depth + 1, out);
            analyse(&right, l, long, c_min, depth + 1, out);
        }
        _ => {}
    }
}

/// Wires in a rendered image; `sensitivity` 0..100 (higher finds fainter wires).
pub fn detect(img: &Rgba8, sensitivity: f32) -> Vec<Candidate> {
    let (w, h) = (img.width, img.height);
    if w < 64 || h < 64 || w.saturating_mul(h) > MAX_PIXELS || img.data.len() != w * h {
        return Vec::new();
    }
    let long = w.max(h) as f64;
    let l = Plane::from_fn(w, h, |x, y| {
        let p = img.data[y * w + x];
        (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0
    });
    let c_min = min_contrast(sensitivity);
    let rd = ridges(&l, long);
    let hi = 0.25 * c_min;
    let lo = 0.4 * hi;
    // thin to one pixel across the ridge
    let edge = 3usize;
    let mut ridge = Mask::new(w, h);
    for y in edge..h - edge {
        for x in edge..w - edge {
            let i = y * w + x;
            if rd.r[i] <= lo {
                continue;
            }
            let (nx, ny) = rd.n[i];
            let (dx, dy) = (nx.round() as i64, ny.round() as i64);
            let at = |s: i64| rd.r[((y as i64 + s * dy) as usize) * w + (x as i64 + s * dx) as usize];
            if rd.r[i] >= at(1) && rd.r[i] > at(-1) {
                ridge.bits[i] = true;
            }
        }
    }
    // bridge gaps of a pixel or two when linking, then keep the ridge pixels of each link
    let linked = dilate(&ridge, 1.5);
    let mut out: Vec<(f64, Found)> = Vec::new();
    for comp in components(&linked, 400) {
        let px: Vec<Px> = comp
            .mask
            .bits
            .iter()
            .enumerate()
            .filter(|(i, b)| **b && ridge.bits[*i])
            .map(|(i, _)| Px { x: (i % w) as f64 + 0.5, y: (i / w) as f64 + 0.5, n: (f64::from(rd.n[i].0), f64::from(rd.n[i].1)), r: rd.r[i] })
            .collect();
        if px.len() < 8 || !px.iter().any(|p| p.r > hi) {
            continue;
        }
        let mut found = Vec::new();
        analyse(&px, &l, long, c_min, 0, &mut found);
        for f in found {
            let len: f64 = f.path.windows(2).map(|s| (s[1].0 - s[0].0).hypot(s[1].1 - s[0].1)).sum();
            out.push((len, f));
        }
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.truncate(MAX_WIRES);
    out.into_iter()
        .map(|(len, f)| {
            let norm: Vec<(f64, f64)> = f.path.iter().map(|p| ((p.0 / w as f64).clamp(0.0, 1.0), (p.1 / h as f64).clamp(0.0, 1.0))).collect();
            let (mut x0, mut y0, mut x1, mut y1) = (1.0f64, 1.0f64, 0.0f64, 0.0f64);
            for p in &norm {
                (x0, y0, x1, y1) = (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1));
            }
            Candidate {
                kind: Kind::Wire,
                source: "lines",
                outline: norm,
                closed: false,
                bbox: [x0, y0, x1, y1],
                area: len * f.width / (w * h) as f64,
                score: f.score,
                main: false,
                spots: cover::cover_path(&f.path, f.width, w, h),
            }
        })
        .collect()
}
