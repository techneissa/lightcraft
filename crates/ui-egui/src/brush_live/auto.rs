//! Auto Mask while painting: the live stroke weighs each dab's pixels by their median colour's
//! distance to the dab's reference, as the committed stroke does
//! ([`lightcraft_pipeline::masks::AutoRefs`], `masks::auto_edge`), reading the picture at the size
//! it's on screen instead of the source.

use egui::ColorImage;
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_pipeline::local::log_lum;
use lightcraft_pipeline::masks::{AutoColor, auto_color, median3};
use std::cell::RefCell;
use std::rc::Rc;

/// The colour of every pixel of the picture a stroke is painted over, and its 3 × 3 median colour
/// (worked out the first time a stroke reads it).
pub struct Guide {
    size: [usize; 2],
    colors: Vec<AutoColor>,
    medians: RefCell<Vec<Option<AutoColor>>>,
}

impl Guide {
    pub fn new(img: &ColorImage) -> Guide {
        let lut: Vec<f32> = (0..=255u32).map(|v| srgb_to_linear(v as f32 / 255.0)).collect();
        let lin = |v: u8| lut.get(usize::from(v)).copied().unwrap_or(0.0);
        let colors: Vec<AutoColor> = img
            .pixels
            .iter()
            .map(|p| {
                let c = [lin(p.r()), lin(p.g()), lin(p.b())];
                auto_color(log_lum(c), c)
            })
            .collect();
        let medians = RefCell::new(vec![None; colors.len()]);
        Guide { size: img.size, colors, medians }
    }

    /// The picture's size (pixels).
    pub fn size(&self) -> [usize; 2] {
        self.size
    }

    /// The pixel at fractions `(fx, fy)` of the picture (clamped to it).
    #[inline]
    pub fn pixel(&self, fx: f32, fy: f32) -> (isize, isize) {
        let [w, h] = self.size;
        let px = |f: f32, n: usize| if f.is_finite() { ((f * n as f32).floor().max(0.0) as usize).min(n.saturating_sub(1)) } else { 0 };
        (px(fx, w) as isize, px(fy, h) as isize)
    }

    /// The index of pixel `(x, y)` clamped to the picture.
    #[inline]
    fn index(&self, x: isize, y: isize) -> usize {
        let [w, h] = self.size;
        let c = |v: isize, n: usize| v.clamp(0, n.saturating_sub(1) as isize) as usize;
        c(y, h) * w + c(x, w)
    }

    /// The colour of pixel `(x, y)` (clamped to the picture).
    #[inline]
    pub fn color(&self, x: isize, y: isize) -> AutoColor {
        self.colors.get(self.index(x, y)).copied().unwrap_or([0.0, 1.0, 1.0, 1.0])
    }

    /// The 3 × 3 median colour of pixel `(x, y)` (clamped to the picture), as the committed stroke
    /// compares pixels.
    pub fn median(&self, x: isize, y: isize) -> AutoColor {
        let i = self.index(x, y);
        let Ok(mut memo) = self.medians.try_borrow_mut() else {
            return median3(|dx, dy| self.color(x + dx, y + dy));
        };
        match memo.get(i) {
            Some(Some(c)) => *c,
            _ => {
                let c = median3(|dx, dy| self.color(x + dx, y + dy));
                if let Some(slot) = memo.get_mut(i) {
                    *slot = Some(c);
                }
                c
            }
        }
    }
}

thread_local! {
    /// The guide of the last picture asked for: a stroke asks every frame, and a picture is the
    /// same until it is re-rendered.
    static LAST: RefCell<Option<(u64, Rc<Guide>)>> = const { RefCell::new(None) };
}

/// The guide of `img`, the render with key `key` (built once per render).
pub fn guide_for(key: u64, img: &ColorImage) -> Rc<Guide> {
    LAST.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else { return Rc::new(Guide::new(img)) };
        match slot.as_ref() {
            Some((k, g)) if *k == key => g.clone(),
            _ => {
                let g = Rc::new(Guide::new(img));
                *slot = Some((key, g.clone()));
                g
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    #[test]
    fn the_guide_tells_a_surface_from_its_neighbour() {
        use lightcraft_pipeline::masks::AutoRef;
        // left half grey, right half red
        let px =
            (0..16).flat_map(|_| (0..16).map(|x| if x < 8 { Color32::from_rgb(120, 120, 120) } else { Color32::from_rgb(200, 40, 40) })).collect();
        let g = Guide::new(&ColorImage::new([16, 16], px));
        let at = |fx, fy| {
            let (x, y) = g.pixel(fx, fy);
            g.median(x, y)
        };
        let c = at(0.25, 0.5);
        let r = AutoRef { lo: c, hi: c };
        assert!(r.weight(at(0.1, 0.9)) > 0.99, "the same surface");
        assert!(r.weight(at(0.9, 0.5)) < 0.01, "another surface");
        // out of range and hostile positions don't panic
        let _ = (g.pixel(-3.0, 9.0), g.pixel(f32::NAN, f32::INFINITY), g.median(-5, 99));
    }

    #[test]
    fn the_guide_is_built_once_per_render() {
        let img = ColorImage::new([2, 2], vec![Color32::GRAY; 4]);
        let (a, b) = (guide_for(7, &img), guide_for(7, &img));
        assert!(Rc::ptr_eq(&a, &b));
        assert!(!Rc::ptr_eq(&a, &guide_for(8, &img)));
    }
}
