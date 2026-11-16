//! The model's input (CPU, no model needed): the photo resized so its long side is about the processing size,
//! each side rounded to whole 14-pixel patches, area-averaged, and normalised with ImageNet's mean and standard
//! deviation, as Depth Anything 3's input processor does (`upper_bound_resize`).

use crate::{Error, Result};

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// The model's patch size.
pub const PATCH: usize = 14;
/// The long side the reference processes photos at.
pub const PROCESS_LONG: usize = 504;
/// The largest input side accepted (patches the backbone's attention can hold).
pub const MAX_SIDE: usize = 1512;

/// The input size for a `w × h` photo at long side `long`: aspect kept, each side the nearest multiple of
/// [`PATCH`] (at least one patch).
pub fn input_size(w: usize, h: usize, long: usize) -> Result<(usize, usize)> {
    if w == 0 || h == 0 || !(PATCH..=MAX_SIDE).contains(&long) {
        return Err(Error::Model(format!("no input size for a {w} × {h} photo at {long} pixels")));
    }
    let scale = long as f64 / w.max(h) as f64;
    let side = |v: usize| {
        let r = (v as f64 * scale).round().max(1.0);
        (((r / PATCH as f64).round() as usize).max(1) * PATCH).min(MAX_SIDE)
    };
    Ok((side(w), side(h)))
}

/// Separable area-resampling weights for `src → dst` samples (a box filter of the downscale factor's width, a
/// linear one when enlarging).
fn weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|o| {
            if scale >= 1.0 {
                let (a, b) = (o as f64 * scale, (o as f64 + 1.0) * scale);
                let lo = a.floor() as usize;
                let hi = (b.ceil() as usize).min(src).max(lo + 1);
                let w: Vec<f32> = (lo..hi).map(|i| ((b.min(i as f64 + 1.0) - a.max(i as f64)).max(0.0) / scale) as f32).collect();
                (lo, w)
            } else {
                let c = ((o as f64 + 0.5) * scale - 0.5).max(0.0);
                let lo = (c.floor() as usize).min(src.saturating_sub(1));
                let t = (c - lo as f64) as f32;
                if lo + 1 < src { (lo, vec![1.0 - t, t]) } else { (lo, vec![1.0]) }
            }
        })
        .collect()
}

/// An 8-bit sRGB image (`w × h`, row-major RGB) resized to `ow × oh`, normalised and laid out channel-first
/// (`[3, oh, ow]`).
pub fn preprocess(rgb: &[u8], w: usize, h: usize, (ow, oh): (usize, usize)) -> Result<Vec<f32>> {
    let n = w.checked_mul(h).and_then(|n| n.checked_mul(3)).ok_or_else(|| Error::Model("image too large".into()))?;
    if w == 0 || h == 0 || ow == 0 || oh == 0 || rgb.len() < n {
        return Err(Error::Model(format!("image buffer of {} bytes for {w} × {h}", rgb.len())));
    }
    let (wx, wy) = (weights(w, ow), weights(h, oh));
    // horizontal pass: h rows × ow columns × 3
    let mut tmp = vec![0f32; h * ow * 3];
    for y in 0..h {
        for (ox, (lo, ws)) in wx.iter().enumerate() {
            let mut acc = [0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let i = (y * w + lo + k) * 3;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += wt * f32::from(rgb.get(i + c).copied().unwrap_or(0));
                }
            }
            if let Some(dst) = tmp.get_mut((y * ow + ox) * 3..(y * ow + ox) * 3 + 3) {
                dst.copy_from_slice(&acc);
            }
        }
    }
    let plane = ow * oh;
    let mut out = vec![0f32; 3 * plane];
    for (oy, (lo, ws)) in wy.iter().enumerate() {
        for ox in 0..ow {
            let mut acc = [0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let i = ((lo + k) * ow + ox) * 3;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += wt * tmp.get(i + c).copied().unwrap_or(0.0);
                }
            }
            for (c, a) in acc.iter().enumerate() {
                if let Some(v) = out.get_mut(c * plane + oy * ow + ox) {
                    *v = (a / 255.0 - MEAN[c]) / STD[c];
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_keep_the_aspect_in_whole_patches() {
        assert_eq!(input_size(4032, 3024, 504).unwrap(), (504, 378));
        assert_eq!(input_size(3024, 4032, 504).unwrap(), (378, 504));
        assert_eq!(input_size(6000, 4000, 504).unwrap(), (504, 336));
        // a sliver still gets one patch
        assert_eq!(input_size(5000, 10, 504).unwrap(), (504, 14));
        assert!(input_size(0, 10, 504).is_err() && input_size(10, 10, 5000).is_err());
    }

    #[test]
    fn a_flat_grey_photo_is_flat_after_resizing() {
        let rgb = vec![128u8; 37 * 23 * 3];
        let x = preprocess(&rgb, 37, 23, (28, 14)).unwrap();
        let want = (128.0 / 255.0 - MEAN[0]) / STD[0];
        assert!(x[..28 * 14].iter().all(|v| (v - want).abs() < 1e-4));
        assert!(preprocess(&rgb[..10], 37, 23, (28, 14)).is_err());
    }
}
