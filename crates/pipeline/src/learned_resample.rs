//! Fork feature (`fork/lama-fill.md`): moving a crop between the size of a render and the size a learned fill works at.
//!
//! A preview, a loupe and an export draw the same spot at different sizes, but the model must see it at one size
//! (set by the hole in the original's pixels, see [`crate::spots_learned`]), or the three would not agree. These
//! helpers scale a crop there and back: an area average going down, bilinear between pixel centres going up.

/// The size `(w, h)` a `cw × ch` crop of a render is fed to the fill at: scaled by `k` (the render's pixels to the
/// original's), but at most `max_side` on its longer side. Never zero, never non-finite.
pub fn fill_size(cw: usize, ch: usize, k: f64, max_side: usize) -> (usize, usize) {
    let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
    let long = cw.max(ch) as f64 * k;
    let k = if long > max_side as f64 { k * max_side as f64 / long } else { k };
    let one = |n: usize| ((n as f64 * k).round() as usize).clamp(1, max_side.max(1));
    (one(cw), one(ch))
}

/// `src` (`sw × sh`, `n` values per pixel) scaled to `dw × dh`. Empty when a size is zero or `src` does not fit it.
pub fn scale(src: &[f32], (sw, sh): (usize, usize), (dw, dh): (usize, usize), n: usize) -> Vec<f32> {
    let fits = sw.checked_mul(sh).and_then(|p| p.checked_mul(n)) == Some(src.len());
    if !fits || sw == 0 || sh == 0 || dw == 0 || dh == 0 || n == 0 {
        return Vec::new();
    }
    if (sw, sh) == (dw, dh) {
        return src.to_vec();
    }
    let down = sw > dw || sh > dh;
    let mut out = vec![0f32; dw * dh * n];
    for y in 0..dh {
        for x in 0..dw {
            let o = (y * dw + x) * n;
            if down {
                average(src, (sw, sh), (dw, dh), (x, y), n, &mut out[o..o + n]);
            } else {
                bilinear(src, (sw, sh), (dw, dh), (x, y), n, &mut out[o..o + n]);
            }
        }
    }
    out
}

/// `hole` (`sw × sh`) scaled to `dw × dh` so that a destination pixel is in the hole when any source pixel it covers is.
pub fn scale_hole(hole: &[f32], (sw, sh): (usize, usize), (dw, dh): (usize, usize)) -> Vec<f32> {
    if sw.checked_mul(sh) != Some(hole.len()) || sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Vec::new();
    }
    let mut out = vec![0f32; dw * dh];
    for y in 0..dh {
        let (y0, y1) = span(y, sh, dh);
        for x in 0..dw {
            let (x0, x1) = span(x, sw, dw);
            let any = (y0..y1).any(|sy| hole.get(sy * sw + x0..sy * sw + x1).is_some_and(|row| row.iter().any(|v| *v > 0.0)));
            out[y * dw + x] = f32::from(any);
        }
    }
    out
}

/// The source pixels `[start, end)` that destination pixel `d` of `dn` covers along an axis of `sn` pixels (at least one).
fn span(d: usize, sn: usize, dn: usize) -> (usize, usize) {
    let start = (d * sn / dn).min(sn.saturating_sub(1));
    let end = (((d + 1) * sn).div_ceil(dn)).clamp(start + 1, sn);
    (start, end)
}

fn average(src: &[f32], (sw, sh): (usize, usize), (dw, dh): (usize, usize), (x, y): (usize, usize), n: usize, out: &mut [f32]) {
    let ((x0, x1), (y0, y1)) = (span(x, sw, dw), span(y, sh, dh));
    let mut acc = vec![0f64; n];
    let mut count = 0f64;
    for sy in y0..y1 {
        for sx in x0..x1 {
            for (c, a) in acc.iter_mut().enumerate() {
                *a += f64::from(src.get((sy * sw + sx) * n + c).copied().unwrap_or(0.0));
            }
            count += 1.0;
        }
    }
    for (o, a) in out.iter_mut().zip(acc) {
        *o = (a / count.max(1.0)) as f32;
    }
}

fn bilinear(src: &[f32], (sw, sh): (usize, usize), (dw, dh): (usize, usize), (x, y): (usize, usize), n: usize, out: &mut [f32]) {
    let at = |p: f64, sn: usize, dn: usize| (((p + 0.5) * sn as f64 / dn as f64 - 0.5).clamp(0.0, sn.saturating_sub(1) as f64)) as f32;
    let (fx, fy) = (at(x as f64, sw, dw), at(y as f64, sh, dh));
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    for (c, o) in out.iter_mut().enumerate() {
        let v = |px: usize, py: usize| src.get((py * sw + px) * n + c).copied().unwrap_or(0.0);
        let top = v(x0, y0) + (v(x1, y0) - v(x0, y0)) * tx;
        let bottom = v(x0, y1) + (v(x1, y1) - v(x0, y1)) * tx;
        *o = top + (bottom - top) * ty;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_the_scale_and_stay_in_bounds() {
        assert_eq!(fill_size(100, 50, 2.0, 1536), (200, 100));
        assert_eq!(fill_size(100, 50, 0.5, 1536), (50, 25));
        assert_eq!(fill_size(4000, 2000, 1.0, 1536), (1536, 768));
        for k in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
            assert_eq!(fill_size(10, 10, k, 100), (10, 10), "{k}");
        }
        assert_eq!(fill_size(1, 1, 1e-9, 100), (1, 1));
    }

    #[test]
    fn scaling_a_flat_picture_keeps_it_flat_both_ways() {
        let flat = vec![0.25f32; 20 * 10 * 3];
        for (w, h) in [(7, 3), (40, 20), (20, 10)] {
            let s = scale(&flat, (20, 10), (w, h), 3);
            assert_eq!(s.len(), w * h * 3);
            assert!(s.iter().all(|v| (v - 0.25).abs() < 1e-6), "{w}×{h}");
        }
    }

    #[test]
    fn bad_buffers_scale_to_nothing() {
        assert!(scale(&[0.0; 5], (2, 2), (4, 4), 3).is_empty());
        assert!(scale(&[], (0, 0), (4, 4), 3).is_empty());
        assert!(scale_hole(&[0.0; 5], (2, 2), (4, 4)).is_empty());
    }

    #[test]
    fn a_hole_keeps_everything_that_touches_it() {
        let mut hole = vec![0f32; 8 * 8];
        hole[3 * 8 + 3] = 1.0;
        let small = scale_hole(&hole, (8, 8), (2, 2));
        assert_eq!(small.iter().sum::<f32>(), 1.0);
        let big = scale_hole(&hole, (8, 8), (16, 16));
        assert!(big[6 * 16 + 6] > 0.0 && big[7 * 16 + 7] > 0.0 && big[0] == 0.0);
    }
}
