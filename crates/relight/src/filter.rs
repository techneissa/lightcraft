//! The filters the field is made with: box means, a guided filter, and joint-bilateral upsampling. Planes are
//! row-major `f32`, `w × h`; every index is checked or comes from a loop bounded by the plane's own size.

use rayon::prelude::*;

/// The mean of `src` over a `(2r + 1)²` box around each pixel, the box clipped at the borders (each mean is over
/// the pixels inside).
pub fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if w == 0 || h == 0 || src.len() != w * h {
        return src.to_vec();
    }
    // rows, then columns, each with a running sum in f64 (no drift over long rows)
    let mut rows = vec![0f32; w * h];
    rows.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, row)| mean_1d(row, out, r));
    let mut cols_t = vec![0f32; w * h];
    // transpose so columns are contiguous
    let t = transpose(&rows, w, h);
    cols_t.par_chunks_mut(h).zip(t.par_chunks(h)).for_each(|(out, col)| mean_1d(col, out, r));
    transpose(&cols_t, h, w)
}

fn mean_1d(src: &[f32], out: &mut [f32], r: usize) {
    let n = src.len();
    if n == 0 {
        return;
    }
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0f64);
    let mut acc = 0f64;
    for v in src {
        acc += f64::from(*v);
        prefix.push(acc);
    }
    for (i, o) in out.iter_mut().enumerate() {
        let lo = i.saturating_sub(r);
        let hi = (i + r + 1).min(n);
        let (a, b) = (prefix.get(lo).copied().unwrap_or(0.0), prefix.get(hi).copied().unwrap_or(acc));
        *o = ((b - a) / (hi - lo).max(1) as f64) as f32;
    }
}

/// `src` (`w × h`) transposed (`h × w`).
pub fn transpose(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = src.get(y * w + x).copied().unwrap_or(0.0);
        }
    });
    out
}

/// He et al.'s guided filter of `p` with guide `g` (both `w × h`): edges of the guide are kept, variations of
/// `p` smaller than `eps` (a variance) inside a `(2r + 1)²` box are smoothed away.
pub fn guided(g: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let n = w * h;
    if g.len() != n || p.len() != n || n == 0 {
        return p.to_vec();
    }
    let mean_g = box_mean(g, w, h, r);
    let mean_p = box_mean(p, w, h, r);
    let gg: Vec<f32> = g.par_iter().map(|v| v * v).collect();
    let gp: Vec<f32> = g.par_iter().zip(p).map(|(a, b)| a * b).collect();
    let corr_gg = box_mean(&gg, w, h, r);
    let corr_gp = box_mean(&gp, w, h, r);
    let mut a = vec![0f32; n];
    let mut b = vec![0f32; n];
    a.par_iter_mut().zip(b.par_iter_mut()).enumerate().for_each(|(i, (a, b))| {
        let (mg, mp) = (mean_g.get(i).copied().unwrap_or(0.0), mean_p.get(i).copied().unwrap_or(0.0));
        let var = corr_gg.get(i).copied().unwrap_or(0.0) - mg * mg;
        let cov = corr_gp.get(i).copied().unwrap_or(0.0) - mg * mp;
        *a = cov / (var.max(0.0) + eps);
        *b = mp - *a * mg;
    });
    let (ma, mb) = (box_mean(&a, w, h, r), box_mean(&b, w, h, r));
    (0..n).into_par_iter().map(|i| ma.get(i).copied().unwrap_or(0.0) * g.get(i).copied().unwrap_or(0.0) + mb.get(i).copied().unwrap_or(0.0)).collect()
}

/// A guide colour: three channels compared by their Euclidean distance.
pub type Guide = [f32; 3];

/// How [`joint_bilateral_up`] weighs its samples.
#[derive(Clone, Copy, Debug)]
pub struct Upsample {
    pub sigma_s: f32,
    pub sigma_r: f32,
    pub split: f32,
}

