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
        s.relight.lights[0] =
            LightSettings { amount: 48.0, softness: 84.0, wrap: 28.0, x: 0.5, y: 0.5, depth: 10.0, warmth, ..LightSettings::default() };
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
    s.relight.lights = lights
        .iter()
        .map(|&(x, warmth)| LightSettings { amount: 60.0, softness: 30.0, wrap: 28.0, x, y: 0.5, depth: 10.0, warmth, ..LightSettings::default() })
        .collect();
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
    built.relight.lights[0] =
        LightSettings { amount: 70.0, softness: 35.0, warmth: 40.0, x: 0.3, y: 0.45, depth: 10.0, wrap: 30.0, ..LightSettings::default() };
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

#[test]
fn each_light_type_goes_through_the_render() {
    let (src, info) = (scene(W, H), info());
    let render_of = |s: &DevelopSettings| render(&src, &info, s, &RenderRequest::fit(W, H)).image;
    let point = render_of(&lit(60.0));
    // a spot aimed at the wall to the right of the square lights it, and leaves the left of the photo as it was
    let mut spot = lit(100.0);
    let l = &mut spot.relight.lights[0];
    (l.x, l.y, l.depth) = (0.55, 0.5, 5.0);
    for (id, v) in [("relight.kind", 1.0), ("relight.spotAimX", 0.85), ("relight.spotAimY", 0.5), ("relight.spotAngle", 14.0)] {
        assert!(lightcraft_develop::relight::set(&mut spot, id, v));
    }
    let off = render_of(&DevelopSettings::default());
    let spot = render_of(&spot);
    let (lit_box, dark_box) = ((0.78, 0.4, 0.92, 0.6), (0.02, 0.8, 0.12, 0.95));
    assert!(mean(&spot, lit_box) > mean(&off, lit_box) + 20.0, "{} {}", mean(&spot, lit_box), mean(&off, lit_box));
    assert!((mean(&spot, dark_box) - mean(&off, dark_box)).abs() < 2.0);
    // and every other type changes the photo, none like the point (the settings read back as saved)
    for kind in [2.0, 3.0, 4.0, 5.0] {
        let mut s = lit(100.0);
        assert!(lightcraft_develop::relight::set(&mut s, "relight.kind", kind));
        let back = DevelopSettings::from_json(&s.to_json()).unwrap();
        assert_eq!(back.relight, s.relight);
        let img = render_of(&s);
        assert!(img.data != off.data && img.data != point.data, "kind {kind}");
    }
}

/// Light 0 of the wall with the effects `f` set on it (Amount 70, in the middle).
fn effect(f: impl FnOnce(&mut lightcraft_develop::relight_effects::LightEffects)) -> DevelopSettings {
    let mut s = wall_with(&[(0.5, 0.0)]);
    s.relight.lights[0].amount = 70.0;
    f(&mut s.relight.lights[0].effects);
    s
}

#[test]
fn a_red_gel_reddens_a_grey_wall_and_a_wall_without_one_is_as_it_was() {
    let (src, info) = wall();
    let req = RenderRequest::fit(W, H);
    let plain = render(&src, &info, &effect(|_| {}), &req).image;
    let red = render(&src, &info, &effect(|e| (e.gel_hue, e.gel_strength) = (0.0, 100.0)), &req).image;
    let [r, g, b] = mean_rgb(&red, (0.4, 0.4, 0.6, 0.6));
    assert!(red.data.iter().all(|p| p[3] == 255) && r > 2.0 * g.max(b), "{r} {g} {b}");
    let [pr, pg, pb] = mean_rgb(&plain, (0.4, 0.4, 0.6, 0.6));
    assert!((pr - pg).abs() < 2.0 && (pg - pb).abs() < 2.0, "{pr} {pg} {pb}");
    // a gel at no strength is no gel: the same picture, to the byte
    let none = render(&src, &info, &effect(|e| (e.gel_hue, e.gel_strength) = (200.0, 0.0)), &req).image;
    assert!(none.data == plain.data);
    // teal and magenta go to their sides
    let teal = mean_rgb(&render(&src, &info, &effect(|e| (e.gel_hue, e.gel_strength) = (178.0, 80.0)), &req).image, (0.4, 0.4, 0.6, 0.6));
    assert!(teal[0] < teal[1] && teal[0] < teal[2], "{teal:?}");
    let magenta = mean_rgb(&render(&src, &info, &effect(|e| (e.gel_hue, e.gel_strength) = (305.0, 80.0)), &req).image, (0.4, 0.4, 0.6, 0.6));
    assert!(magenta[1] < magenta[0] && magenta[1] < magenta[2], "{magenta:?}");
}

