//! [`close_seams`]: thin dips inside a selection closed before its edge is snapped.

use crate::Alpha;

/// Close every dip in `alpha` narrower than `2 · radius + 1` pixels (a morphological closing: the largest coverage
/// around each pixel, then the smallest of those): a seam a people parse leaves where two of its crops meet, which
/// snapping to the photo would otherwise turn into a crisp cut along whatever dark line the photo has there (a
/// zipper). Gaps wider than that, and the outside, are left as they are. Sizes that don't add up change nothing.
pub fn close_seams(alpha: &mut Alpha, radius: usize) {
    if !alpha.is_valid() || radius == 0 {
        return;
    }
    let (w, h) = (alpha.w, alpha.h);
    let grown = filter(&alpha.data, w, h, radius, f32::max);
    let closed = filter(&grown, w, h, radius, f32::min);
    for (a, c) in alpha.data.iter_mut().zip(&closed) {
        *a = a.max(*c);
    }
}

/// `pick` (max or min) over the square of `radius` around each pixel, the square clipped to the picture.
fn filter(v: &[f32], w: usize, h: usize, radius: usize, pick: fn(f32, f32) -> f32) -> Vec<f32> {
    let mut rows = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(radius), (x + radius + 1).min(w));
            rows[y * w + x] = v.get(y * w + a..y * w + b).map_or(0.0, |s| s.iter().copied().reduce(pick).unwrap_or(0.0));
        }
    }
    let mut out = vec![0.0f32; w * h];
    for x in 0..w {
        for y in 0..h {
            let (a, b) = (y.saturating_sub(radius), (y + radius + 1).min(h));
            out[y * w + x] = (a..b).map(|k| rows.get(k * w + x).copied().unwrap_or(0.0)).reduce(pick).unwrap_or(0.0);
        }
    }
    out
}