/// Joint-bilateral upsampling (Kopf et al. 2007): the low-resolution plane `lo` (`lw × lh`) brought to `w × h`,
/// each output from the `5 × 5` low-resolution samples around it, weighted by distance on the low grid (`sigma_s`,
/// in low pixels) and by how far the guide colour at the output (`hi_guide`) is from the guide colour of the
/// sample (`lo_guide`, the guide averaged over the sample's cell; `sigma_r`). So the plane's edges move onto the
/// guide's: depth stops at the photo's outlines. Where the weighted samples agree (their weighted standard
/// deviation is under `split`) the output is their weighted mean; where they don't (an edge between near and far)
/// it is their weighted median, so an edge pixel takes one side's depth, never a depth in between (a point floating
/// between the two that no surface has).
pub fn joint_bilateral_up(
    lo: &[f32],
    (lw, lh): (usize, usize),
    lo_guide: &[Guide],
    hi_guide: &[Guide],
    (w, h): (usize, usize),
    k: &Upsample,
) -> Vec<f32> {
    let Upsample { sigma_s, sigma_r, split } = *k;
    let n = w * h;
    if lo.len() != lw * lh || lo_guide.len() != lo.len() || hi_guide.len() != n || lw == 0 || lh == 0 || n == 0 {
        return vec![0.0; n];
    }
    let (kx, ky) = (lw as f32 / w as f32, lh as f32 / h as f32);
    let (is2, ir2) = (1.0 / (2.0 * sigma_s * sigma_s).max(1e-6), 1.0 / (2.0 * sigma_r * sigma_r).max(1e-12));
    let split2 = split * split;
    let mut out = vec![0f32; n];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 + 0.5) * ky - 0.5;
        let cy = fy.round() as isize;
        let mut taps: Vec<(f32, f32)> = Vec::with_capacity(25);
        for (x, o) in row.iter_mut().enumerate() {
            let fx = (x as f32 + 0.5) * kx - 0.5;
            let cx = fx.round() as isize;
            let gp = hi_guide.get(y * w + x).copied().unwrap_or([0.0; 3]);
            taps.clear();
            let (mut nearest, mut nearest_d) = (0f32, f32::MAX);
            for dy in -2isize..=2 {
                let qy = cy + dy;
                if qy < 0 || qy >= lh as isize {
                    continue;
                }
                for dx in -2isize..=2 {
                    let qx = cx + dx;
                    if qx < 0 || qx >= lw as isize {
                        continue;
                    }
                    let q = qy as usize * lw + qx as usize;
                    let (Some(v), Some(gq)) = (lo.get(q), lo_guide.get(q)) else { continue };
                    let ds = (qx as f32 - fx).powi(2) + (qy as f32 - fy).powi(2);
                    let dr = (gp[0] - gq[0]).powi(2) + (gp[1] - gq[1]).powi(2) + (gp[2] - gq[2]).powi(2);
                    taps.push((*v, (-(ds * is2) - dr * ir2).exp()));
                    if ds < nearest_d {
                        nearest_d = ds;
                        nearest = *v;
                    }
                }
            }
            let sw: f32 = taps.iter().map(|t| t.1).sum();
            // (no sample like the pixel at all: the nearest one)
            if sw.is_nan() || sw <= 1e-12 {
                *o = nearest;
                continue;
            }
            let mean = taps.iter().map(|(v, wt)| v * wt).sum::<f32>() / sw;
            let var = taps.iter().map(|(v, wt)| wt * (v - mean) * (v - mean)).sum::<f32>() / sw;
            *o = if var <= split2 || !var.is_finite() {
                if mean.is_finite() { mean } else { nearest }
            } else {
                taps.sort_by(|a, b| a.0.total_cmp(&b.0));
                let half = 0.5 * sw;
                let mut acc = 0.0;
                let mut m = nearest;
                for (v, wt) in &taps {
                    acc += wt;
                    m = *v;
                    if acc >= half {
                        break;
                    }
                }
                m
            };
        }
    });
    out
}

/// `src` (`w × h` guide colours) averaged over the cells of a `lw × lh` grid laid over it.
pub fn area_down(src: &[Guide], (w, h): (usize, usize), (lw, lh): (usize, usize)) -> Vec<Guide> {
    let mut out = vec![[0f32; 3]; lw * lh];
    if src.len() != w * h || w == 0 || h == 0 || lw == 0 || lh == 0 {
        return out;
    }
    out.par_chunks_mut(lw).enumerate().for_each(|(qy, row)| {
        let y0 = qy * h / lh;
        let y1 = ((qy + 1) * h / lh).max(y0 + 1).min(h);
        for (qx, o) in row.iter_mut().enumerate() {
            let x0 = qx * w / lw;
            let x1 = ((qx + 1) * w / lw).max(x0 + 1).min(w);
            let mut acc = [0f64; 3];
            let mut k = 0usize;
            for y in y0..y1 {
                for x in x0..x1 {
                    if let Some(g) = src.get(y * w + x) {
                        acc.iter_mut().zip(g).for_each(|(a, v)| *a += f64::from(*v));
                        k += 1;
                    }
                }
            }
            let k = k.max(1) as f64;
            *o = acc.map(|a| (a / k) as f32);
        }
    });
    out
}

