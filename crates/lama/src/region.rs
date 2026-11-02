//! Filling a hole in a picture of any size: LaMa works on about 512 pixels, so the fill is made on a
//! crop around the hole (the hole plus some of its surroundings), scaled down to at most
//! [`Params::work`] pixels, and only the hole's own pixels take the result: everything outside it is the
//! original, byte for byte. The result is scaled back up into the hole, so it is a little softer than the
//! rest of the picture when the crop had to shrink.
//!
//! Pictures are linear light (three f32 per pixel). The model sees gamma-encoded values in `0..1`, so the
//! crop is first scaled to a middle-grey average (undone afterwards): a dark raw or an HDR picture works too.

use crate::{Error, Lama, Result};

/// How a hole is filled.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Longest side of the crop the model sees, pixels.
    pub work: usize,
    /// How much of the hole's surroundings the crop takes in, as a fraction of the hole's longer side.
    pub context: f32,
    /// How far the hole grows at working size, pixels, so the model also rebuilds the edge of what is removed.
    pub grow: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self { work: 512, context: 0.5, grow: 2 }
    }
}

/// Mean luminance the crop is scaled to before encoding.
const MID_GREY: f32 = 0.18;
/// Largest picture side accepted.
const MAX_SIDE: usize = 1 << 15;
/// Most pixels a picture may have: the picture is copied and the crop cut from it, so a larger one is refused with a
/// message instead of taking gigabytes (the app hands over crops of at most about 2.4 million).
pub const MAX_PIXELS: usize = 1 << 24;

