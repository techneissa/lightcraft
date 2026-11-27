use super::*;
use lightcraft_color::perceptual::{lch_to_lab, oklab_from_2020, oklab_to_2020};
use lightcraft_develop::{ColorSwap, DevelopSettings};

const BLUE: [f64; 3] = [0.20, 0.35, 0.80];
const RED: [f64; 3] = [0.80, 0.15, 0.12];
const GREEN: [f64; 3] = [0.15, 0.70, 0.25];

fn swap(from: [f64; 3], to: [f64; 3]) -> ColorSwap {
    ColorSwap { from: Some(from), to: Some(to), ..Default::default() }
}

fn stage(entries: Vec<ColorSwap>) -> ColorSwaps {
    ColorSwaps::new(&DevelopSettings { color_swaps: entries, ..Default::default() })
}

/// A display-linear pixel of OkLCh `lch`.
fn px(lch: [f32; 3]) -> [f32; 3] {
    oklab_to_2020(lch_to_lab(lch))
}

fn lch_of(d: [f32; 3]) -> [f32; 3] {
    lab_to_lch(oklab_from_2020(d))
}

/// The From colour's own lightness, chroma over lightness and hue: a shirt's colour at any light.
fn blue() -> (f32, f32, f32) {
    let [l, c, h] = lab_to_lch(srgb_to_oklab(BLUE));
    (l, c / l, h)
}

/// A shirt lit from deep shadow to highlight (the same hue and saturation at each lightness) swapped from blue to
/// red keeps its shading: each pixel's lightness stays, and so does the order of the folds.
#[test]
fn shading_and_texture_are_kept_in_light_and_in_shadow() {
    let (_, sat, h) = blue();
    let s = stage(vec![swap(BLUE, RED)]);
    let ramp: Vec<f32> = (0..16).map(|i| 0.22 + 0.04 * i as f32).collect();
    let red_h = lch_of(px([0.6, 0.15, lab_to_lch(srgb_to_oklab(RED))[2]]))[2];
    let mut last = f32::MIN;
    for l in ramp {
        let out = lch_of(s.apply(px([l, sat * l, h])));
        assert!((out[0] - l).abs() < 0.01, "lightness {l} → {}", out[0]);
        assert!(out[0] > last, "the folds' order changed at {l}");
        last = out[0];
        assert!(wrap(out[2] - red_h).abs() < 0.35, "at {l} the hue is {} (red {red_h})", out[2]);
        assert!(out[1] > 0.4 * sat * l, "at {l} the chroma is {}", out[1]);
    }
}

/// With Keep Lightness off the To colour's lightness is taken, folds in proportion: a darker pixel stays darker.
#[test]
fn without_keep_lightness_the_to_colour_sets_the_light_and_folds_stay_in_proportion() {
    let (_, sat, h) = blue();
    let white = [0.95, 0.95, 0.95];
    let mut e = swap(BLUE, white);
    e.keep_lightness = false;
    let s = stage(vec![e]);
    let out: Vec<f32> = [0.3f32, 0.4, 0.5].iter().map(|l| lch_of(s.apply(px([*l, sat * l, h])))[0]).collect();
    assert!(out.windows(2).all(|w| w[1] > w[0]), "{out:?}");
    assert!(out[2] > 0.8, "the From colour's own lightness reaches the To colour's: {out:?}");
}

/// Colours well outside the range come back exactly as they went in, bit for bit.
#[test]
fn colours_outside_the_range_are_unchanged() {
    let s = stage(vec![swap(BLUE, RED)]);
    for c in [GREEN, [0.9, 0.55, 0.1], [0.5, 0.5, 0.5], [0.05, 0.05, 0.05], [0.95, 0.9, 0.85], [0.85, 0.6, 0.5]] {
        let d =
            lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply_f32(c.map(|v| lightcraft_color::transfer::srgb_to_linear(v as f32)));
        assert_eq!(s.apply(d), d, "{c:?}");
    }
    let near =
        lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply_f32(BLUE.map(|v| lightcraft_color::transfer::srgb_to_linear(v as f32)));
    assert_ne!(s.apply(near), near);
}

/// Entries run in order, each on what the one before made.
#[test]
fn entries_combine_in_order() {
    let (l, sat, h) = blue();
    let d = px([l, sat * l, h]);
    let green_h = lab_to_lch(srgb_to_oklab(GREEN))[2];
    let red_h = lab_to_lch(srgb_to_oklab(RED))[2];
    let chain = stage(vec![swap(BLUE, RED), swap(RED, GREEN)]);
    let backwards = stage(vec![swap(RED, GREEN), swap(BLUE, RED)]);
    assert!(wrap(lch_of(chain.apply(d))[2] - green_h).abs() < 0.4, "blue → red → green");
    assert!(wrap(lch_of(backwards.apply(d))[2] - red_h).abs() < 0.4, "the red entry has nothing to take before blue made it");
}

