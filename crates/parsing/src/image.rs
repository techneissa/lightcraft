//! Pictures in and out of the models: an 8-bit RGB photo, rotated and scaled crops of it, and bilinear sampling.

/// An upright 8-bit RGB photo, `width * height * 3` bytes, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl RgbImage {
    /// A photo, checked: its sizes must match its bytes.
    pub fn new(width: usize, height: usize, data: Vec<u8>) -> crate::Result<Self> {
        if width == 0 || height == 0 || width.checked_mul(height).and_then(|n| n.checked_mul(3)) != Some(data.len()) {
            return Err(crate::Error::Input(format!("a {width} × {height} picture needs {width}·{height}·3 bytes, not {}", data.len())));
        }
        Ok(Self { width, height, data })
    }

    /// The colour at a point in pixels (centres at +0.5), bilinear, 0 to 1 per channel; `None` outside the photo.
    pub fn sample(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 || x > self.width as f32 || y > self.height as f32 {
            return None;
        }
        let fx = (x - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = (y - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let px = |x: usize, y: usize, c: usize| f32::from(self.data.get((y * self.width + x) * 3 + c).copied().unwrap_or(0)) / 255.0;
        let mut out = [0.0; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let top = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
            let bottom = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
            *o = top * (1.0 - ty) + bottom * ty;
        }
        Some(out)
    }

    /// Luminance 0 to 1 at a pixel (clamped to the photo).
    pub fn luma(&self, x: usize, y: usize) -> f32 {
        let i = (y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 3;
        let c = |k: usize| f32::from(self.data.get(i + k).copied().unwrap_or(0)) / 255.0;
        0.299 * c(0) + 0.587 * c(1) + 0.114 * c(2)
    }
}

/// Bilinear value of a `w × h` plane at `(fx, fy)` in pixel-centre coordinates, clamped at the edges.
pub fn bilinear(data: &[f32], w: usize, h: usize, fx: f32, fy: f32) -> f32 {
    if w == 0 || h == 0 || !(fx.is_finite() && fy.is_finite()) {
        return 0.0;
    }
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fy = fy.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let v = |x: usize, y: usize| data.get(y * w + x).copied().unwrap_or(0.0);
    (v(x0, y0) * (1.0 - tx) + v(x1, y0) * tx) * (1.0 - ty) + (v(x0, y1) * (1.0 - tx) + v(x1, y1) * tx) * ty
}

/// A square crop of the photo, possibly turned: crop pixel `(u, v)` (0 to `side`) shows photo pixel
/// `centre + R(angle) · ((u, v) − side/2) · scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub cx: f32,
    pub cy: f32,
    /// Photo pixels per crop pixel.
    pub scale: f32,
    /// Radians, the turn that makes the face upright in the crop.
    pub angle: f32,
    pub side: usize,
}

impl Crop {
    /// The photo pixel a crop position shows.
    pub fn to_photo(&self, u: f32, v: f32) -> (f32, f32) {
        let h = self.side as f32 * 0.5;
        let (du, dv) = ((u - h) * self.scale, (v - h) * self.scale);
        let (s, c) = self.angle.sin_cos();
        (self.cx + c * du - s * dv, self.cy + s * du + c * dv)
    }

    /// The crop position that shows a photo pixel.
    pub fn to_crop(&self, x: f32, y: f32) -> (f32, f32) {
        let h = self.side as f32 * 0.5;
        let (dx, dy) = (x - self.cx, y - self.cy);
        let (s, c) = self.angle.sin_cos();
        let scale = self.scale.max(1e-6);
        ((c * dx + s * dy) / scale + h, (-s * dx + c * dy) / scale + h)
    }

    /// The photo-pixel box the crop covers.
    pub fn photo_bounds(&self) -> (f32, f32, f32, f32) {
        let s = self.side as f32;
        let corners = [self.to_photo(0.0, 0.0), self.to_photo(s, 0.0), self.to_photo(0.0, s), self.to_photo(s, s)];
        corners.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |(a, b, c, d), &(x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)))
    }
}

/// The colours of an `out_w × out_h` grid where `map(u, v)` gives the photo pixel each cell centre shows, as three
/// planes 0 to 1 (channel-major); cells outside the photo are `fill`. Shrinking more than 1.5 × averages a few samples
/// per cell so small faces in a big photo do not alias.
pub fn sample_grid(img: &RgbImage, out_w: usize, out_h: usize, step: f32, fill: [f32; 3], map: impl Fn(f32, f32) -> (f32, f32)) -> Vec<f32> {
    let n = out_w * out_h;
    let mut out = vec![0.0f32; n * 3];
    let taps = if step > 1.5 { (step.ceil() as usize).min(4) } else { 1 };
    let inv = 1.0 / taps as f32;
    for v in 0..out_h {
        for u in 0..out_w {
            let mut acc = [0.0f32; 3];
            for j in 0..taps {
                for i in 0..taps {
                    let (x, y) = map(u as f32 + (i as f32 + 0.5) * inv, v as f32 + (j as f32 + 0.5) * inv);
                    let c = img.sample(x, y).unwrap_or(fill);
                    for k in 0..3 {
                        acc[k] += c[k];
                    }
                }
            }
            let t = (taps * taps) as f32;
            let i = v * out_w + u;
            for (k, a) in acc.iter().enumerate() {
                if let Some(o) = out.get_mut(k * n + i) {
                    *o = a / t;
                }
            }
        }
    }
    out
}

/// ImageNet normalisation of channel-major planes in place.
pub fn imagenet(planes: &mut [f32], n: usize) {
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    for (k, chunk) in planes.chunks_mut(n.max(1)).enumerate().take(3) {
        let (m, s) = (MEAN.get(k).copied().unwrap_or(0.0), STD.get(k).copied().unwrap_or(1.0));
        for v in chunk {
            *v = (*v - m) / s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_maps_there_and_back() {
        let c = Crop { cx: 100.0, cy: 50.0, scale: 0.7, angle: 0.4, side: 256 };
        let (x, y) = c.to_photo(30.0, 200.0);
        let (u, v) = c.to_crop(x, y);
        assert!((u - 30.0).abs() < 1e-3 && (v - 200.0).abs() < 1e-3);
        assert_eq!(c.to_photo(128.0, 128.0), (100.0, 50.0));
    }

    #[test]
    fn sizes_must_match_the_bytes() {
        assert!(RgbImage::new(2, 2, vec![0; 12]).is_ok());
        assert!(RgbImage::new(2, 2, vec![0; 11]).is_err());
        assert!(RgbImage::new(0, 2, vec![]).is_err());
        assert!(RgbImage::new(usize::MAX, 2, vec![]).is_err());
        let one = RgbImage::new(1, 1, vec![255, 0, 0]).unwrap();
        assert_eq!(one.sample(0.5, 0.5), Some([1.0, 0.0, 0.0]));
        assert_eq!(one.sample(2.0, 0.5), None);
        assert!(bilinear(&[], 0, 0, 0.0, 0.0) == 0.0);
    }
}