fn encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// `rgb` (linear, `w·h` pixels) with the pixels where `hole > 0` rebuilt by `run`, which is given the
/// gamma-encoded crop, its hole (1 where to fill) and the crop's size, and returns the filled crop.
pub fn fill(
    rgb: &[f32],
    hole: &[f32],
    w: usize,
    h: usize,
    params: &Params,
    run: &dyn Fn(&[f32], &[f32], usize, usize) -> Result<Vec<f32>>,
) -> Result<Vec<f32>> {
    let n = w
        .checked_mul(h)
        .filter(|n| *n > 0 && w <= MAX_SIDE && h <= MAX_SIDE)
        .ok_or_else(|| Error::Input(format!("cannot fill a {w}×{h} picture")))?;
    if n > MAX_PIXELS {
        return Err(Error::Input(format!("a {w}×{h} picture is too large to fill (at most {} million pixels)", MAX_PIXELS >> 20)));
    }
    if rgb.len() != n * 3 || hole.len() != n {
        return Err(Error::Input(format!("buffers hold {} and {} values for {w}×{h} pixels", rgb.len(), hole.len())));
    }
    if rgb.iter().chain(hole).any(|v| !v.is_finite()) {
        return Err(Error::Input("the picture or hole has a value that is not a number".into()));
    }
    let mut out = rgb.to_vec();
    // the hole's bounding box; nothing to fill is not an error
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if hole[y * w + x] > 0.0 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Ok(out);
    }
    // the crop: the hole and its surroundings, at least a few pixels past it
    let side = (x1 - x0).max(y1 - y0);
    let margin = ((side as f32 * params.context.clamp(0.0, 4.0)).ceil() as usize).max(8);
    let (cx0, cy0, cx1, cy1) = (x0.saturating_sub(margin), y0.saturating_sub(margin), (x1 + margin).min(w), (y1 + margin).min(h));
    let (cw, ch) = (cx1 - cx0, cy1 - cy0);
    // working size
    let work = params.work.clamp(32, crate::MAX_SIDE);
    let scale = (work as f32 / cw.max(ch) as f32).min(1.0);
    let (ww, wh) = (((cw as f32 * scale).round() as usize).max(1), ((ch as f32 * scale).round() as usize).max(1));
    let at = |x: usize, y: usize| (cy0 + y) * w + cx0 + x;
    // exposure: the mean luminance of the crop's pixels outside the hole
    let (mut sum, mut count) = (0f64, 0usize);
    for y in 0..ch {
        for x in 0..cw {
            let i = at(x, y);
            if hole[i] <= 0.0 {
                let l = 0.2126 * rgb[i * 3] + 0.7152 * rgb[i * 3 + 1] + 0.0722 * rgb[i * 3 + 2];
                sum += l.max(0.0) as f64;
                count += 1;
            }
        }
    }
    let mean = if count > 0 { (sum / count as f64) as f32 } else { MID_GREY };
    let gain = if mean > 1e-6 { (MID_GREY / mean).clamp(0.02, 50.0) } else { 1.0 };
    // scale the crop to working size: encoded colour (area average) and the hole (any coverage counts)
    let mut small = vec![0f32; ww * wh * 3];
    let mut small_hole = vec![0f32; ww * wh];
    for wy in 0..wh {
        let (sy0, sy1) = (wy * ch / wh, ((wy + 1) * ch).div_ceil(wh).clamp(wy * ch / wh + 1, ch));
        for wx in 0..ww {
            let (sx0, sx1) = (wx * cw / ww, ((wx + 1) * cw).div_ceil(ww).clamp(wx * cw / ww + 1, cw));
            let (mut acc, mut cnt, mut any) = ([0f32; 3], 0f32, false);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = at(sx, sy);
                    any |= hole[i] > 0.0;
                    for c in 0..3 {
                        acc[c] += encode(rgb[i * 3 + c] * gain);
                    }
                    cnt += 1.0;
                }
            }
            for c in 0..3 {
                small[(wy * ww + wx) * 3 + c] = acc[c] / cnt.max(1.0);
            }
            small_hole[wy * ww + wx] = f32::from(any);
        }
    }
    let small_hole = grow(&small_hole, ww, wh, params.grow);
    let filled = run(&small, &small_hole, ww, wh)?;
    if filled.len() != ww * wh * 3 {
        return Err(Error::Compute("the model returned a picture of the wrong size".into()));
    }
    // back to full size, into the hole only (bilinear between the model's pixel centres)
    for y in 0..ch {
        for x in 0..cw {
            let i = at(x, y);
            if hole[i] <= 0.0 {
                continue;
            }
            let (fx, fy) = (
                ((x as f32 + 0.5) * ww as f32 / cw as f32 - 0.5).clamp(0.0, ww as f32 - 1.0),
                ((y as f32 + 0.5) * wh as f32 / ch as f32 - 0.5).clamp(0.0, wh as f32 - 1.0),
            );
            let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
            let (tx, ty) = (fx - ix as f32, fy - iy as f32);
            let (jx, jy) = ((ix + 1).min(ww - 1), (iy + 1).min(wh - 1));
            for c in 0..3 {
                let p = |px: usize, py: usize| filled.get((py * ww + px) * 3 + c).copied().unwrap_or(0.0);
                let v = (p(ix, iy) * (1.0 - tx) + p(jx, iy) * tx) * (1.0 - ty) + (p(ix, jy) * (1.0 - tx) + p(jx, jy) * tx) * ty;
                if let Some(o) = out.get_mut(i * 3 + c) {
                    *o = decode(v) / gain;
                }
            }
        }
    }
    Ok(out)
}

/// `mask` (0 or 1) grown by `r` pixels (a square).
fn grow(mask: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return mask.to_vec();
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] > 0.0 {
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        out[yy * w + xx] = 1.0;
                    }
                }
            }
        }
    }
    out
}

