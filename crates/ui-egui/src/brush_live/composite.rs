//! The adjustment under a live brush stroke: the photo as it is drawn now (`base`) blended toward
//! the photo as it would look with the mask covering everything (`target`) by the stroke's
//! coverage, one rectangle of pixels at a time.
//!
//! Where the coverage is 1 the result is the `target` render exactly, and where it is 0 the
//! `base` one, so a settled stroke (the committed render replacing this blend) shows no jump
//! inside the core of the brush; the feathered edge differs only by the rounding of mixing
//! the two finished pictures instead of the adjustment itself.

use egui::{Color32, ColorImage, Rect};
use std::sync::Arc;

/// A rectangle of image pixels: `x0..x1` by `y0..y1`.
pub type PxRect = (usize, usize, usize, usize);

/// `a` mixed toward `b` by `t` (0..1), per channel, opaque.
#[inline]
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    // (also a coverage that is not a number)
    if t.is_nan() || t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), 255)
}

/// The blended picture, kept between frames so each one recomputes only the pixels that changed.
pub struct Blend {
    base: Arc<ColorImage>,
    target: Arc<ColorImage>,
    out: ColorImage,
}

impl Blend {
    /// `None` unless both pictures are the same, non-empty size.
    pub fn new(base: Arc<ColorImage>, target: Arc<ColorImage>) -> Option<Blend> {
        if base.size != target.size || base.size[0] == 0 || base.size[1] == 0 || base.pixels.len() != target.pixels.len() {
            return None;
        }
        let out = ColorImage { size: base.size, source_size: base.source_size, pixels: base.pixels.clone() };
        Some(Blend { base, target, out })
    }

    pub fn size(&self) -> [usize; 2] {
        self.out.size
    }

    pub fn image(&self) -> &ColorImage {
        &self.out
    }

    /// Recompute the pixels of `rect` (clipped to the picture) from `coverage(x, y)` and return
    /// the rectangle actually computed.
    pub fn update(&mut self, rect: PxRect, coverage: impl Fn(usize, usize) -> f32) -> Option<PxRect> {
        let [w, h] = self.out.size;
        let (x0, y0, x1, y1) = (rect.0.min(w), rect.1.min(h), rect.2.min(w), rect.3.min(h));
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        for y in y0..y1 {
            let row = y * w;
            for x in x0..x1 {
                let i = row + x;
                if let (Some(a), Some(b), Some(o)) = (self.base.pixels.get(i), self.target.pixels.get(i), self.out.pixels.get_mut(i)) {
                    *o = mix(*a, *b, coverage(x, y));
                }
            }
        }
        Some((x0, y0, x1, y1))
    }

    /// The pixels of `rect` as an image (for a partial texture upload).
    pub fn region(&self, (x0, y0, x1, y1): PxRect) -> ColorImage {
        let w = self.out.size[0];
        let (cols, rows) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let mut px = Vec::with_capacity(cols.saturating_mul(rows));
        for y in y0..y0.saturating_add(rows) {
            match self.out.pixels.get(y * w + x0..y * w + x0 + cols) {
                Some(row) => px.extend_from_slice(row),
                // (a rectangle past the picture: keep the image the size asked for)
                None => px.resize(px.len() + cols, Color32::BLACK),
            }
        }
        ColorImage::new([cols, rows], px)
    }
}

/// How the stroke's screen-resolution buffer lines up with the picture drawn on `img_rect`.
#[derive(Clone, Copy, Debug)]
pub struct Mapping {
    /// The canvas the buffer covers (screen points) and the buffer's pixels per point.
    pub clip: Rect,
    pub scale: f32,
    /// Where the picture is drawn (screen points) and its size in pixels.
    pub img_rect: Rect,
    pub size: [usize; 2],
}

impl Mapping {
    /// The buffer pixel under the centre of picture pixel `(x, y)`.
    #[inline]
    pub fn buffer_px(&self, x: usize, y: usize) -> (usize, usize) {
        let [w, h] = self.size;
        let sx = self.img_rect.left() + (x as f32 + 0.5) / w.max(1) as f32 * self.img_rect.width();
        let sy = self.img_rect.top() + (y as f32 + 0.5) / h.max(1) as f32 * self.img_rect.height();
        let bx = ((sx - self.clip.left()) * self.scale).floor();
        let by = ((sy - self.clip.top()) * self.scale).floor();
        (bx.max(0.0) as usize, by.max(0.0) as usize)
    }