/// The value of `src` (`w × h`) at `(x, y)` (pixel centres at `i + 0.5`), bilinear, clamped at the borders.
#[inline]
pub fn bilinear(src: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let (Some(xm), Some(ym)) = (w.checked_sub(1), h.checked_sub(1)) else { return 0.0 };
    let fx = (x - 0.5).clamp(0.0, xm as f32);
    let fy = (y - 0.5).clamp(0.0, ym as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(xm), (y0 + 1).min(ym));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: usize, y: usize| src.get(y * w + x).copied().unwrap_or(0.0);
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    top + (bottom - top) * ty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_mean_of_a_constant_is_the_constant_and_borders_are_clipped() {
        let src = vec![3.0f32; 7 * 5];
        assert!(box_mean(&src, 7, 5, 2).iter().all(|v| (v - 3.0).abs() < 1e-6));
        let ramp: Vec<f32> = (0..5).map(|x| x as f32).collect();
        // at the left border the box holds 0, 1, 2
        assert!((box_mean(&ramp, 5, 1, 2)[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_guided_filter_keeps_a_step_of_its_guide_and_smooths_small_ripples() {
        let (w, h) = (40, 4);
        // a step of 1.0 with a ripple of 0.01 on both sides
        let p: Vec<f32> = (0..w * h).map(|i| if i % w < 20 { 0.0 } else { 1.0 } + if i % 2 == 0 { 0.01 } else { -0.01 }).collect();
        let q = guided(&p, &p, w, h, 4, 1e-3);
        assert!(q[10].abs() < 0.005 && (q[30] - 1.0).abs() < 0.005, "{} {}", q[10], q[30]);
        assert!(q[19] < 0.1 && q[20] > 0.9, "the step stays sharp: {} {}", q[19], q[20]);
    }

    const K: Upsample = Upsample { sigma_s: 1.0, sigma_r: 0.1, split: 0.05 };

    #[test]
    fn joint_bilateral_upsampling_puts_a_soft_edge_on_the_guides() {
        // low: 8 × 1 with the edge between samples 3 and 4; the guide's edge is at output pixel 35 of 64
        let lo: Vec<f32> = (0..8).map(|i| if i < 4 { 1.0 } else { 5.0 }).collect();
        let hi_guide: Vec<Guide> = (0..64).map(|x| if x < 35 { [0.0; 3] } else { [1.0; 3] }).collect();
        let lo_guide = area_down(&hi_guide, (64, 1), (8, 1));
        let up = joint_bilateral_up(&lo, (8, 1), &lo_guide, &hi_guide, (64, 1), &K);
        assert!(up[33] < 1.2 && up[34] < 1.2, "{:?}", &up[30..38]);
        assert!(up[36] > 4.8, "{:?}", &up[30..38]);
        // hostile sizes are no panic
        assert_eq!(joint_bilateral_up(&lo, (9, 1), &lo_guide, &hi_guide, (64, 1), &K), vec![0.0; 64]);
        // an edge pixel whose guide matches neither side takes one side's value, never one in between
        let mid: Vec<Guide> = (0..64)
            .map(|x| {
                if x < 35 {
                    [0.0; 3]
                } else if x == 35 {
                    [0.5; 3]
                } else {
                    [1.0; 3]
                }
            })
            .collect();
        let lo_mid = area_down(&mid, (64, 1), (8, 1));
        let up = joint_bilateral_up(&lo, (8, 1), &lo_mid, &mid, (64, 1), &K);
        assert!(up.iter().all(|v| (v - 1.0).abs() < 0.3 || (v - 5.0).abs() < 0.3), "{:?}", &up[30..40]);
    }

    #[test]
    fn bilinear_reads_between_pixels_and_clamps() {
        let p = vec![0.0, 1.0, 2.0, 3.0];
        assert!((bilinear(&p, 2, 2, 1.0, 1.0) - 1.5).abs() < 1e-6);
        assert_eq!(bilinear(&p, 2, 2, -5.0, -5.0), 0.0);
        assert_eq!(bilinear(&p, 2, 2, 9.0, 9.0), 3.0);
        assert_eq!(bilinear(&[], 0, 0, 0.5, 0.5), 0.0);
    }
}
