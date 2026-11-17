//! Relight through the whole render (fork, issue #288): Amount 0 or no field changes nothing, the light brightens
//! what is near it, and a draft and an export four times its size make the same light, with a crop too.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight::LightSettings;
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
    s.relight.lights[0] = LightSettings { amount, x: 0.3, y: 0.45, depth: 10.0, ..LightSettings::default() };
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
    s.relight.lights[0].warmth = 60.0;
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

/// The mean colour (8-bit, each channel) of `img` over the normalized box.
fn mean_rgb(img: &Rgba8, (u0, v0, u1, v1): (f32, f32, f32, f32)) -> [f32; 3] {
    let (w, h) = (img.width as f32, img.height as f32);
    let (x0, x1, y0, y1) = ((u0 * w) as usize, (u1 * w) as usize, (v0 * h) as usize, (v1 * h) as usize);
    let mut sum = [0f32; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get(x, y);
            (0..3).for_each(|c| sum[c] += f32::from(p[c]));
        }
    }
    let n = ((x1 - x0) * (y1 - y0)).max(1) as f32;
    sum.map(|v| v / n)
}

#[test]
fn a_warm_light_warms_a_neutral_wall_and_a_cool_one_cools_it() {
    // a flat grey wall (a few shades of grey, none with a colour) lit from its middle
    let src = Rgb32f::from_fn(W, H, |x, _| [0.03 + 0.12 * x as f32 / W as f32; 3]);
    let wall = {
        let (fw, fh) = (240, 160);
        let field = Field::from_parts(fw, fh, vec![0.0; fw * fh], vec![-2.0; fw * fh]).unwrap();
        SourceInfo { relight: Some(relight::Input { field: Arc::new(field), key: 9 }), ..SourceInfo::default() }
    };
    for warmth in [-100.0, -67.0, 67.0, 100.0] {
        let mut s = DevelopSettings::default();
        s.relight.lights[0] = LightSettings { amount: 48.0, softness: 84.0, wrap: 28.0, x: 0.5, y: 0.5, depth: 10.0, warmth };
        let img = render(&src, &wall, &s, &RenderRequest::fit(W, H)).image;
        for (name, b) in [("left", (0.1, 0.4, 0.2, 0.6)), ("middle", (0.45, 0.4, 0.55, 0.6)), ("right", (0.8, 0.4, 0.9, 0.6))] {
            let [r, g, b] = mean_rgb(&img, b);
            // (the finish stage's temperature may not take a channel to nothing, and keeps the order of the light's colour)
            assert!(r.min(g).min(b) > 20.0, "warmth {warmth} {name}: {r} {g} {b}");
            if warmth > 0.0 {
                assert!(r > g + 8.0 && g > b + 8.0, "warmth {warmth} {name} is not warm: {r} {g} {b}");
            } else {
                assert!(b > g + 5.0 && g > r + 5.0, "warmth {warmth} {name} is not cool: {r} {g} {b}");
            }
        }
    }
}

/// A flat grey wall field, lit by `lights` (place; Warmth), each at Amount 60.
fn wall_with(lights: &[(f64, f64)]) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.relight.lights =
        lights.iter().map(|&(x, warmth)| LightSettings { amount: 60.0, softness: 30.0, wrap: 28.0, x, y: 0.5, depth: 10.0, warmth }).collect();
    s
}

fn wall() -> (Rgb32f, SourceInfo) {
    let src = Rgb32f::from_fn(W, H, |_, _| [0.06; 3]);
    let (fw, fh) = (240, 160);
    let field = Field::from_parts(fw, fh, vec![0.0; fw * fh], vec![-2.0; fw * fh]).unwrap();
    (src, SourceInfo { relight: Some(relight::Input { field: Arc::new(field), key: 21 }), ..SourceInfo::default() })
}

#[test]
fn a_block_saved_with_one_light_renders_as_that_light() {
    // (the shape before issue #299: one light's numbers and Darken Surroundings in one object)
    let (src, info) = (scene(W, H), info());
    let old = serde_json::json!({"fork_relight": {"amount": 70.0, "softness": 35.0, "warmth": 40.0, "x": 0.3, "y": 0.45, "depth": 10.0, "darken": 25.0, "wrap": 30.0}});
    let read = DevelopSettings { relight: DevelopSettings::from_json(&old).unwrap().relight, ..DevelopSettings::default() };
    let mut built = DevelopSettings::default();
    built.relight.lights[0] = LightSettings { amount: 70.0, softness: 35.0, warmth: 40.0, x: 0.3, y: 0.45, depth: 10.0, wrap: 30.0 };
    built.relight.darken = 25.0;
    let req = RenderRequest::fit(W, H);
    let (a, b) = (render(&src, &info, &read, &req).image, render(&src, &info, &built, &req).image);
    assert!(a.data == b.data, "the old block and the one light built by hand render differently");
    assert!(a.data != render(&src, &SourceInfo::default(), &read, &req).image.data, "the light does something");
    // and saved again it reads back to the same picture
    let again = DevelopSettings::from_json(&read.to_json()).unwrap();
    assert!(render(&src, &info, &again, &req).image.data == a.data, "saved and read again, the light renders differently");
}

#[test]
fn two_lights_add_up_and_a_warm_and_a_cool_one_each_colour_their_side() {
    let (src, info) = wall();
    let req = RenderRequest::fit(W, H);
    let alone = |x: f64, w: f64| render(&src, &info, &wall_with(&[(x, w)]), &req).image;
    let (left, right) = (alone(0.2, 100.0), alone(0.8, -100.0));
    let both = render(&src, &info, &wall_with(&[(0.2, 100.0), (0.8, -100.0)]), &req).image;
    let plain = render(&src, &SourceInfo::default(), &DevelopSettings::default(), &req).image;
    // between the two lights, both together are brighter than either alone
    let middle = (0.45, 0.4, 0.55, 0.6);
    let (b, l, r, p) = (mean(&both, middle), mean(&left, middle), mean(&right, middle), mean(&plain, middle));
    assert!(b > l.max(r) + 2.0 && l > p && r > p, "both {b}, left {l}, right {r}, plain {p}");
    // the warm light's side is warm and the cool one's cool, as when each is alone
    let (near_warm, near_cool) = ((0.15, 0.4, 0.25, 0.6), (0.75, 0.4, 0.85, 0.6));
    let [wr, _, wb] = mean_rgb(&both, near_warm);
    let [cr, _, cb] = mean_rgb(&both, near_cool);
    assert!(wr > wb + 6.0, "near the warm light: {wr} {wb}");
    assert!(cb > cr + 3.0, "near the cool light: {cr} {cb}");
    // a light with no Amount adds nothing: the picture is the other light's
    let mut dead = wall_with(&[(0.2, 100.0), (0.8, -100.0)]);
    dead.relight.lights[1].amount = 0.0;
    assert!(render(&src, &info, &dead, &req).image.data == left.data, "a light with no Amount changed the picture");
    // eight lights on one spot are brighter than one, and the grey wall stays a neutral grey
    let (one, eight) = (alone(0.5, 0.0), render(&src, &info, &wall_with(&[(0.5, 0.0); 8]), &req).image);
    let [r, g, b] = mean_rgb(&eight, (0.45, 0.4, 0.55, 0.6));
    assert!(mean(&eight, (0.45, 0.4, 0.55, 0.6)) > mean(&one, (0.45, 0.4, 0.55, 0.6)) && (r - g).abs() < 2.0 && (g - b).abs() < 2.0, "{r} {g} {b}");
}
