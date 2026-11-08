//! A part's mask at any size: the planes sampled over a `width × height` frame of the photo, then (with a guide
//! picture) refined along the photo's own edges by a fast guided filter (He and Sun, 2015) whose radius is a fraction
//! of the frame, so a small preview and a 24 MP export draw the same mask.

use crate::image::RgbImage;
use crate::types::{NormRect, Parsing, Part, Plane};

/// The guided filter's radius is this many of a mask's own cells (the size one model pixel covers in the frame): it
/// reaches across the blur of upsampling and no further, so faces parsed up close keep their detail.
pub const CELLS: f32 = 1.5;
/// Guided filter regularisation (on luminance 0 to 1): edges weaker than about √EPS are smoothed over.
pub const EPS: f32 = 2e-4;
/// The radius never exceeds this fraction of the frame's long side.
const MAX_RADIUS: f32 = 0.01;
/// Largest frame rendered (a 100 MP photo).
const MAX_PIXELS: usize = 120_000_000;

/// Which mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// One part of one person (index into [`Parsing::people`]).
    Person(usize, Part),
    /// One part of everybody.
    Everyone(Part),
    /// Everything that is not a person.
    Background,
}

/// The planes a selection draws from.
fn planes(p: &Parsing, sel: Select) -> Vec<&Plane> {
    let part = match sel {
        Select::Person(_, part) | Select::Everyone(part) => part,
        Select::Background => Part::EntirePerson,
    };
    let people: Vec<_> = match sel {
        Select::Person(k, _) => p.people.get(k).into_iter().collect(),
        _ => p.people.iter().collect(),
    };
    people.into_iter().filter_map(|q| q.parts.get(&part)).flatten().collect()
}

/// The mask, `width × height` values 0 to 1 over the whole photo (bilinear, no edge refinement).
pub fn mask(p: &Parsing, sel: Select, width: usize, height: usize) -> Vec<f32> {
    let Some(n) = width.checked_mul(height).filter(|&n| n > 0 && n <= MAX_PIXELS) else { return Vec::new() };
    let mut out = vec![0.0f32; n];
    for plane in planes(p, sel) {
        draw_max(&mut out, width, height, plane);
    }
    if sel == Select::Background {
        for v in &mut out {
            *v = 1.0 - *v;
        }
    }
    out
}

/// Draw `plane` into a frame, keeping the larger value per pixel.
fn draw_max(out: &mut [f32], width: usize, height: usize, plane: &Plane) {
    let Some(r) = plane.rect.clamped() else { return };
    let span = |a: f32, b: f32, n: usize| ((a * n as f32).floor().max(0.0) as usize, ((b * n as f32).ceil() as usize).min(n));
    let ((x0, x1), (y0, y1)) = (span(r.x0, r.x1, width), span(r.y0, r.y1, height));
    for y in y0..y1 {
        let fy = (y as f32 + 0.5) / height as f32;
        for x in x0..x1 {
            let v = plane.sample((x as f32 + 0.5) / width as f32, fy);
            if let Some(o) = out.get_mut(y * width + x) {
                *o = o.max(v);
            }
        }
    }
}

/// The mask refined along the edges of `guide` (the photo at the frame's size).
pub fn refined(p: &Parsing, sel: Select, guide: &RgbImage) -> Vec<f32> {
    refined_with(p, sel, guide, CELLS, EPS)
}

/// [`refined`] with another radius (in the mask's cells; 0: no refinement) and regularisation.
pub fn refined_with(p: &Parsing, sel: Select, guide: &RgbImage, cells: f32, eps: f32) -> Vec<f32> {
    let m = mask(p, sel, guide.width, guide.height);
    let r = radius_px(p, sel, guide.width, guide.height, cells);
    if m.is_empty() || r < 1.0 {
        return m;
    }
    guided_px(guide, &m, r.round() as usize, eps)
}

