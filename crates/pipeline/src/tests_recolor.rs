use super::*;
use lightcraft_color::perceptual::{lab_to_lch, lch_to_lab, oklab_from_2020};

fn hue_dist(a: f32, b: f32) -> f32 {
    wrap(a - b).abs()
}

fn reference_of(pixels: &[[f32; 3]]) -> Reference {
    let mut s = Sums::default();
    pixels.iter().for_each(|p| s.add(*p, 1.0));
    s.reference(1.0).unwrap_or(Reference { l: 0.5, c: 0.0, h: [1.0, 0.0] })
}

/// A red gradient (lightness 0.3 → 0.7, as shading on a red boot) recolored to a purple as light as its mean keeps
/// every pixel's lightness and takes the purple's hue.
#[test]
fn a_red_gradient_recolored_to_purple_keeps_its_lightness_profile() {
    let red_hue = lab_to_lch(oklab_from_2020([0.6, 0.05, 0.04]))[2];
    let grad: Vec<[f32; 3]> = (0..21).map(|i| lch_to_lab([0.3 + 0.02 * i as f32, 0.15 * (0.6 + 0.02 * i as f32), red_hue])).collect();
    let r = reference_of(&grad);
    let purple = lab_to_lch(srgb_to_oklab([0.48, 0.25, 0.69]));
    let target = lch_to_lab([r.l, purple[1], purple[2]]);
    for p in &grad {
        let o = apply(*p, target, &r, 1.0);
        let (lo, li) = (lab_to_lch(o), lab_to_lch(*p));
        assert!((lo[0] - li[0]).abs() < 1e-4, "lightness {} → {}", li[0], lo[0]);
        assert!(hue_dist(lo[2], purple[2]) < 0.05, "hue {} vs purple {}", lo[2], purple[2]);
        assert!(lo[1] > 0.05, "chroma {}", lo[1]);
    }
    // a target lighter than the area keeps the profile's shape: same order, same proportion below the mean
    let lighter = lch_to_lab([r.l + 0.1, purple[1], purple[2]]);
    let ls: Vec<f32> = grad.iter().map(|p| apply(*p, lighter, &r, 1.0)[0]).collect();
    assert!(ls.windows(2).all(|w| w[1] > w[0]));
    let below = grad.iter().zip(&ls).filter(|(p, _)| p[0] < r.l);
    for (p, l) in below {
        assert!((l / (r.l + 0.1) - p[0] / r.l).abs() < 1e-4);
    }
}

/// A black garment with folds recolored to white keeps the folds' contrast in proportion: a fold half as light as
/// the garment's mean is half as light as the new white.
#[test]
fn a_black_garment_recolored_to_white_keeps_its_fold_contrast_in_proportion() {
    let folds = [0.08f32, 0.12, 0.16, 0.2, 0.24, 0.2, 0.16];
    let px: Vec<[f32; 3]> = folds.iter().map(|l| [*l, 0.002, -0.003]).collect();
    let r = reference_of(&px);
    let white = srgb_to_oklab([0.95, 0.95, 0.95]);
    let out: Vec<[f32; 3]> = px.iter().map(|p| apply(*p, white, &r, 1.0)).collect();
    for (p, o) in px.iter().zip(&out) {
        if p[0] <= r.l {
            assert!((o[0] / white[0] - p[0] / r.l).abs() < 1e-4, "{} → {}", p[0], o[0]);
        }
        assert!(o[0] <= 1.0 + 1e-6 && o[0] > p[0]);
        // white stays neutral: no random hue from the black's noise
        assert!(o[1].hypot(o[2]) < 0.01, "chroma {:?}", o);
    }
    let (dark, light) = (out[0][0], out[4][0]);
    assert!(light / dark > 1.5, "the folds keep their contrast: {dark} vs {light}");
}

/// A near-grey area (a black shirt) recolored to red takes red chroma following its shading; its white specular
/// highlight stays neutral.
#[test]
fn a_grey_area_gets_chroma_from_its_shading_and_its_specular_stays_neutral() {
    let px: Vec<[f32; 3]> = [0.15f32, 0.2, 0.25, 0.3].iter().map(|l| [*l, 0.0, 0.0]).collect();
    let r = reference_of(&px);
    let red = srgb_to_oklab([0.8, 0.1, 0.1]);
    let shaded = apply([0.15, 0.0, 0.0], red, &r, 1.0);
    let lit = apply([0.3, 0.0, 0.0], red, &r, 1.0);
    assert!(lit[1].hypot(lit[2]) > shaded[1].hypot(shaded[2]));
    let spec = apply([1.0, 0.0, 0.0], red, &r, 1.0);
    assert!(spec[1].hypot(spec[2]) < 0.02, "{spec:?}");
    // a red area's grey highlight stays as grey, relatively
    let red_px = vec![[0.5, 0.15, 0.08], [0.55, 0.16, 0.08], [0.9, 0.01, 0.005]];
    let rr = reference_of(&red_px);
    let blue = srgb_to_oklab([0.1, 0.2, 0.8]);
    let hi = apply(red_px[2], blue, &rr, 1.0);
    let body = apply(red_px[0], blue, &rr, 1.0);
    assert!(hi[1].hypot(hi[2]) < 0.25 * body[1].hypot(body[2]));
}