impl Lama {
    /// Fills where `hole > 0` in a linear-light picture of any size (see [`fill`] and [`Params`]).
    pub fn fill_region(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, params: &Params, cancel: &dyn Fn() -> bool) -> Result<Vec<f32>> {
        fill(rgb, hole, w, h, params, &|img, m, ww, wh| self.inpaint(img, m, ww, wh, cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_over_the_pixel_limit_is_refused_with_a_message() {
        let side = 1 << 13;
        let e = fill(&[], &[], side, side, &Params::default(), &|_, _, _, _| Ok(Vec::new())).unwrap_err();
        assert!(matches!(&e, Error::Input(m) if m.contains("too large")), "{e:?}");
    }

    /// A "model" that paints the hole one flat colour.
    fn paint(colour: [f32; 3]) -> impl Fn(&[f32], &[f32], usize, usize) -> Result<Vec<f32>> {
        move |img: &[f32], m: &[f32], _w: usize, _h: usize| {
            let mut o = img.to_vec();
            for (i, v) in m.iter().enumerate() {
                if *v > 0.0 {
                    o[i * 3..i * 3 + 3].copy_from_slice(&colour);
                }
            }
            Ok(o)
        }
    }

    fn picture(w: usize, h: usize) -> Vec<f32> {
        (0..w * h * 3).map(|i| 0.1 + (i % 17) as f32 * 0.02).collect()
    }

    #[test]
    fn only_the_holes_pixels_change_and_the_rest_is_exact() {
        let (w, h) = (300, 200);
        // a flat grey of the exposure the model is given (so the gain is 1)
        let img = vec![0.18f32; w * h * 3];
        let mut hole = vec![0f32; w * h];
        for y in 80..120 {
            for x in 100..160 {
                hole[y * w + x] = 1.0;
            }
        }
        let out = fill(&img, &hole, w, h, &Params::default(), &paint([0.9, 0.5, 0.2])).unwrap();
        for i in 0..w * h {
            if hole[i] > 0.0 {
                assert_ne!(&out[i * 3..i * 3 + 3], &img[i * 3..i * 3 + 3]);
            } else {
                assert_eq!(&out[i * 3..i * 3 + 3], &img[i * 3..i * 3 + 3]);
            }
        }
        // the model's flat colour comes back as the same linear colour (up to the encode and decode)
        let i = (100 * w + 130) * 3;
        for (got, want) in out[i..i + 3].iter().zip([0.9, 0.5, 0.2]) {
            assert!((got - decode(want)).abs() < 0.01, "{got} vs {}", decode(want));
        }
    }

    #[test]
    fn the_crop_is_scaled_down_to_the_working_size_and_the_hole_up_again() {
        let (w, h) = (2000, 1500);
        let img = picture(w, h);
        let mut hole = vec![0f32; w * h];
        for y in 500..1100 {
            for x in 700..1400 {
                hole[y * w + x] = 1.0;
            }
        }
        let seen = std::cell::Cell::new((0, 0));
        let run = |im: &[f32], m: &[f32], ww: usize, wh: usize| {
            seen.set((ww, wh));
            paint([0.5; 3])(im, m, ww, wh)
        };
        let out = fill(&img, &hole, w, h, &Params::default(), &run).unwrap();
        let (ww, wh) = seen.get();
        assert!(ww.max(wh) <= 512 && ww.max(wh) >= 500, "{ww}x{wh}");
        assert!(out[(800 * w + 1000) * 3] != img[(800 * w + 1000) * 3]);
    }

    #[test]
    fn no_hole_is_a_no_op_and_a_single_pixel_hole_works() {
        let (w, h) = (64, 48);
        let img = picture(w, h);
        assert_eq!(fill(&img, &vec![0.0; w * h], w, h, &Params::default(), &paint([1.0; 3])).unwrap(), img);
        let mut hole = vec![0f32; w * h];
        hole[20 * w + 30] = 1.0;
        let out = fill(&img, &hole, w, h, &Params::default(), &paint([0.7; 3])).unwrap();
        assert_ne!(out[(20 * w + 30) * 3], img[(20 * w + 30) * 3]);
        assert_eq!(out[(20 * w + 31) * 3], img[(20 * w + 31) * 3]);
    }

    #[test]
    fn hostile_input_is_an_error() {
        let ok = |r: Result<Vec<f32>>| matches!(r, Err(Error::Input(_)));
        let p = Params::default();
        let run = paint([0.5; 3]);
        assert!(ok(fill(&[], &[], 0, 0, &p, &run)));
        assert!(ok(fill(&[0.0; 3], &[1.0], 2, 1, &p, &run)));
        assert!(ok(fill(&[0.0; 6], &[1.0], 2, 1, &p, &run)));
        assert!(ok(fill(&[f32::NAN; 3], &[1.0], 1, 1, &p, &run)));
        assert!(ok(fill(&[0.0; 3], &[f32::INFINITY], 1, 1, &p, &run)));
        assert!(ok(fill(&[], &[], usize::MAX, 2, &p, &run)));
        // a model that returns the wrong amount is an error too
        let bad = |_: &[f32], _: &[f32], _: usize, _: usize| Ok(vec![0.0; 3]);
        assert!(fill(&picture(16, 16), &vec![1.0; 256], 16, 16, &p, &bad).is_err());
    }
}