#[test]
fn a_pattern_stripes_the_wall_and_haze_brightens_around_the_light() {
    let (photo, field) = (scene(W, H), info());
    let (src, info) = wall();
    let req = RenderRequest::fit(W, H);
    let plain = render(&src, &info, &effect(|_| {}), &req).image;
    let blinds = render(&src, &info, &effect(|e| (e.pattern, e.pattern_scale, e.pattern_softness) = (1.0, 60.0, 10.0)), &req).image;
    // down a column the light goes on and off (the wall under no light is about 70)
    let col: Vec<f32> = (0..H).map(|y| luma(blinds.get(W / 2, y))).collect();
    let flips = col.windows(2).filter(|p| (p[0] > 85.0) != (p[1] > 85.0)).count();
    assert!(flips >= 4, "{flips} edges");
    assert!(blinds.data != plain.data);
    // haze adds light, more over the far wall than over the near square at the same distance from the light (a wall
    // with no depth to it has no haze: nothing is farther than anything)
    let (plain, hazy) = (render(&photo, &field, &effect(|_| {}), &req).image, render(&photo, &field, &effect(|e| e.haze = 100.0), &req).image);
    let (far, near) = ((0.65, 0.4, 0.75, 0.6), (0.25, 0.4, 0.35, 0.6));
    assert!(mean(&hazy, far) > mean(&plain, far) + 1.0, "{} {}", mean(&hazy, far), mean(&plain, far));
    assert!(mean(&hazy, far) - mean(&plain, far) > mean(&hazy, near) - mean(&plain, near), "haze is no stronger over what is far");
}

#[test]
fn the_same_effect_on_another_light_is_another_scene() {
    // two lights, a red gel on the first or on the second: the shading key names which, and the render puts the colour
    // on the right side
    let (src, info) = wall();
    let req = RenderRequest::fit(W, H);
    let with_gel_on = |i: usize| {
        let mut s = wall_with(&[(0.2, 0.0), (0.8, 0.0)]);
        (s.relight.lights[i].effects.gel_hue, s.relight.lights[i].effects.gel_strength) = (0.0, 100.0);
        s
    };
    let (first, second) = (with_gel_on(0), with_gel_on(1));
    let input = info.relight.as_ref().unwrap();
    let key = |s: &DevelopSettings| relight::shading_key(input, &relight::scene_of(s).unwrap(), 0.0, false);
    assert_ne!(key(&first), key(&second), "the key doesn't say which light has the gel");
    // (and shading_for, asked in turn, gives each its own)
    for _ in 0..2 {
        for s in [&first, &second] {
            let (k, _) = relight::shading_for(input, &relight::scene_of(s).unwrap(), 0.0, false);
            assert_eq!(k, key(s));
        }
    }
    let redness = |img: &Rgba8, b: (f32, f32, f32, f32)| {
        let [r, g, _] = mean_rgb(img, b);
        r / g.max(1.0)
    };
    let (left, right) = ((0.1, 0.4, 0.3, 0.6), (0.7, 0.4, 0.9, 0.6));
    let a = render(&src, &info, &first, &req).image;
    assert!(redness(&a, left) > 1.5 * redness(&a, right), "the gel on the first light isn't on the left");
    let b = render(&src, &info, &second, &req).image;
    assert!(redness(&b, right) > 1.5 * redness(&b, left), "the gel on the second light isn't on the right");
}

