//! Whites and Iris: retouching inside the eye's opening, on the Eye Whites and Iris masks.
//!
//! Both work in the eye's opening only: the parser's masks are coarse (a few pixels of theirs per eye), so where the
//! face has the 478 face mesh points each change is also cut to the opening between the lids ([`opening`]), and the
//! lids, lashes and skin never change. Every size is a fraction of the eye's width or the iris's radius, and every
//! reference level comes from a fixed grid over the whole eye, so a draft, the loupe and an export agree.

mod iris;
mod opening;
mod whites;

use lightcraft_raster::Image;

pub use iris::iris;
pub use whites::whites;

use super::grid::blur;

/// How the sliders' 0…1 turns into strength: a little more than linear at the low end, so 25 and 50 show.
fn response(amount: f32) -> f32 {
    amount.clamp(0.0, 1.0).powf(0.8)
}

/// A pixel box `[x0, y0, x1, y1)` of the image, with its values copied out.
struct Patch {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

impl Patch {
    /// Box `b` grown by `pad` pixels, clamped to a `width × height` image; `None` when empty.
    fn grown(b: [usize; 4], pad: usize, width: usize, height: usize) -> Option<Patch> {
        let (x0, y0) = (b[0].saturating_sub(pad), b[1].saturating_sub(pad));
        let (x1, y1) = (b[2].saturating_add(pad).min(width), b[3].saturating_add(pad).min(height));
        (x1 > x0 && y1 > y0).then_some(Patch { x0, y0, w: x1 - x0, h: y1 - y0 })
    }

    /// The image index of patch cell `j`.
    fn image_index(&self, j: usize, width: usize) -> usize {
        (self.y0 + j / self.w.max(1)) * width + self.x0 + j % self.w.max(1)
    }

    /// The pixel centre of patch cell `j`.
    fn centre(&self, j: usize) -> (f64, f64) {
        ((self.x0 + j % self.w.max(1)) as f64 + 0.5, (self.y0 + j / self.w.max(1)) as f64 + 0.5)
    }
}

/// `values` blurred with a Gaussian of `sigma` pixels counting each by its `weight` (a normalised convolution): what
/// lies outside the weights (lashes, the pupil, a catchlight) does not leak into it. Where no weight reaches, the
/// value itself.
fn weighted_blur(values: &[f32], weights: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    let num = Image::<f32>::from_fn(w, h, |x, y| {
        let i = y * w + x;
        values.get(i).copied().unwrap_or(0.0) * weights.get(i).copied().unwrap_or(0.0)
    });
    let den = Image::<f32>::from_fn(w, h, |x, y| weights.get(y * w + x).copied().unwrap_or(0.0));
    let (num, den) = (blur(&num, sigma), blur(&den, sigma));
    num.data.iter().zip(&den.data).zip(values).map(|((n, d), v)| if *d > 1e-4 && (n / d).is_finite() { n / d } else { *v }).collect()
}

/// The `q` quantile (0…1) of `v` (reordered); `None` when empty.
fn quantile(v: &mut [f32], q: f32) -> Option<f32> {
    let last = v.len().checked_sub(1)?;
    let k = ((last as f32 * q.clamp(0.0, 1.0)) as usize).min(last);
    let (_, x, _) = v.select_nth_unstable_by(k, f32::total_cmp);
    Some(*x)
}
