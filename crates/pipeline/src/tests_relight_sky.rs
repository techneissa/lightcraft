//! A sun's rays in the sky through the whole render (fork, issue #330): Blocking cuts them below thick cloud, nothing on a
//! photo without a sky, rays in the air in front of far land and none over near ground, no sky clipped to white, a draft and
//! an export four times its size make the same rays, and a drag of a ray's shape traces nothing again.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight::LightSettings;
use lightcraft_raster::{Rgb32f, Rgba8};
use lightcraft_relight::Field;

use crate::{RenderRequest, SourceInfo, relight, relight_sky, render};

const W: usize = 480;
const H: usize = 320;
const BLUE: [f32; 3] = [0.25, 0.32, 0.55];
/// A sky near white, with little headroom.
const BRIGHT: [f32; 3] = [0.4, 0.48, 0.75];
const GREY: [f32; 3] = [0.06, 0.06, 0.065];
const GREEN: [f32; 3] = [0.06, 0.14, 0.03];

/// A bright hazy sky over the top 60 % (the sun ahead), a band of thick cloud in it, dark against it, open at 40–55 % across,
/// green land below.
fn colour(u: f32, v: f32, blue: [f32; 3]) -> [f32; 3] {
    if v >= 0.6 {
        GREEN
    } else if (0.15..0.28).contains(&v) && !(0.4..0.55).contains(&u) {
        GREY
    } else {
        blue
    }
}

/// The sky at the far end; the land from far hills at the horizon (nearly as far) to near ground at the bottom.
fn depth(v: f32) -> f32 {
    if v < 0.6 { 1.5 } else { 1.4 - 6.0 * (v - 0.6) }
}

fn photo(w: usize, h: usize, sky: bool) -> Rgb32f {
    photo_of(w, h, sky, BLUE)
}

fn photo_of(w: usize, h: usize, sky: bool, blue: [f32; 3]) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| if sky { colour(x as f32 / w as f32, y as f32 / h as f32, blue) } else { GREEN })
}

fn info(sky: bool) -> SourceInfo {
    info_of(sky, BLUE)
}

fn info_of(sky: bool, blue: [f32; 3]) -> SourceInfo {
    let (fw, fh) = (240, 160);
    let px = |i: usize| (((i % fw) as f32 + 0.5) / fw as f32, ((i / fw) as f32 + 0.5) / fh as f32);
    // (the guide as the engine makes it: the photo rendered as by default, 8-bit sRGB brought back to linear Rec.2020)
    let shown = render(&photo_of(fw, fh, sky, blue), &SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(fw, fh)).image;
    let m = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020);
    let rgb: Vec<[f32; 3]> = shown.to_linear().data.iter().map(|p| m.apply_f32(*p)).collect();
    assert_eq!(rgb.len(), fw * fh);
    let lz = (0..fw * fh).map(|i| if sky { depth(px(i).1) } else { -0.3 * px(i).1 }).collect();
    let lum = rgb.iter().map(|c| ((0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]) / 0.18).log2()).collect();
    let field = Field::from_parts(fw, fh, lz, lum).unwrap().with_colour(&rgb).unwrap();
    SourceInfo {
        relight: Some(relight::Input { field: Arc::new(field), key: 330 + u64::from(sky) + 2 * u64::from(blue[0].to_bits()) }),
        ..SourceInfo::default()
    }
}

/// A faint sun above the frame over the gap, with Sky Rays `rays` and the default shape but for `more`.
fn sun_with(rays: f64, more: &[(&str, f64)]) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.relight.lights[0] = LightSettings { amount: 1.0, x: 0.47, y: 0.05, ..LightSettings::default() };
    let base = [("relight.kind", 2.0), ("relight.skyRays", rays), ("relight.raysPlaced", 1.0), ("relight.raysX", 0.475), ("relight.raysY", -0.2)];
    for (id, v) in base.iter().chain(more) {
        assert!(lightcraft_develop::relight::set(&mut s, id, *v), "{id}");
    }
    s
}

