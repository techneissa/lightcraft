//! Colour Range (issue #279): one click selects a surface in light and shade, and nothing else.

use super::color_range::{median, representatives};
use super::*;

/// The selection-space colour of each pixel (as `mask.sampleColor` reads it).
fn labs(img: &Rgb32f) -> Vec<[f32; 3]> {
    img.data.iter().map(|c| lightcraft_color::perceptual::oklab_from_2020(tonemap_for_select(*c))).collect()
}

/// A little multiplicative noise, the same on every run.
fn noise(i: usize) -> f32 {
    let x = (i as u32).wrapping_mul(2_654_435_761).rotate_left(13).wrapping_mul(2_246_822_519);
    1.0 + 0.08 * ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
}

/// Issue #279: a red surface, lit at the top and 2 EV darker in shade at the bottom, beside a neutral grey patch of
/// the same brightness, with some noise. One click on the lit red at the default Refine selects all of the red,
/// shade included, without a speckle, and none of the grey.
#[test]
fn one_click_selects_a_surface_in_light_and_shade_and_not_the_grey() {
    let (w, h) = (160usize, 80usize);
    let red = [0.55f32, 0.06, 0.05];
    let grey = [0.15f32; 3];
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let base = if x < 100 { if y < 40 { red } else { red.map(|v| v * 0.25) } } else { grey };
        base.map(|v| v * noise(y * w + x))
    });
    let log_l = img.map(crate::local::log_lum);
    let f = Frame::new(w, h, &Default::default(), true);
    // the click: the median of the neighbourhood on the lit red
    let all = labs(&img);
    let near: Vec<[f32; 3]> = (18..23).flat_map(|y| (48..53).map(move |x| y * w + x)).map(|i| all[i]).collect();
    let sample = median(&near).unwrap();
    let a = shape_alpha(&MaskShape::ColorRange { samples: vec![sample], refine: 50.0 }, &f, w, h, &img, &log_l, 0.0, None);
    let (mut lit, mut shade, mut grey_max) = (f32::MAX, f32::MAX, 0f32);
    // (a few pixels in from the edges between the patches, where the soft edge is)
    for y in 0..h {
        for x in 0..w {
            let v = a.get(x, y);
            if x < 95 && y < 35 {
                lit = lit.min(v);
            } else if x < 95 && y > 45 {
                shade = shade.min(v);
            } else if x > 105 {
                grey_max = grey_max.max(v);
            }
        }
    }
    assert!(lit > 0.95, "the lit red is selected: {lit}");
    assert!(shade > 0.95, "the red in shade is selected: {shade}");
    assert!(grey_max < 0.05, "the grey stays out: {grey_max}");
}

/// A box over two colours picks both; a box over one flat colour picks that one only.
#[test]
fn a_box_picks_the_colours_in_it() {
    let red = lightcraft_color::perceptual::oklab_from_2020(tonemap_for_select([0.55, 0.06, 0.05]));
    let blue = lightcraft_color::perceptual::oklab_from_2020(tonemap_for_select([0.05, 0.1, 0.5]));
    let two: Vec<[f32; 3]> = (0..400).map(|i| if i % 5 < 3 { red } else { blue }).collect();
    let reps = representatives(&two, 4);
    assert_eq!(reps.len(), 2, "{reps:?}");
    let near = |c: [f32; 3]| reps.iter().any(|r| (0..3).all(|k| (r[k] - f64::from(c[k])).abs() < 1e-4));
    assert!(near(red) && near(blue), "{reps:?}");
    // a single stray pixel is not a colour of its own
    let mut one = vec![red; 400];
    one[7] = blue;
    assert_eq!(representatives(&one, 4).len(), 1);
    assert!(representatives(&[], 4).is_empty());
}