/// The filter radius in frame pixels: `cells` of the coarsest plane the selection draws from.
pub fn radius_px(p: &Parsing, sel: Select, width: usize, height: usize, cells: f32) -> f32 {
    let coarsest = planes(p, sel).iter().fold(0.0f32, |a, q| {
        a.max(q.rect.width() * width as f32 / q.width.max(1) as f32).max(q.rect.height() * height as f32 / q.height.max(1) as f32)
    });
    (coarsest * cells).min(MAX_RADIUS * width.max(height) as f32)
}

/// Box sums over a `w × h` plane with radius `r` (the window clipped at the edges), as means.
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut integral = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += f64::from(src.get(y * w + x).copied().unwrap_or(0.0));
            let above = integral.get(y * (w + 1) + x + 1).copied().unwrap_or(0.0);
            if let Some(s) = integral.get_mut((y + 1) * (w + 1) + x + 1) {
                *s = above + row;
            }
        }
    }
    let at = |x: usize, y: usize| integral.get(y * (w + 1) + x).copied().unwrap_or(0.0);
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (ya, yb) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (xa, xb) = (x.saturating_sub(r), (x + r + 1).min(w));
            let area = ((xb - xa) * (yb - ya)).max(1) as f64;
            if let Some(o) = out.get_mut(y * w + x) {
                *o = ((at(xb, yb) - at(xa, yb) - at(xb, ya) + at(xa, ya)) / area) as f32;
            }
        }
    }
    out
}

/// Average `s × s` blocks of a plane.
fn shrink(src: &[f32], w: usize, h: usize, s: usize) -> (Vec<f32>, usize, usize) {
    let (sw, sh) = (w.div_ceil(s), h.div_ceil(s));
    let mut out = vec![0.0f32; sw * sh];
    let mut count = vec![0u32; sw * sh];
    for y in 0..h {
        for x in 0..w {
            let j = (y / s) * sw + x / s;
            if let (Some(o), Some(c)) = (out.get_mut(j), count.get_mut(j)) {
                *o += src.get(y * w + x).copied().unwrap_or(0.0);
                *c += 1;
            }
        }
    }
    for (o, c) in out.iter_mut().zip(&count) {
        *o /= (*c).max(1) as f32;
    }
    (out, sw, sh)
}

/// Fast guided filter of `mask` (same size as `guide`) with the guide's luminance; `radius` as a fraction of the long
/// side. Values stay 0 to 1.
pub fn guided(guide: &RgbImage, mask: &[f32], radius: f32, eps: f32) -> Vec<f32> {
    guided_px(guide, mask, (radius * guide.width.max(guide.height) as f32).round().max(1.0) as usize, eps)
}

/// [`guided`] with the radius in pixels.
pub fn guided_px(guide: &RgbImage, mask: &[f32], r_full: usize, eps: f32) -> Vec<f32> {
    let (w, h) = (guide.width, guide.height);
    if mask.len() != w * h || w == 0 || h == 0 {
        return mask.to_vec();
    }
    let r_full = r_full.max(1);
    // work at a smaller size when the radius is large: the coefficients are smooth, the detail comes from the guide
    let s = (r_full / 4).max(1);
    let r = (r_full / s).max(1);
    let luma: Vec<f32> = (0..w * h).map(|i| guide.luma(i % w, i / w)).collect();
    let (i_s, sw, sh) = shrink(&luma, w, h, s);
    let (p_s, _, _) = shrink(mask, w, h, s);
    let mean_i = box_mean(&i_s, sw, sh, r);
    let mean_p = box_mean(&p_s, sw, sh, r);
    let ii: Vec<f32> = i_s.iter().map(|v| v * v).collect();
    let ip: Vec<f32> = i_s.iter().zip(&p_s).map(|(a, b)| a * b).collect();
    let corr_i = box_mean(&ii, sw, sh, r);
    let corr_ip = box_mean(&ip, sw, sh, r);
    let mut a = vec![0.0f32; sw * sh];
    let mut b = vec![0.0f32; sw * sh];
    for k in 0..sw * sh {
        let g = |v: &Vec<f32>| v.get(k).copied().unwrap_or(0.0);
        let var = g(&corr_i) - g(&mean_i) * g(&mean_i);
        let cov = g(&corr_ip) - g(&mean_i) * g(&mean_p);
        let ak = cov / (var + eps);
        if let (Some(x), Some(y)) = (a.get_mut(k), b.get_mut(k)) {
            *x = ak;
            *y = g(&mean_p) - ak * g(&mean_i);
        }
    }
    let ma = box_mean(&a, sw, sh, r);
    let mb = box_mean(&b, sw, sh, r);
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let fy = (y as f32 + 0.5) / s as f32 - 0.5;
        for x in 0..w {
            let fx = (x as f32 + 0.5) / s as f32 - 0.5;
            let i = y * w + x;
            let q = crate::image::bilinear(&ma, sw, sh, fx, fy) * luma.get(i).copied().unwrap_or(0.0) + crate::image::bilinear(&mb, sw, sh, fx, fy);
            if let Some(o) = out.get_mut(i) {
                *o = if q.is_finite() { q.clamp(0.0, 1.0) } else { 0.0 };
            }
        }
    }
    out
}

