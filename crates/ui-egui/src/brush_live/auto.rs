//! Auto Mask while painting: the live stroke weighs each dab's pixels by how much they look like
//! the pixel under the dab's centre ([`lightcraft_pipeline::masks::auto_similarity`], as the
//! committed stroke does), reading the picture at the size it's on screen instead of the source.
//! The committed stroke also snaps its edges to the photo with a guided filter at full
//! resolution; that comes with the render once the button is up.

use egui::ColorImage;
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_pipeline::local::log_lum;
use lightcraft_pipeline::masks::chromaticity;
use std::cell::RefCell;
use std::sync::Arc;

/// Log luminance and chromaticity of every pixel of the picture a stroke is painted over.
pub struct Guide {
    size: [usize; 2],
    logl: Vec<f32>,
    chroma: Vec<[f32; 3]>,
}

impl Guide {
    pub fn new(img: &ColorImage) -> Guide {
        let lut: Vec<f32> = (0..=255u32).map(|v| srgb_to_linear(v as f32 / 255.0)).collect();
        let lin = |v: u8| lut.get(usize::from(v)).copied().unwrap_or(0.0);
        let (logl, chroma) = img
            .pixels
            .iter()
            .map(|p| {
                let c = [lin(p.r()), lin(p.g()), lin(p.b())];
                (log_lum(c), chromaticity(c))
            })
            .unzip();
        Guide { size: img.size, logl, chroma }
    }

    /// The pixel at fractions `(fx, fy)` of the picture (clamped to it): (log luminance, chromaticity).
    #[inline]
    pub fn at(&self, fx: f32, fy: f32) -> (f32, [f32; 3]) {
        let [w, h] = self.size;
        let px = |f: f32, n: usize| if f.is_finite() { ((f * n as f32).floor().max(0.0) as usize).min(n.saturating_sub(1)) } else { 0 };
        let i = px(fy, h) * w + px(fx, w);
        match (self.logl.get(i), self.chroma.get(i)) {
            (Some(l), Some(c)) => (*l, *c),
            _ => (0.0, [1.0; 3]),
        }
    }
}

thread_local! {
    /// The guide of the last picture asked for: a stroke asks every frame, and a picture is the
    /// same until it is re-rendered.
    static LAST: RefCell<Option<(u64, Arc<Guide>)>> = const { RefCell::new(None) };
}

/// The guide of `img`, the render with key `key` (built once per render).
pub fn guide_for(key: u64, img: &ColorImage) -> Arc<Guide> {
    LAST.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else { return Arc::new(Guide::new(img)) };
        match slot.as_ref() {
            Some((k, g)) if *k == key => g.clone(),
            _ => {
                let g = Arc::new(Guide::new(img));
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
        use lightcraft_pipeline::masks::auto_similarity;
        // left half grey, right half red
        let px =
            (0..16).flat_map(|_| (0..16).map(|x| if x < 8 { Color32::from_rgb(120, 120, 120) } else { Color32::from_rgb(200, 40, 40) })).collect();
        let g = Guide::new(&ColorImage::new([16, 16], px));
        let (rl, rch) = g.at(0.25, 0.5);
        let (l, ch) = g.at(0.1, 0.9);
        assert!(auto_similarity(l, ch, rl, rch) > 0.99, "the same surface");
        let (l, ch) = g.at(0.9, 0.5);
        assert!(auto_similarity(l, ch, rl, rch) < 0.01, "another surface");
        // out of range and hostile positions don't panic
        let _ = (g.at(-3.0, 9.0), g.at(f32::NAN, f32::INFINITY));
    }

    #[test]
    fn the_guide_is_built_once_per_render() {
        let img = ColorImage::new([2, 2], vec![Color32::GRAY; 4]);
        let (a, b) = (guide_for(7, &img), guide_for(7, &img));
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &guide_for(8, &img)));
    }
}
