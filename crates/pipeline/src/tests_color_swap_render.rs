//! Recolor through the whole render (issue #460): only the colours an entry reaches change, an entry that is off
//! changes nothing, and the range view and the before show what they say.

use lightcraft_color::perceptual::{lab_to_lch, oklab_from_2020};
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_develop::{ColorSwap, DevelopSettings};
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::{Overlay, RenderRequest, SourceInfo, render};

const W: usize = 240;
const H: usize = 120;
const BLUE: [f64; 3] = [0.15, 0.30, 0.85];

/// A shaded red object on the left (darker at the top, a lighter band, folds across), a green wall on the right.
fn scene() -> Rgb32f {
    Rgb32f::from_fn(W, H, |x, y| {
        let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
        let shade = 0.25 + 0.75 * v + 0.3 * (u * 9.0).sin().abs();
        if u < 0.5 { [0.30 * shade, 0.05 * shade, 0.04 * shade] } else { [0.05, 0.20, 0.06] }
    })
}

fn red_to_blue() -> DevelopSettings {
    // the From colour is a red of the object as it renders, picked from the plain render
    let plain = shot(&DevelopSettings::default(), Overlay::None);
    let p = plain.get(30, 70);
    let from = [p[0], p[1], p[2]].map(|v| f64::from(v) / 255.0);
    DevelopSettings { color_swaps: vec![ColorSwap { from: Some(from), to: Some(BLUE), ..Default::default() }], ..Default::default() }
}

fn shot(s: &DevelopSettings, overlay: Overlay) -> Rgba8 {
    render(&scene(), &SourceInfo::default(), s, &RenderRequest { overlay, ..RenderRequest::fit(W, H) }).image
}

fn lch(p: [u8; 4]) -> [f32; 3] {
    lab_to_lch(oklab_from_2020(
        lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply_f32([p[0], p[1], p[2]].map(|v| srgb_to_linear(f32::from(v) / 255.0))),
    ))
}

#[test]
fn only_what_the_entry_reaches_changes_and_an_entry_that_is_off_changes_nothing() {
    let plain = shot(&DevelopSettings::default(), Overlay::None);
    let s = red_to_blue();
    let on = shot(&s, Overlay::None);
    for y in 0..H {
        for x in (W * 55 / 100)..W {
            assert_eq!(on.get(x, y), plain.get(x, y), "the green wall at ({x}, {y})");
        }
    }
    let (before, after) = (lch(plain.get(30, 70)), lch(on.get(30, 70)));
    assert!((after[2] - before[2]).abs() > 1.0, "the object's hue moved: {before:?} → {after:?}");
    assert!((after[0] - before[0]).abs() < 0.03, "its lightness stayed");
    // the entry off, and its section's eye off: the photo as it was, byte for byte
    let mut off = s.clone();
    off.color_swaps[0].on = false;
    assert_eq!(shot(&off, Overlay::None).data, plain.data);
    let mut eye = s.clone();
    eye.set_section_enabled("colorSwap", false);
    assert_eq!(shot(&eye, Overlay::None).data, plain.data);
    // Before: the same entry left out of its own render
    assert_eq!(shot(&s, Overlay::ColorSwapBefore(0)).data, plain.data);
}

#[test]
fn the_range_view_shows_the_object_in_colour_and_the_wall_dimmed() {
    let s = red_to_blue();
    let plain = shot(&DevelopSettings::default(), Overlay::None);
    let view = shot(&s, Overlay::ColorSwapRange(0));
    let (object, wall, wall_plain) = (lch(view.get(30, 70)), lch(view.get(200, 60)), lch(plain.get(200, 60)));
    assert!(object[1] > 0.05, "the object keeps its colour: {object:?}");
    assert!(wall[1] < 0.01 && wall[0] < wall_plain[0], "the wall is grey and darker: {wall:?} vs {wall_plain:?}");
}
