//! Recolor through the whole render (issue #83): only the masked area changes, Amount 0 changes nothing, and a
//! preview, a window and an export recolor alike.

use lightcraft_color::perceptual::{lab_to_lch, oklab_from_2020};
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::recolor::srgb_to_oklab;
use crate::{PixelWindow, RenderRequest, SourceInfo, render};

const W: usize = 240;
const H: usize = 120;
const PURPLE: [f64; 3] = [0x7a as f64 / 255.0, 0x3f as f64 / 255.0, 0xb0 as f64 / 255.0];

/// A red boot-like gradient on the left (darker at the top, a lighter band, shading across), green on the right.
fn scene(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let shade = 0.25 + 0.75 * v + 0.3 * (u * 9.0).sin().abs();
        if u < 0.5 { [0.30 * shade, 0.05 * shade, 0.04 * shade] } else { [0.05, 0.20, 0.06] }
    })
}

/// One mask over the left half (x < 0.45) with Recolor to `to` at `amount`.
fn recolored(amount: f64, to: [f64; 3]) -> DevelopSettings {
    let shape = MaskShape::Linear { start: Point::new(0.45, 0.5), end: Point::new(0.46, 0.5) };
    let adjust = LocalAdjustments { recolor: amount, recolor_r: to[0], recolor_g: to[1], recolor_b: to[2], ..Default::default() };
    let mut s = DevelopSettings::default();
    s.masks.push(Mask { id: 1, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], adjust, ..Default::default() });
    s
}

fn shot(src: &Rgb32f, s: &DevelopSettings, w: usize, h: usize) -> Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(w, h)).image
}

fn lch(p: [u8; 4]) -> [f32; 3] {
    lab_to_lch(oklab_from_2020(
        lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply_f32([p[0], p[1], p[2]].map(|v| srgb_to_linear(f32::from(v) / 255.0))),
    ))
}

#[test]
fn only_the_masked_area_changes_and_amount_zero_changes_nothing() {
    let src = scene(W, H);
    let plain = shot(&src, &DevelopSettings::default(), W, H);
    let on = shot(&src, &recolored(100.0, PURPLE), W, H);
    for y in 0..H {
        for x in (W * 47 / 100)..W {
            assert_eq!(on.get(x, y), plain.get(x, y), "outside the mask at ({x}, {y})");
        }
    }
    assert_ne!(on.get(20, 60), plain.get(20, 60));
    // Amount 0, or the mask's own Amount 0, is the photo without the mask, byte for byte
    assert_eq!(shot(&src, &recolored(0.0, PURPLE), W, H).data, plain.data);
    let mut off = recolored(100.0, PURPLE);
    off.masks[0].adjust.amount = 0.0;
    assert_eq!(shot(&src, &off, W, H).data, plain.data);
}

#[test]
fn the_area_takes_the_hue_and_keeps_its_lightness_profile() {
    let src = scene(W, H);
    let plain = shot(&src, &DevelopSettings::default(), W, H);
    let on = shot(&src, &recolored(100.0, PURPLE), W, H);
    let purple = lab_to_lch(srgb_to_oklab(PURPLE))[2];
    let column: Vec<(f32, f32)> = (4..H - 4).map(|y| (lch(plain.get(30, y))[0], lch(on.get(30, y))[0])).collect();
    for y in (4..H - 4).step_by(7) {
        let h = lch(on.get(30, y))[2];
        let d = (h - purple + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        assert!(d.abs() < 0.2, "hue at row {y}: {h} vs {purple}");
    }
    // lighter where it was lighter, in the same proportion
    for pair in column.windows(2) {
        let ((a0, a1), (b0, b1)) = (pair[0], pair[1]);
        if (b0 - a0).abs() > 0.01 {
            assert_eq!((b1 - a1).signum(), (b0 - a0).signum(), "{pair:?}");
        }
    }
    let ratio = |c: &[(f32, f32)], f: fn(&(f32, f32)) -> f32| f(c.last().unwrap_or(&(1.0, 1.0))) / f(c.first().unwrap_or(&(1.0, 1.0)));
    let (before, after) = (ratio(&column, |p| p.0), ratio(&column, |p| p.1));
    assert!((after / before - 1.0).abs() < 0.25, "lightness profile {before} → {after}");
}

/// The export (4x the preview) scaled down to the preview's size matches the preview as closely as without
/// recolor, and a window of the export matches its full render.
#[test]
fn preview_window_and_export_recolor_alike() {
    let src = scene(960, 480);
    let s = recolored(100.0, PURPLE);
    let diff = |s: &DevelopSettings| {
        let (small, big) = (shot(&src, s, W, H), shot(&src, s, W * 4, H * 4));
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in 0..H {
            for x in 4..W * 4 / 10 {
                let p = small.get(x, y);
                for k in 0..3 {
                    let m: f64 = (0..16).map(|i| f64::from(big.get(x * 4 + i % 4, y * 4 + i / 4)[k])).sum::<f64>() / 16.0;
                    sum += (f64::from(p[k]) - m).abs();
                    n += 1.0;
                }
            }
        }
        sum / n
    };
    let (with, without) = (diff(&s), diff(&DevelopSettings::default()));
    assert!(with <= without + 0.75, "preview vs export: {with:.3} (without recolor {without:.3})");
    let win = PixelWindow { x: 40, y: 100, w: 200, h: 160 };
    let full = shot(&src, &s, W * 4, H * 4);
    let part = render(&src, &SourceInfo::default(), &s, &RenderRequest { window: Some(win), ..RenderRequest::fit(W * 4, H * 4) }).image;
    for y in 0..win.h {
        for x in 0..win.w {
            let (a, b) = (full.get(win.x + x, win.y + y), part.get(x, y));
            assert!((0..3).all(|k| a[k].abs_diff(b[k]) <= 1), "window at ({x}, {y}): {a:?} vs {b:?}");
        }
    }
}

/// A Describe mask ("boots", SAM 3's segmentation kept in the mask) recolors what it selects and nothing else.
#[test]
fn a_describe_mask_recolors_what_it_selects() {
    let side = 64;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = (((i % side) as f32 + 0.5) / side as f32, ((i / side) as f32 + 0.5) / side as f32);
            let r = (((x - 0.25) / 0.15).powi(2) + ((y - 0.5) / 0.3).powi(2)).sqrt();
            (48.0 * (1.0 - r)).clamp(-12.0, 12.0)
        })
        .collect();
    let seg = lightcraft_develop::SegMask::from_logits(side, &logits);
    let Ok(mut shape) = serde_json::from_value::<MaskShape>(serde_json::json!({"kind": "prompt", "text": "boots"})) else { panic!("a prompt shape") };
    if let MaskShape::Prompt { seg: s, .. } = &mut shape {
        *s = Some(seg);
    }
    let mut s = recolored(100.0, PURPLE);
    s.masks[0].components[0].shape = shape;
    let src = scene(W, H);
    let plain = shot(&src, &DevelopSettings::default(), W, H);
    let on = shot(&src, &s, W, H);
    let purple = lab_to_lch(srgb_to_oklab(PURPLE))[2];
    let h = lch(on.get(W / 4, H / 2))[2];
    assert!((h - purple).abs() < 0.2, "the boot is purple: {h} vs {purple}");
    assert_eq!(on.get(W - 5, 5), plain.get(W - 5, 5), "outside what it selects");
    assert_eq!(on.get(3, 3), plain.get(3, 3));
}
