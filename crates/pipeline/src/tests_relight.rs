//! Relight through the whole render (fork, issue #288): Amount 0 or no field changes nothing, the light brightens
//! what is near it, and a draft and an export four times its size make the same light, with a crop too.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_raster::{Rgb32f, Rgba8};
use lightcraft_relight::Field;

use crate::{RenderRequest, SourceInfo, relight, render};

const W: usize = 480;
const H: usize = 320;

/// A mid-grey wall with a texture, and a nearer square (a quarter of the way in) on it.
fn scene(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let t = 0.08 + 0.02 * ((u * 40.0).sin() * (v * 30.0).cos());
        if near(u, v) { [t * 1.4, t * 1.2, t] } else { [t; 3] }
    })
}

fn near(u: f32, v: f32) -> bool {
    (0.2..0.45).contains(&u) && (0.25..0.7).contains(&v)
}

/// The scene's field: the square at log depth −0.8, the wall at 0.
fn info() -> SourceInfo {
    let (fw, fh) = (240, 160);
    let lz = (0..fw * fh).map(|i| if near((i % fw) as f32 / fw as f32 + 0.002, (i / fw) as f32 / fh as f32 + 0.003) { -0.8 } else { 0.0 }).collect();
    let field = Field::from_parts(fw, fh, lz, vec![-2.0; fw * fh]).unwrap();
    SourceInfo { relight: Some(relight::Input { field: Arc::new(field), key: 7 }), ..SourceInfo::default() }
}

fn lit(amount: f64) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.relight.amount = amount;
    s.relight.x = 0.3;
    s.relight.y = 0.45;
    s.relight.depth = 10.0;
    s
}

fn luma(p: [u8; 4]) -> f32 {
    0.25 * f32::from(p[0]) + 0.6 * f32::from(p[1]) + 0.15 * f32::from(p[2])
}

/// The mean of `img`'s luma over a box of normalized coordinates.
fn mean(img: &Rgba8, (u0, v0, u1, v1): (f32, f32, f32, f32)) -> f32 {
    let (w, h) = (img.width as f32, img.height as f32);
    let (x0, x1, y0, y1) = ((u0 * w) as usize, (u1 * w) as usize, (v0 * h) as usize, (v1 * h) as usize);
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += luma(img.get(x, y));
        }
    }
    sum / ((x1 - x0) * (y1 - y0)).max(1) as f32
}

#[test]
fn no_light_or_no_field_changes_nothing_and_the_near_square_catches_it() {
    let src = scene(W, H);
    let plain = render(&src, &SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(W, H)).image;
    assert_eq!(render(&src, &info(), &lit(0.0), &RenderRequest::fit(W, H)).image.data, plain.data);
    assert_eq!(render(&src, &SourceInfo::default(), &lit(80.0), &RenderRequest::fit(W, H)).image.data, plain.data);
    let mut eye_off = lit(80.0);
    eye_off.set_section_enabled("relight", false);
    assert_eq!(render(&src, &info(), &eye_off, &RenderRequest::fit(W, H)).image.data, plain.data);
    let on = render(&src, &info(), &lit(80.0), &RenderRequest::fit(W, H)).image;
    let square = (0.25, 0.35, 0.4, 0.6);
    let wall = (0.7, 0.35, 0.95, 0.6);
    let (gain_square, gain_wall) = (mean(&on, square) - mean(&plain, square), mean(&on, wall) - mean(&plain, wall));
    assert!(gain_square > 25.0, "the lit square brightens: {gain_square}");
    assert!(gain_square > 4.0 * gain_wall.max(0.0), "the far wall falls off: square {gain_square}, wall {gain_wall}");
}

#[test]
fn a_draft_and_an_export_make_the_same_light() {
    let src = scene(W, H);
    let info = info();
    let mut s = lit(80.0);
    s.relight.warmth = 60.0;
    s.relight.darken = 30.0;
    let check = |s: &DevelopSettings| {
        let small = render(&src, &info, s, &RenderRequest::fit(W / 4, H / 4)).image;
        let big = render(&src, &info, s, &RenderRequest::fit(W, H)).image;
        let (sw, sh) = (small.width, small.height);
        let mut worst = 0f32;
        let mut total = 0f32;
        for y in 1..sh - 1 {
            for x in 1..sw - 1 {
                // the export's 4 × 4 block under the draft's pixel
                let mut acc = [0f32; 3];
                for dy in 0..4 {
                    for dx in 0..4 {
                        let p = big.get((x * big.width / sw + dx).min(big.width - 1), (y * big.height / sh + dy).min(big.height - 1));
                        (0..3).for_each(|c| acc[c] += f32::from(p[c]) / 16.0);
                    }
                }
                let p = small.get(x, y);
                let d = (0..3).map(|c| (acc[c] - f32::from(p[c])).abs()).fold(0f32, f32::max);
                worst = worst.max(d);
                total += d;
            }
        }
        (total / ((sw - 2) * (sh - 2)) as f32, worst)
    };
    let (mean_d, _) = check(&s);
    assert!(mean_d < 2.0, "mean difference {mean_d}");
    // with a crop: the light stays on the same place of the photo
    let uncropped = s.clone();
    s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.1, y0: 0.1, x1: 0.8, y1: 0.9 };
    let (mean_d, _) = check(&s);
    assert!(mean_d < 2.0, "cropped: mean difference {mean_d}");
    let full = render(&src, &info, &uncropped, &RenderRequest::fit(W, H)).image;
    let cropped = render(&src, &info, &s, &RenderRequest::fit(W, H)).image;
    // the square's middle (photo 0.3, 0.45) is at (0.2 / 0.7, 0.35 / 0.8) of the crop
    let at_full = luma(full.get((0.3 * W as f32) as usize, (0.45 * H as f32) as usize));
    let at_crop = luma(cropped.get((0.2 / 0.7 * cropped.width as f32) as usize, (0.35 / 0.8 * cropped.height as f32) as usize));
    assert!((at_crop - at_full).abs() < 6.0, "{at_full} {at_crop}");
}
