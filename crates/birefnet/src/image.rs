//! Image preprocessing (CPU, no model needed): the photo stretched to the model's square input and
//! normalised the way BiRefNet's reference does (Pillow-style antialiased bilinear resize, then
//! ImageNet mean and standard deviation).

use crate::{Error, Result};

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Separable resampling weights for `src → dst` samples with an antialiased triangle filter
/// (Pillow's BILINEAR: the support widens with the downscale factor).
fn weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f64 / dst as f64;
    let ss = scale.max(1.0);
    (0..dst)
        .map(|o| {
            let center = (o as f64 + 0.5) * scale;
            let lo = ((center - ss + 0.5).floor().max(0.0)) as usize;
            let hi = ((center + ss + 0.5).floor() as usize).min(src);
            let mut w: Vec<f32> = (lo..hi).map(|i| (1.0 - ((i as f64 - center + 0.5) / ss).abs()).max(0.0) as f32).collect();
            let sum: f32 = w.iter().sum();
            if sum > 0.0 {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            (lo, w)
        })
        .collect()
}

/// An 8-bit RGB image (`w × h`, row-major) stretched to `side × side`, normalised and laid out
/// channel-first (`[3, side, side]`).
pub fn preprocess(rgb: &[u8], w: usize, h: usize, side: usize) -> Result<Vec<f32>> {
    let n = w.checked_mul(h).and_then(|n| n.checked_mul(3)).ok_or_else(|| Error::Model("image too large".into()))?;
    if w == 0 || h == 0 || side == 0 || rgb.len() < n {
        return Err(Error::Model(format!("image buffer of {} bytes for {w} × {h}", rgb.len())));
    }
    let (wx, wy) = (weights(w, side), weights(h, side));
    // horizontal pass: h rows × side columns × 3
    let mut tmp = vec![0f32; h * side * 3];
    for y in 0..h {
        for (ox, (lo, ws)) in wx.iter().enumerate() {
            let mut acc = [0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let i = (y * w + lo + k) * 3;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += wt * f32::from(rgb.get(i + c).copied().unwrap_or(0));
                }
            }
            if let Some(dst) = tmp.get_mut((y * side + ox) * 3..(y * side + ox) * 3 + 3) {
                dst.copy_from_slice(&acc);
            }
        }
    }
    let mut out = vec![0f32; 3 * side * side];
    for (oy, (lo, ws)) in wy.iter().enumerate() {
        for ox in 0..side {
            let mut acc = [0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let i = ((lo + k) * side + ox) * 3;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += wt * tmp.get(i + c).copied().unwrap_or(0.0);
                }
            }
            for (c, a) in acc.iter().enumerate() {
                // Pillow rounds to 8 bits; then /255 and (x − mean) / std
                let v = a.round().clamp(0.0, 255.0) / 255.0;
                if let Some(d) = out.get_mut((c * side + oy) * side + ox) {
                    *d = (v - MEAN[c]) / STD[c];
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
    fn a_flat_white_image_normalises_to_the_imagenet_white() {
        let flat = vec![255u8; 30 * 20 * 3];
        let p = preprocess(&flat, 30, 20, 7).unwrap();
        assert_eq!(p.len(), 3 * 49);
        for (c, plane) in p.chunks(49).enumerate() {
            let want = (1.0 - MEAN[c]) / STD[c];
            assert!(plane.iter().all(|v| (v - want).abs() < 1e-5), "channel {c}");
        }
    }

    #[test]
    fn short_or_empty_buffers_are_errors() {
        assert!(preprocess(&[0; 5], 2, 2, 4).is_err());
        assert!(preprocess(&[], 0, 0, 4).is_err());
        assert!(preprocess(&[0; 12], 2, 2, 0).is_err());
        assert!(preprocess(&[0; 12], usize::MAX, 2, 4).is_err());
    }
}