/// An entry switched off, without a colour, at Intensity 0 or with its section's eye off changes nothing.
#[test]
fn an_entry_that_is_off_changes_nothing() {
    let (l, sat, h) = blue();
    let d = px([l, sat * l, h]);
    let off = ColorSwap { on: false, ..swap(BLUE, RED) };
    let no_to = ColorSwap { to: None, ..swap(BLUE, RED) };
    let none = ColorSwap { intensity: 0.0, ..swap(BLUE, RED) };
    for e in [off, no_to, none] {
        assert!(stage(vec![e]).is_identity());
    }
    let mut s = DevelopSettings { color_swaps: vec![swap(BLUE, RED)], ..Default::default() };
    s.set_section_enabled("colorSwap", false);
    assert!(ColorSwaps::new(&s).is_identity());
    assert_eq!(ColorSwaps::new(&s).apply(d), d);
}

/// The weight falls smoothly from the From colour out: it never rises, and no two neighbouring colours differ by a
/// step (a step would show as banding in a gradient or a halo at an edge).
#[test]
fn the_edge_falls_off_without_a_step() {
    let (l, sat, h) = blue();
    for softness in [0.0, 50.0, 100.0] {
        let k = SwapK::new(&ColorSwap { softness, ..swap(BLUE, RED) }).expect("a From colour");
        let step = 0.0025f32;
        let w: Vec<f32> = (0..500).map(|i| k.weight([l, sat * l, h + step * i as f32])).collect();
        assert!(w[0] > 0.99 && w[499] < 0.01, "{} .. {}", w[0], w[499]);
        assert!(w.windows(2).all(|p| p[1] <= p[0] + 1e-6), "the weight rose at softness {softness}");
        let max_step = w.windows(2).map(|p| p[0] - p[1]).fold(0.0, f32::max);
        assert!(max_step < 0.08, "a step of {max_step} at softness {softness}");
    }
}

/// A wider Range reaches colours a narrower one leaves alone.
#[test]
fn range_widens_what_is_reached() {
    let (l, sat, h) = blue();
    let off = px([l, sat * l, h + 0.45]);
    let narrow = stage(vec![ColorSwap { range: 20.0, ..swap(BLUE, RED) }]);
    let wide = stage(vec![ColorSwap { range: 90.0, ..swap(BLUE, RED) }]);
    assert_eq!(narrow.apply(off), off);
    assert_ne!(wide.apply(off), off);
}

/// The range view shows the entry's range in colour and everything else dimmed and grey.
#[test]
fn the_range_view_keeps_colour_inside_and_drains_it_outside() {
    let (l, sat, h) = blue();
    let mut e = swap(BLUE, RED);
    e.view = true;
    let s = stage(vec![e]);
    let inside = px([l, sat * l, h]);
    let outside =
        lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply_f32(GREEN.map(|v| lightcraft_color::transfer::srgb_to_linear(v as f32)));
    let (i, o) = (lch_of(s.apply(inside)), lch_of(s.apply(outside)));
    assert!(i[1] > 0.5 * sat * l, "inside keeps its colour: {i:?}");
    assert!(o[1] < 0.02 && o[0] < lch_of(outside)[0], "outside is grey and dim: {o:?}");
}

/// A weakly coloured pixel between the From colour and something else (the grey-purple where a purple stripe meets an
/// olive one) takes the new hue at its own chroma and lightness, so no thin line of the old hue is left along an edge.
#[test]
fn a_weak_mix_at_an_edge_takes_the_new_hue_but_not_new_chroma() {
    let (_, _, h) = blue();
    let s = stage(vec![swap(BLUE, RED)]);
    let red_h = lab_to_lch(srgb_to_oklab(RED))[2];
    let mix = [0.42, 0.022, h + 0.5];
    let out = lch_of(s.apply(px(mix)));
    assert!(wrap(out[2] - red_h).abs() < wrap(mix[2] - red_h).abs() * 0.7, "hue {} → {} (red {red_h})", mix[2], out[2]);
    assert!(out[1] < 0.05 && (out[0] - mix[0]).abs() < 0.01, "chroma and lightness stay: {out:?}");
}