/// Write a `w × h` mask as an 8-bit grey PNG.
pub fn save_png(path: &std::path::Path, mask: &[f32], w: usize, h: usize) -> crate::Result<()> {
    let bytes: Vec<u8> = mask.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let (wu, hu) = (
        u32::try_from(w).map_err(|_| crate::Error::Input("too wide".into()))?,
        u32::try_from(h).map_err(|_| crate::Error::Input("too tall".into()))?,
    );
    let png = image::GrayImage::from_raw(wu, hu, bytes).ok_or_else(|| crate::Error::Input("the mask's size does not match its values".into()))?;
    png.save(path).map_err(|e| crate::Error::Input(format!("could not write {}: {e}", path.display())))
}

/// The box around a selection's planes, in fractions of the photo.
pub fn bounds(p: &Parsing, sel: Select) -> Option<NormRect> {
    if sel == Select::Background {
        return Some(NormRect::new(0.0, 0.0, 1.0, 1.0));
    }
    planes(p, sel).iter().map(|q| q.rect).reduce(|a, b| a.union(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Person;

    fn one_square() -> Parsing {
        let mut person = Person::default();
        let plane = Plane { rect: NormRect::new(0.25, 0.25, 0.75, 0.75), width: 4, height: 4, data: vec![1.0; 16] };
        person.parts.insert(Part::EntirePerson, vec![plane]);
        Parsing { people: vec![person], width: 100, height: 100, ..Parsing::default() }
    }

    #[test]
    fn a_mask_is_the_same_shape_at_any_size() {
        let p = one_square();
        for size in [8usize, 64, 333] {
            let m = mask(&p, Select::Everyone(Part::EntirePerson), size, size);
            let covered = m.iter().filter(|&&v| v > 0.5).count() as f32 / (size * size) as f32;
            assert!((covered - 0.25).abs() < 0.06, "{size}: {covered}");
            let bg = mask(&p, Select::Background, size, size);
            assert!((bg[0] - 1.0).abs() < 1e-6);
        }
        assert!(mask(&p, Select::Person(3, Part::Hair), 10, 10).iter().all(|&v| v == 0.0));
        assert!(mask(&p, Select::Background, 0, 10).is_empty());
    }

    #[test]
    fn the_guided_filter_snaps_a_soft_edge_to_the_photo_edge() {
        let (w, h) = (64usize, 32usize);
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 32..w {
                data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let guide = RgbImage::new(w, h, data).unwrap();
        // a mask whose edge is soft and a few pixels off the photo's edge
        let mask: Vec<f32> = (0..w * h).map(|i| (((i % w) as f32 - 28.0) / 8.0).clamp(0.0, 1.0)).collect();
        let out = guided(&guide, &mask, 0.06, 1e-4);
        let row = &out[16 * w..17 * w];
        assert!(row[20] < 0.2 && row[44] > 0.8, "{row:?}");
        assert!(row[30] < row[34], "{row:?}");
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
        assert_eq!(guided(&guide, &mask[..10], 0.1, 1e-3).len(), 10);
    }
}