#[test]
fn shadows_darken_what_a_near_thing_hides_from_the_light_and_no_amount_changes_no_byte() {
    // a sun from above and a little to the left, in front: the wall below the near square is in its shadow, the wall beside it
    // is not
    let (src, info) = (scene(W, H), info());
    let req = RenderRequest::fit(W, H);
    let side = |shadows: f64| {
        let mut s = DevelopSettings::default();
        s.relight.lights[0] = LightSettings { amount: 100.0, softness: 20.0, wrap: 100.0, ..LightSettings::default() };
        for (field, v) in [("kind", 2.0), ("sunAzimuth", -25.0), ("sunElevation", 40.0), ("shadows", shadows)] {
            assert!(lightcraft_develop::relight::set(&mut s, &format!("relight.{field}"), v));
        }
        s
    };
    let (plain, shadowed) = (render(&src, &info, &side(0.0), &req).image, render(&src, &info, &side(100.0), &req).image);
    // an edit saved before shadows (a block with no `shadows` key) reads as Shadows 0 and renders to the byte as Shadows 0 does:
    // the shadow code is not run for it, and the kept lobe has the key it had
    let mut json = serde_json::to_value(&side(0.0).relight).unwrap();
    assert!(json["lights"][0].as_object_mut().unwrap().remove("shadows").is_none(), "Shadows 0 is not saved");
    let mut old = side(0.0);
    old.relight = DevelopSettings::from_json(&serde_json::json!({ "fork_relight": json })).unwrap().relight;
    assert_eq!(old.relight, side(0.0).relight);
    assert_eq!(render(&src, &info, &old, &req).image.data, plain.data);
    let (behind, beside) = ((0.45, 0.78, 0.6, 0.9), (0.8, 0.4, 0.95, 0.6));
    assert!(
        mean(&shadowed, behind) < mean(&plain, behind) - 3.0,
        "below the square: {} lit, {} shadowed",
        mean(&plain, behind),
        mean(&shadowed, behind)
    );
    assert!(
        (mean(&shadowed, beside) - mean(&plain, beside)).abs() < 1.5,
        "beside it: {} lit, {} shadowed",
        mean(&plain, beside),
        mean(&shadowed, beside)
    );
    // a rim or a fill takes no shadow: the same render at any amount
    for kind in [4.0, 5.0] {
        let with = |shadows: f64| {
            let mut s = side(shadows);
            assert!(lightcraft_develop::relight::set(&mut s, "relight.kind", kind));
            assert!(lightcraft_develop::relight::set(&mut s, "relight.shadows", shadows));
            render(&src, &info, &s, &req).image.data
        };
        assert_eq!(with(0.0), with(100.0), "kind {kind}");
    }
}

#[test]
fn a_near_thing_blocks_a_pattern_even_at_cast_shadows_0() {
    // a sun through wide-open, soft blinds from above and a little to the left: the wall below the near square is in its
    // shadow even with Cast Shadows at 0, as a person blocks the light through a window (issue #316); it used to be lit
    // through the person
    let (src, info) = (scene(W, H), info());
    let req = RenderRequest::fit(W, H);
    let sun = |shadows: f64, pattern: f64| {
        let mut s = DevelopSettings::default();
        s.relight.lights[0] = LightSettings { amount: 100.0, softness: 20.0, wrap: 100.0, x: 0.6, y: 0.62, ..LightSettings::default() };
        let set = [("kind", 2.0), ("sunAzimuth", -25.0), ("sunElevation", 40.0), ("shadows", shadows), ("pattern", pattern), ("patternOpen", 100.0)];
        for (field, v) in set.into_iter().chain([("patternSoftness", 100.0)]) {
            assert!(lightcraft_develop::relight::set(&mut s, &format!("relight.{field}"), v), "{field}");
        }
        render(&src, &info, &s, &req).image
    };
    let (open, blocked) = (sun(0.0, 1.0), sun(100.0, 1.0));
    assert_eq!(open.data, blocked.data, "Cast Shadows changes a patterned light");
    // against the same sun with no pattern (and no shadows): the blinds take a little off beside the square, and much more
    // behind it, where the square blocks them
    let (behind, beside) = ((0.45, 0.78, 0.6, 0.9), (0.7, 0.4, 0.8, 0.6));
    let bare = sun(0.0, 0.0);
    let drop = |b| mean(&bare, b) - mean(&open, b);
    assert!(drop(behind) > drop(beside) + 3.0, "behind the square {} darker, beside it {}", drop(behind), drop(beside));
}