    /// The picture pixels that the buffer pixels `(x0, y0, x1, y1)` (exclusive ends) can touch,
    /// widened a pixel each side for rounding.
    pub fn picture_rect(&self, (x0, y0, x1, y1): PxRect) -> PxRect {
        let [w, h] = self.size;
        let to_x =
            |b: f32| ((self.clip.left() + b / self.scale - self.img_rect.left()) / self.img_rect.width().max(1e-6) * w as f32).clamp(0.0, w as f32);
        let to_y =
            |b: f32| ((self.clip.top() + b / self.scale - self.img_rect.top()) / self.img_rect.height().max(1e-6) * h as f32).clamp(0.0, h as f32);
        let lo = |v: f32| (v.floor() as usize).saturating_sub(1);
        let hi = |v: f32, max: usize| ((v.ceil() as usize).saturating_add(1)).min(max);
        (lo(to_x(x0 as f32)), lo(to_y(y0 as f32)), hi(to_x(x1 as f32), w), hi(to_y(y1 as f32), h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, pos2, vec2};

    fn solid(w: usize, h: usize, c: Color32) -> Arc<ColorImage> {
        Arc::new(ColorImage::new([w, h], vec![c; w * h]))
    }

    #[test]
    fn mix_is_exact_at_the_ends_and_between() {
        let (a, b) = (Color32::from_rgb(10, 20, 30), Color32::from_rgb(110, 220, 130));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, f32::NAN), a, "a coverage that is not a number leaves the base");
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(60, 120, 80));
    }

    #[test]
    fn pictures_of_different_sizes_do_not_blend() {
        assert!(Blend::new(solid(4, 4, Color32::BLACK), solid(4, 5, Color32::WHITE)).is_none());
        assert!(Blend::new(solid(0, 0, Color32::BLACK), solid(0, 0, Color32::WHITE)).is_none());
    }

    /// Updating the rectangles a stroke touched, frame by frame, gives the picture of updating
    /// everything from the final coverage.
    #[test]
    fn dirty_rectangles_equal_a_full_blend() {
        let (w, h) = (64, 48);
        let base = Arc::new(ColorImage::new([w, h], (0..w * h).map(|i| Color32::from_rgb((i % 251) as u8, (i % 13) as u8 * 9, 40)).collect()));
        let target =
            Arc::new(ColorImage::new([w, h], (0..w * h).map(|i| Color32::from_rgb(255 - (i % 200) as u8, 200, (i % 7) as u8 * 30)).collect()));
        let cov = |x: usize, y: usize| (((x * 3 + y * 5) % 11) as f32 / 10.0).min(1.0);
        let mut steps = Blend::new(base.clone(), target.clone()).unwrap();
        for r in [(0, 0, 20, 20), (10, 10, 40, 30), (30, 20, 64, 48)] {
            steps.update(r, cov);
        }
        let mut full = Blend::new(base, target).unwrap();
        full.update((0, 0, w, h), |x, y| {
            let inside = |r: PxRect| x >= r.0 && x < r.2 && y >= r.1 && y < r.3;
            if [(0, 0, 20, 20), (10, 10, 40, 30), (30, 20, 64, 48)].into_iter().any(inside) { cov(x, y) } else { 0.0 }
        });
        assert_eq!(steps.image().pixels, full.image().pixels);
    }

    #[test]
    fn full_coverage_is_the_target_and_none_is_the_base() {
        let (base, target) = (solid(8, 8, Color32::from_rgb(1, 2, 3)), solid(8, 8, Color32::from_rgb(200, 150, 100)));
        let mut b = Blend::new(base.clone(), target.clone()).unwrap();
        b.update((0, 0, 8, 4), |_, _| 1.0);
        b.update((0, 4, 8, 8), |_, _| 0.0);
        assert!(b.image().pixels[..32].iter().all(|p| *p == Color32::from_rgb(200, 150, 100)));
        assert!(b.image().pixels[32..].iter().all(|p| *p == Color32::from_rgb(1, 2, 3)));
        // a rectangle past the picture is clipped, one outside it is ignored
        assert_eq!(b.update((6, 6, 100, 100), |_, _| 1.0), Some((6, 6, 8, 8)));
        assert_eq!(b.update((9, 9, 20, 20), |_, _| 1.0), None);
        assert_eq!(b.region((6, 6, 8, 8)).size, [2, 2]);
    }

    /// A picture drawn at twice the buffer's resolution, offset in the canvas: buffer and picture
    /// pixels find each other, and the dirty rectangle maps over every picture pixel it touches.
    #[test]
    fn the_mapping_connects_buffer_and_picture_pixels() {
        let m = Mapping {
            clip: Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0)),
            scale: 1.0,
            img_rect: Rect::from_min_size(pos2(20.0, 10.0), vec2(160.0, 80.0)),
            size: [320, 160],
        };
        assert_eq!(m.buffer_px(0, 0), (20, 10));
        assert_eq!(m.buffer_px(319, 159), (179, 89));
        let (x0, y0, x1, y1) = m.picture_rect((50, 30, 60, 40));
        for by in 30..40 {
            for bx in 50..60 {
                // the picture pixels whose centre is in buffer pixel (bx, by) lie inside the rectangle
                let (px, py) = ((bx - 20) * 2, (by - 10) * 2);
                assert!((x0..x1).contains(&px) && (y0..y1).contains(&py), "{px},{py} outside {x0}..{x1} {y0}..{y1}");
                assert_eq!(m.buffer_px(px, py), (bx, by));
            }
        }
        // a rectangle outside the picture maps to an empty or clipped one
        let (a, _, b, _) = m.picture_rect((0, 0, 5, 5));
        assert!(a == 0 && b < 10);
    }

    /// A frame of painting on a 2.5 MP picture (a brush-sized rectangle blended and cut out for
    /// the upload, as `draw_blend` does) stays far inside the 16 ms frame.
    #[test]
    fn a_frame_of_blending_is_cheap_on_a_big_picture() {
        let (w, h) = (1920, 1300);
        let img = |c: Color32| Arc::new(ColorImage::new([w, h], vec![c; w * h]));
        let mut b = Blend::new(img(Color32::from_rgb(40, 60, 80)), img(Color32::from_rgb(200, 160, 120))).unwrap();
        let t = std::time::Instant::now();
        let frames = 30;
        for k in 0..frames {
            // a brush of radius 100 px that moved 30 px since the last frame
            let x = 100 + k * 30;
            let rect = (x - 100, 500, x + 130, 700);
            let done = b.update(rect, |x, y| ((x + y) % 7) as f32 / 6.0).unwrap();
            std::hint::black_box(b.region(done));
        }
        let per_frame = t.elapsed().as_secs_f64() * 1e3 / frames as f64;
        eprintln!("blend: {per_frame:.2} ms per frame on {w}x{h}");
        assert!(per_frame < 8.0, "{per_frame} ms per frame");
    }
}