/// [`sun_with`] a fan so dense it is nearly even all round, fully blocked: what the clouds cut shows plainly.
fn sun(rays: f64) -> DevelopSettings {
    sun_with(rays, &EVEN)
}

const EVEN: [(&str, f64); 6] = [
    ("relight.rayCount", 60.0),
    ("relight.rayWidth", 100.0),
    ("relight.raySoftness", 100.0),
    ("relight.raySpread", 360.0),
    ("relight.rayVariation", 0.0),
    ("relight.rayBlocking", 100.0),
];

fn luma(p: [u8; 4]) -> f32 {
    0.25 * f32::from(p[0]) + 0.6 * f32::from(p[1]) + 0.15 * f32::from(p[2])
}

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
fn sky_rays_at_full_blocking_brighten_the_air_below_a_gap_and_change_no_byte_without_a_sky() {
    let req = RenderRequest::fit(W, H);
    let (src, info) = (photo(W, H, true), info(true));
    let (off, on) = (render(&src, &info, &sun(0.0), &req).image, render(&src, &info, &sun(100.0), &req).image);
    let (below_gap, below_cloud) = ((0.44, 0.35, 0.51, 0.5), (0.1, 0.35, 0.2, 0.5));
    let gain = mean(&on, below_gap) - mean(&off, below_gap);
    assert!(gain > 6.0, "no ray below the gap: {gain}");
    assert!(mean(&on, below_cloud) - mean(&off, below_cloud) < 0.25 * gain, "a ray below thick cloud");
    // Sky Rays 0 changes no byte, whatever the shape and centre
    let mut bare = sun_with(0.0, &[]);
    bare.relight.lights[0].effects = Default::default();
    assert_eq!(off.data, render(&src, &info, &bare, &req).image.data);
    // a photo of near green leaves: no sky, and Sky Rays change no byte
    let (src, info) = (photo(W, H, false), self::info(false));
    assert!(!info.relight.as_ref().unwrap().field.has_sky());
    assert_eq!(render(&src, &info, &sun(100.0), &req).image.data, render(&src, &info, &sun(0.0), &req).image.data);
}

#[test]
fn a_draft_and_an_export_make_the_same_rays() {
    // what the rays change, in a draft and in an export four times its size (the photo's own hard cloud edges alias alike
    // with and without them, so the change is compared, not the pictures)
    let (src, info) = (photo(W, H, true), info(true));
    let at = |rays: f64, w: usize, h: usize| render(&src, &info, &sun(rays), &RenderRequest::fit(w, h)).image;
    let (small_on, small_off) = (at(100.0, W / 4, H / 4), at(0.0, W / 4, H / 4));
    let (big_on, big_off) = (at(100.0, W, H), at(0.0, W, H));
    let (sw, sh) = (small_on.width, small_on.height);
    let (mut total, mut moved) = (0f32, 0f32);
    for y in 1..sh - 1 {
        for x in 1..sw - 1 {
            let mut acc = [0f32; 3];
            for dy in 0..4 {
                for dx in 0..4 {
                    let (bx, by) = ((x * big_on.width / sw + dx).min(big_on.width - 1), (y * big_on.height / sh + dy).min(big_on.height - 1));
                    let (on, off) = (big_on.get(bx, by), big_off.get(bx, by));
                    (0..3).for_each(|c| acc[c] += (f32::from(on[c]) - f32::from(off[c])) / 16.0);
                }
            }
            let (on, off) = (small_on.get(x, y), small_off.get(x, y));
            let d = (0..3).map(|c| (acc[c] - (f32::from(on[c]) - f32::from(off[c]))).abs()).fold(0f32, f32::max);
            total += d;
            moved += acc.iter().map(|v| v.abs()).fold(0f32, f32::max);
        }
    }
    let n = ((sw - 2) * (sh - 2)) as f32;
    assert!(moved / n > 1.0, "the rays change too little to compare: {}", moved / n);
    assert!(total < 0.12 * moved, "mean difference of the change {} against a change of {}", total / n, moved / n);
}