#[test]
fn a_release_after_a_drag_draws_a_shadowless_lights_pattern_in_full() {
    // a fill (it casts no shadows) through blinds: the drag's render draws the pattern coarse; the release with the same
    // settings must not reuse that drag's shading: it equals a full render made from nothing (it used to reuse it)
    let (_, info) = wall();
    let mut s = effect(|e| (e.pattern, e.pattern_scale, e.pattern_softness) = (1.0, 90.0, 0.0));
    assert!(lightcraft_develop::relight::set(&mut s, "relight.kind", 5.0));
    let scene = relight::scene_of(&s).unwrap();
    let field = info.relight.as_ref().unwrap().field.clone();
    let (dragged, fresh) = (relight::Input { field: field.clone(), key: 316_001 }, relight::Input { field, key: 316_002 });
    let planes =
        |input: &relight::Input, draft: bool| relight::shading_for(input, &scene, 0.0, draft).1.split(W, H, &|x, y| (x / W as f64, y / H as f64)).up;
    let _drag = planes(&dragged, true);
    assert!(planes(&dragged, false) == planes(&fresh, false), "the release reused the drag's coarse pattern");
}

#[test]
fn haze_0_changes_no_byte_whatever_its_scatter_and_dust() {
    // (issue #317) an edit with no haze renders to the byte as before beams, with Forward Scatter and Dust moved too; and those
    // two are not saved at their defaults, so an edit made before them reads as it was
    let (src, info) = (scene(W, H), info());
    let req = RenderRequest::fit(W, H);
    let plain = render(&src, &info, &effect(|_| {}), &req).image;
    let moved = render(&src, &info, &effect(|e| (e.scatter, e.dust) = (90.0, 70.0)), &req).image;
    assert!(moved.data == plain.data, "Forward Scatter and Dust changed a picture with no haze");
    let json = serde_json::to_value(&effect(|_| {}).relight).unwrap();
    assert!(json["lights"][0].get("scatter").is_none() && json["lights"][0].get("dust").is_none(), "{json}");
    // with haze they do change it
    let hazy = |scatter: f64| render(&src, &info, &effect(|e| (e.haze, e.scatter) = (60.0, scatter)), &req).image.data;
    assert!(hazy(10.0) != hazy(90.0) && hazy(50.0) != plain.data);
}

#[test]
fn a_beam_adds_light_in_its_own_colour_over_black() {
    // (issue #317) a black photo, a warm light in front of the wall in haze: the beam is light added on top, so the black
    // round the light brightens and takes the light's warm colour (a gain on black would leave it black)
    let black = Rgb32f::from_fn(W, H, |_, _| [0.0; 3]);
    let info = info();
    let req = RenderRequest::fit(W, H);
    let sun = |haze: f64| {
        let mut s = DevelopSettings::default();
        s.relight.lights[0] = LightSettings { amount: 20.0, warmth: 100.0, x: 0.75, y: 0.35, depth: 70.0, ..LightSettings::default() };
        assert!(lightcraft_develop::relight::set(&mut s, "relight.haze", haze));
        render(&black, &info, &s, &req).image
    };
    let (none, hazy) = (sun(0.0), sun(60.0));
    let wall = (0.55, 0.45, 0.65, 0.6);
    assert!(mean(&none, wall) < 3.0, "{}", mean(&none, wall));
    assert!(mean(&hazy, wall) > mean(&none, wall) + 20.0, "{} {}", mean(&hazy, wall), mean(&none, wall));
    let [r, g, b] = mean_rgb(&hazy, wall);
    assert!(r < 250.0 && r > g && g > b && r > 1.15 * b, "not the light's warm colour: {r} {g} {b}");
}

/// The spread (standard deviation) of `img`'s luma over a box of normalized coordinates.
fn spread(img: &Rgba8, (u0, v0, u1, v1): (f32, f32, f32, f32)) -> f32 {
    let (w, h) = (img.width as f32, img.height as f32);
    let px: Vec<f32> = ((v0 * h) as usize..(v1 * h) as usize)
        .flat_map(|y| ((u0 * w) as usize..(u1 * w) as usize).map(move |x| (x, y)))
        .map(|(x, y)| luma(img.get(x, y)))
        .collect();
    let m = px.iter().sum::<f32>() / px.len().max(1) as f32;
    (px.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / px.len().max(1) as f32).sqrt()
}