/// Amount 0 (or no alpha) returns the pixel bit for bit; the terms of an Amount-0 recolor are all zeros.
#[test]
fn amount_zero_and_no_alpha_change_nothing() {
    let r = Reference { l: 0.5, c: 0.1, h: [1.0, 0.0] };
    let p = [0.43, 0.12, -0.07];
    let t = srgb_to_oklab([0.2, 0.7, 0.3]);
    assert_eq!(apply(p, t, &r, 0.0), p);
    assert_eq!(apply(p, t, &r, f32::NAN), p);
    let j = LocalAdjustments { recolor: 0.0, recolor_r: 0.5, ..Default::default() };
    assert_eq!(terms(&j, 1.0, Some(&r)), [0.0; TERMS]);
    let j = LocalAdjustments { recolor: 100.0, recolor_r: 0.5, ..Default::default() };
    let tm = terms(&j, 1.0, Some(&r));
    assert_eq!(apply_terms(p, &tm, 0.0), p);
    assert_ne!(apply_terms(p, &tm, 1.0), p);
    assert_eq!(terms(&j, 1.0, None), [0.0; TERMS]);
}

/// Hostile values (NaN, infinite, negative) never panic and never give NaN.
#[test]
fn hostile_values_stay_finite() {
    let bad = [f32::NAN, f32::INFINITY, -1.0, 0.0, 2.0];
    for l in bad {
        for c in bad {
            let r = Reference { l, c, h: [c, l] };
            let o = apply([0.5, 0.1, 0.1], [l, c, 0.1], &r, 1.0);
            assert!(o.iter().all(|v| v.is_finite()), "{o:?}");
        }
    }
    assert!(Sums::default().reference(1.0).is_none());
    let mut s = Sums::default();
    s.add([f32::NAN, 0.0, 0.0], 1.0);
    s.add([0.5, 0.0, 0.0], f32::INFINITY);
    assert!(s.reference(0.5).is_none());
    let mut img = vec![[0.5f32, 0.2, 0.2]; 16];
    recolor_srgb(&mut img, &[1.0; 16], [f64::NAN, 2.0, -1.0], 1.0);
    assert!(img.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn hex_colours_round_trip_and_bad_ones_are_refused() {
    assert_eq!(parse_hex("#7a3fb0").map(to_hex).as_deref(), Some("#7a3fb0"));
    assert_eq!(parse_hex(" 73b ").map(to_hex).as_deref(), Some("#7733bb"));
    for bad in ["", "#12", "#12345g", "#1234567", "#éé1", "zzz"] {
        assert_eq!(parse_hex(bad), None, "{bad}");
    }
    assert_eq!(to_hex([f64::NAN, 2.0, -1.0]), "#00ff00");
}

/// In a coloured area, a pixel that is grey for its lightness (a grey toe cap, a white sole) stays grey, and a
/// streak far more saturated than the area is held near [`MAX_CHROMA_RATIO`] of the target's chroma.
#[test]
fn grey_parts_of_a_coloured_area_stay_grey_and_bright_streaks_stay_in_bounds() {
    let shoe = vec![[0.45, 0.07, 0.01], [0.5, 0.08, 0.0], [0.55, 0.085, 0.01], [0.62, 0.012, 0.008]];
    let r = reference_of(&shoe);
    let purple = srgb_to_oklab([0.48, 0.25, 0.69]);
    let cap = apply(shoe[3], purple, &r, 1.0);
    assert!(cap[1].hypot(cap[2]) < 0.005, "the toe cap stays grey: {cap:?}");
    let streak = apply([0.5, 0.4, 0.0], purple, &r, 1.0);
    let ct = purple[1].hypot(purple[2]);
    assert!(streak[1].hypot(streak[2]) <= ct * MAX_CHROMA_RATIO + 1e-4);
    assert!(chroma_ratio(0.2, 0.1, 0.5) < chroma_ratio(0.4, 0.1, 0.5));
}