#[test]
fn rays_at_50_light_the_air_in_front_of_far_land_and_leave_near_ground_alone() {
    // (issue #330, round 2) the beam through the gap runs down through the air in front of the far hills: there it stands out
    // against the hills under thick cloud; near ground has no air in front of it and keeps every byte
    let req = RenderRequest::fit(W, H);
    let (src, info) = (photo(W, H, true), info(true));
    let (off, on) = (render(&src, &info, &sun(0.0), &req).image, render(&src, &info, &sun(50.0), &req).image);
    let (under_gap, under_cloud) = ((0.44, 0.61, 0.51, 0.66), (0.08, 0.61, 0.18, 0.66));
    let gain = |r| mean(&on, r) - mean(&off, r);
    let (lit, held) = (gain(under_gap), gain(under_cloud));
    assert!(lit - held > 3.0, "no shaft over the far land: {lit} under the gap against {held} under the cloud");
    let (w, h) = (on.width, on.height);
    let near = (h * 85 / 100..h).flat_map(|y| (0..w).map(move |x| (x, y)));
    let worst = near.map(|(x, y)| (0..3).map(|c| on.get(x, y)[c].abs_diff(off.get(x, y)[c])).max().unwrap_or(0)).max().unwrap_or(0);
    assert!(worst <= 1, "near ground changed by {worst}");
}

#[test]
fn sky_rays_100_never_clip_a_sky_that_was_not_white() {
    // (issue #330, round 3) a bright ray fills a share of the headroom left above the sky: it rolls off towards white, also
    // over a sky already near white, at Blocking 100 and at 0 (more light comes through)
    for blocking in [100.0, 0.0] {
        clips_none(blocking);
    }
}

fn clips_none(blocking: f64) {
    let req = RenderRequest::fit(W, H);
    let (src, info) = (photo_of(W, H, true, BRIGHT), info_of(true, BRIGHT));
    let mut even = EVEN;
    even[5].1 = blocking;
    let (off, on) = (render(&src, &info, &sun(0.0), &req).image, render(&src, &info, &sun_with(100.0, &even), &req).image);
    let sky_rows = 0..on.height * 6 / 10;
    let (mut lifted, mut clipped) = (0usize, 0usize);
    for y in sky_rows {
        for x in 0..on.width {
            let (a, b) = (off.get(x, y), on.get(x, y));
            let (ma, mb) = (a[..3].iter().copied().max().unwrap_or(0), b[..3].iter().copied().max().unwrap_or(0));
            lifted += usize::from(mb > ma + 3);
            clipped += usize::from(ma < 250 && mb == u8::MAX);
        }
    }
    assert!(lifted > 40, "the rays lift too little of the sky to tell at Blocking {blocking}: {lifted}");
    assert_eq!(clipped, 0, "{clipped} sky pixels clipped to white at Blocking {blocking}");
}

#[test]
fn a_drag_of_a_rays_shape_reuses_the_traced_blocking_and_a_sun_move_traces_again() {
    // what cuts the rays is traced once per sun position; every shape slider only redraws the fan over it
    let info = info_of(true, [0.25, 0.33, 0.55]);
    let input = info.relight.as_ref().unwrap();
    let (fields, shaped) = (crate::relight::Memo::new(2), crate::relight::Memo::new(4));
    let mut s = sun_with(80.0, &[]);
    let room = |s: &DevelopSettings| {
        let tone = crate::finish::base_tone(s, &info, 1.0);
        let guide = crate::finish::base_tone(&DevelopSettings::default(), &info, 1.0);
        relight_sky::Headroom::of(&tone, &guide, s.light.exposure)
    };
    let rays = |s: &DevelopSettings| {
        let sky = relight_sky::SkyLight::of(&s.relight.lights[0].effects);
        let centre = sky.at.map(|(x, y)| (x as f32, y as f32)).unwrap();
        relight_sky::shaped_for((&fields, &shaped), input, centre, &room(s), &sky.shape).unwrap()
    };
    let before = relight_sky::traces();
    let mut last = rays(&s);
    assert_eq!(relight_sky::traces(), before + 1, "the first rays trace once");
    let drags = [
        ("relight.0.rayWidth", 70.0),
        ("relight.0.rayCount", 30.0),
        ("relight.0.raySpread", 220.0),
        ("relight.0.rayDirection", 25.0),
        ("relight.0.rayLength", 30.0),
        ("relight.0.raySoftness", 10.0),
        ("relight.0.rayVariation", 90.0),
        ("relight.0.rayBlocking", 90.0),
        ("relight.0.skyRays", 40.0),
    ];
    for (id, v) in drags {
        assert!(lightcraft_develop::relight::set(&mut s, id, v), "{id}");
        let now = rays(&s);
        assert_eq!(relight_sky::traces(), before + 1, "a drag of {id} traced again");
        if id != "relight.0.skyRays" {
            assert!(!Arc::ptr_eq(&now, &last), "a drag of {id} kept the old rays");
        }
        last = now;
    }
    assert!(lightcraft_develop::relight::set(&mut s, "relight.0.raysX", 0.3));
    rays(&s);
    assert_eq!(relight_sky::traces(), before + 2, "a move of the sun didn't trace");
}