#[test]
fn mist_off_changes_no_byte_whatever_its_height_and_fade() {
    // (issue #331) a hazy light with no mist renders to the byte as before mist, with Mist Height and Distance Fade moved
    // (no fade without haze or mist: no air to fade into); and the three are not saved at their defaults
    let (src, info) = (scene(W, H), info());
    let req = RenderRequest::fit(W, H);
    let moved = |e: &mut lightcraft_develop::relight_effects::LightEffects| (e.mist_height, e.fade) = (90.0, 0.0);
    let hazy = render(&src, &info, &effect(|e| e.haze = 60.0), &req).image;
    assert!(render(&src, &info, &effect(|e| (e.haze, e.mist_height) = (60.0, 90.0)), &req).image.data == hazy.data);
    let plain = render(&src, &info, &effect(|_| {}), &req).image;
    assert!(render(&src, &info, &effect(moved), &req).image.data == plain.data);
    assert!(render(&src, &info, &effect(|e| (e.mist_height, e.fade) = (90.0, 100.0)), &req).image.data == plain.data);
    let json = serde_json::to_value(&effect(|e| e.haze = 60.0).relight).unwrap();
    assert!(["mist", "mistHeight", "fade"].iter().all(|k| json["lights"][0].get(k).is_none()), "{json}");
}

#[test]
fn distance_fade_takes_the_far_walls_contrast_more_than_the_near_squares_and_a_small_render_matches_a_large_one() {
    // (issue #331) the textured wall is far, the square near: through the haze the wall's texture fades into the air's colour,
    // the square's much less
    let (src, info) = (scene(W, H), info());
    let req = RenderRequest::fit(W, H);
    // (a faint light: its beam's own gradient would hide the texture; the fade doesn't depend on the light's Amount)
    let haze = |fade: f64| {
        let mut s = effect(|e| (e.haze, e.fade) = (60.0, fade));
        s.relight.lights[0].amount = 1.0;
        render(&src, &info, &s, &req).image
    };
    let (clear, faded) = (haze(0.0), haze(100.0));
    let (wall, square) = ((0.6, 0.1, 0.95, 0.4), (0.25, 0.3, 0.4, 0.65));
    let kept = |b| spread(&faded, b) / spread(&clear, b).max(1e-3);
    assert!(spread(&clear, wall) > 1.0, "the wall has no texture to fade: {}", spread(&clear, wall));
    assert!(kept(wall) < 0.8 && kept(wall) + 0.1 < kept(square), "the wall keeps {} of its contrast, the square {}", kept(wall), kept(square));
    // the fade is the same plane at any size: a render a quarter the size is the full-size one's average
    let s = effect(|e| (e.haze, e.mist, e.fade) = (40.0, 50.0, 100.0));
    let (small, big) = (render(&src, &info, &s, &RenderRequest::fit(W / 4, H / 4)).image, render(&src, &info, &s, &req).image);
    let (a, b) = (mean(&small, wall), mean(&big, wall));
    assert!((a - b).abs() < 2.0, "quarter size {a}, full size {b}");
}

#[test]
fn two_lights_with_distance_fade_dim_far_things_as_one_does() {
    // (issue #331) the fog is the scene's air, not a light's: two identical lights with Distance Fade fade the far wall as
    // much as one of them alone (it used to fade it twice, exp(−2τ), with two glows)
    let (_, info) = (scene(W, H), info());
    let one = effect(|e| (e.haze, e.mist, e.fade) = (40.0, 50.0, 80.0));
    let mut two = one.clone();
    two.relight.lights.push(two.relight.lights[0].clone());
    let input = info.relight.as_ref().unwrap();
    // the air's optical depth in front of the far wall, summed over every fade the render applies
    let tau = |s: &DevelopSettings| {
        let lit = relight::lit(&info, s, false).unwrap();
        lit.fades.iter().map(|f| input.field.plane_at(&f.plane, 1, 1, &|_, _| (0.8, 0.2))[0]).sum::<f32>()
    };
    let (a, b) = (tau(&one), tau(&two));
    assert!(a > 0.05, "the far wall doesn't fade: {a}");
    assert!((a - b).abs() < 1e-6, "one light {a}, two lights {b}");
}

#[test]
fn a_softbox_aim_reaches_the_light_the_shadows_the_pattern_and_the_beam() {
    // (issue #335) the same photo, key and light with and without an aim renders another picture however the light is
    // dressed: each kept lobe, shadow, ray set and beam is keyed by the aim. The unaimed ones are rendered first, so a key
    // that left the aim out would hand the aimed one the kept unaimed picture
    let (src, info) = wall();
    let req = RenderRequest::fit(W, H);
    let boxed = |aim_h: f64, f: &dyn Fn(&mut lightcraft_develop::relight_effects::LightEffects)| {
        let mut s = wall_with(&[(0.5, 0.0)]);
        let l = &mut s.relight.lights[0];
        (l.kind.which, l.kind.box_aim_h) = (3.0, aim_h);
        f(&mut l.effects);
        s
    };
    let dressings: [(&str, &dyn Fn(&mut lightcraft_develop::relight_effects::LightEffects)); 4] =
        [("plain", &|_| {}), ("shadows", &|e| e.shadows = 80.0), ("pattern", &|e| e.pattern = 2.0), ("haze", &|e| e.haze = 60.0)];
    for (name, f) in dressings {
        let straight = render(&src, &info, &boxed(0.0, f), &req).image;
        let aimed = render(&src, &info, &boxed(55.0, f), &req).image;
        assert!(straight.data != aimed.data, "{name}: the aim changed nothing");
    }
    // an unaimed softbox saves no aim, and an aimed one reads back
    let json = serde_json::to_value(&boxed(0.0, &|_| {}).relight).unwrap();
    assert!(json["lights"][0].get("boxAimH").is_none() && json["lights"][0].get("boxAimV").is_none(), "{json}");
    let back = DevelopSettings::from_json(&boxed(55.0, &|_| {}).to_json()).unwrap();
    assert_eq!(back.relight.lights[0].kind.box_aim_h, 55.0);
}

#[test]
fn a_visible_source_is_a_bright_point_in_the_lights_colour_and_the_shading_does_not_know_it() {
    // (issue #334) a black photo and a warm light inside it with no haze: with Visible Source it shows as a bright point of the
    // light's warm colour over the black, with none the photo stays black; the source only adds a plane, so the shading's key
    // is the same
    let black = Rgb32f::from_fn(W, H, |_, _| [0.0; 3]);
    let info = info();
    let req = RenderRequest::fit(W, H);
    let with = |source: f64, x: f64| {
        let mut s = DevelopSettings::default();
        s.relight.lights[0] = LightSettings { amount: 60.0, warmth: 100.0, x, y: 0.35, depth: 70.0, ..LightSettings::default() };
        assert!(lightcraft_develop::relight::set(&mut s, "relight.source", source));
        s
    };
    let (none, shown) = (render(&black, &info, &with(0.0, 0.75), &req).image, render(&black, &info, &with(70.0, 0.75), &req).image);
    let point = (0.72, 0.31, 0.78, 0.39);
    assert!(mean(&none, point) < 3.0, "{}", mean(&none, point));
    assert!(mean(&shown, point) > mean(&none, point) + 40.0, "{} {}", mean(&shown, point), mean(&none, point));
    let [r, g, b] = mean_rgb(&shown, point);
    assert!(r > g && g > b, "not the light's warm colour: {r} {g} {b}");
    // far from the point it stays dark
    assert!(mean(&shown, (0.0, 0.7, 0.2, 0.95)) < 3.0);
    // a Sun shines from a direction, not from its handle's place: it shows no point either
    let sun = |source: f64| {
        let mut s = with(source, 0.75);
        s.relight.lights[0].kind.which = 2.0;
        render(&black, &info, &s, &req).image
    };
    assert!(sun(70.0).data == sun(0.0).data, "a Sun with Visible Source drew a point");
    // a light outside the photo shows no point
    let outside = render(&black, &info, &with(70.0, 1.3), &req).image;
    assert!(outside.data == none.data);
    let input = info.relight.as_ref().unwrap();
    let key = |s: &DevelopSettings| relight::shading_key(input, &relight::scene_of(s).unwrap(), 0.0, false);
    assert_eq!(key(&with(0.0, 0.75)), key(&with(70.0, 0.75)));
}