/// An open sky of colour `sky` (linear Rec.2020) over the top 60 %, green land below, with the field the engine would make.
fn open_sky(sky: [f32; 3]) -> (Rgb32f, SourceInfo) {
    let (fw, fh) = (240, 160);
    let paint = |w: usize, h: usize| Rgb32f::from_fn(w, h, |_, y| if (y as f32 / h as f32) < 0.6 { sky } else { GREEN });
    let shown = render(&paint(fw, fh), &SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(fw, fh)).image;
    let m = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020);
    let rgb: Vec<[f32; 3]> = shown.to_linear().data.iter().map(|p| m.apply_f32(*p)).collect();
    let lz = (0..fw * fh).map(|i| depth(((i / fw) as f32 + 0.5) / fh as f32)).collect();
    let lum = rgb.iter().map(|c| ((0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(1e-6) / 0.18).log2()).collect();
    let field = Field::from_parts(fw, fh, lz, lum).unwrap().with_colour(&rgb).unwrap();
    let key = 3301 + u64::from(sky[0].to_bits());
    (paint(W, H), SourceInfo { relight: Some(relight::Input { field: Arc::new(field), key }), ..SourceInfo::default() })
}

#[test]
fn sky_rays_100_show_plain_shafts_on_a_near_white_sky_and_a_mid_blue_one_without_clipping() {
    // (issue #330, round 6) with little headroom the gaps between shafts darken, with plenty the shafts brighten: either way
    // the shafts stand out by several percent at Sky Rays 100, and nothing clips
    for (name, sky) in [("near-white", [0.62, 0.66, 0.72]), ("mid-blue", [0.1, 0.16, 0.38])] {
        let (src, info) = open_sky(sky);
        let req = RenderRequest::fit(W, H);
        let at = |rays: f64| {
            let mut s = sun_with(rays, &[]);
            assert!(lightcraft_develop::relight::set(&mut s, "relight.raysY", -0.05));
            render(&src, &info, &s, &req).image
        };
        let (off, on) = (at(0.0), at(100.0));
        let mut ratios = Vec::new();
        let mut clipped = 0usize;
        for y in on.height * 15 / 100..on.height / 2 {
            for x in 0..on.width {
                let (a, b) = (off.get(x, y), on.get(x, y));
                ratios.push((luma(b) + 1.0) / (luma(a) + 1.0));
                let (ma, mb) = (a[..3].iter().copied().max().unwrap_or(0), b[..3].iter().copied().max().unwrap_or(0));
                clipped += usize::from(ma < 250 && mb == u8::MAX);
            }
        }
        ratios.sort_by(f32::total_cmp);
        let q = |p: f32| ratios[((ratios.len() - 1) as f32 * p) as usize];
        let contrast = q(0.95) / q(0.05) - 1.0;
        assert!(contrast > 0.06, "the shafts barely show on the {name} sky: {contrast:.3}");
        assert_eq!(clipped, 0, "{clipped} pixels of the {name} sky clipped to white");
    }
}
