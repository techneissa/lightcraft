//! Tests of the detectors and the covering on procedural images.

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::{Rgb32f, Rgba8};

use super::*;
use crate::geometry::Frame;

/// A sky: a smooth vertical gradient, a little bluer towards the top, with `wires` drawn over it. Each wire is
/// `(y at the left, y at the right, sag in px, width in px, how much darker)`.
fn sky(w: usize, h: usize, wires: &[(f64, f64, f64, f64, f32)]) -> Rgba8 {
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            let base = 0.78 - 0.18 * (y / h as f64) as f32;
            let mut dark = 0.0f32;
            for (yl, yr, sag, width, depth) in wires {
                let t = x / w as f64;
                let cy = yl + (yr - yl) * t + sag * 4.0 * t * (1.0 - t);
                let slope = (yr - yl) / w as f64 + sag * 4.0 * (1.0 - 2.0 * t) / w as f64;
                let d = (y - cy).abs() / (1.0 + slope * slope).sqrt();
                dark = dark.max(((width / 2.0 + 0.5 - d).clamp(0.0, 1.0) as f32) * depth);
            }
            let v = base - dark;
            let b = (v * 255.0).round().clamp(0.0, 255.0) as u8;
            [(v * 0.85 * 255.0) as u8, (v * 0.92 * 255.0) as u8, b, 255]
        })
        .collect();
    Rgba8 { width: w, height: h, data }
}

#[test]
fn a_sagging_wire_across_the_sky_is_found() {
    let (w, h) = (1600, 1000);
    let img = sky(w, h, &[(300.0, 380.0, 60.0, 2.5, 0.25)]);
    let found = wires::detect(&img, 50.0);
    assert_eq!(found.len(), 1, "{found:?}");
    let c = &found[0];
    assert_eq!((c.kind, c.source, c.closed, c.main), (Kind::Wire, "lines", false, false));
    // it spans the photo and follows the wire
    assert!(c.bbox[2] - c.bbox[0] > 0.9, "{:?}", c.bbox);
    for p in &c.outline {
        let t = p.0;
        let want = (300.0 + 80.0 * t + 60.0 * 4.0 * t * (1.0 - t)) / h as f64;
        assert!((p.1 - want).abs() < 3.0 / h as f64, "{p:?} vs {want}");
    }
    assert!(!c.spots.is_empty() && c.spots.iter().all(|s| s.size < 0.02), "{:?}", c.spots.iter().map(|s| s.size).collect::<Vec<_>>());
    assert!(c.hit(0.5, (300.0 + 40.0 + 60.0) / h as f64, 0.01) && !c.hit(0.5, 0.9, 0.01));
}

#[test]
fn faint_wires_need_sensitivity() {
    let img = sky(1600, 1000, &[(500.0, 520.0, 20.0, 2.0, 0.03)]);
    assert!(wires::detect(&img, 20.0).is_empty());
    assert_eq!(wires::detect(&img, 100.0).len(), 1);
}

#[test]
fn two_wires_and_a_broken_one() {
    let img = sky(1600, 1000, &[(200.0, 230.0, 30.0, 3.0, 0.3), (260.0, 300.0, 40.0, 2.0, 0.2)]);
    assert_eq!(wires::detect(&img, 50.0).len(), 2);
}

#[test]
fn a_kink_at_a_pole_still_gives_wires() {
    // two spans meeting at a pole in the middle, at different heights: a bend no single parabola follows
    let (w, h) = (1600usize, 1000usize);
    let mut img = sky(w, h, &[(300.0, 330.0, 0.0, 2.5, 0.25)]);
    let right = sky(w, h, &[(380.0, 330.0, 0.0, 2.5, 0.25)]);
    for y in 0..h {
        for x in w / 2..w {
            img.data[y * w + x] = right.data[y * w + x];
        }
    }
    let found = wires::detect(&img, 50.0);
    assert!(!found.is_empty(), "no wire");
    assert!(found.iter().all(|c| c.kind == Kind::Wire));
}

#[test]
fn a_clean_sky_has_no_wires() {
    assert!(wires::detect(&sky(1200, 800, &[]), 100.0).is_empty());
}

#[test]
fn an_edge_and_a_short_line_are_not_wires() {
    // a roofline: sky above, dark below, with a gentle slope
    let (w, h) = (1200usize, 800usize);
    let mut roof = sky(w, h, &[]);
    for y in 0..h {
        for x in 0..w {
            if y as f64 > 500.0 + 0.05 * x as f64 {
                roof.data[y * w + x] = [30, 30, 30, 255];
            }
        }
    }
    assert!(wires::detect(&roof, 100.0).is_empty(), "a roof edge");
    // a line a twentieth of the width
    let short = sky(w, h, &[(300.0, 300.0, 0.0, 2.5, 0.3)]);
    let clean = sky(w, h, &[]);
    let mut cut = short.clone();
    for y in 0..h {
        for x in 0..w {
            if !(500..560).contains(&x) {
                cut.data[y * w + x] = clean.data[y * w + x];
            }
        }
    }
    assert!(wires::detect(&cut, 100.0).is_empty(), "a short line");
}

#[test]
fn textured_scenes_give_few_false_wires() {
    for i in 0..3 {
        let rendered = lightcraft_scenes::demo_library()[i].render(800, 600);
        let data = rendered
            .data
            .iter()
            .map(|c| {
                let e = |v: f32| (lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0) as u8;
                [e(c[0]), e(c[1]), e(c[2]), 255]
            })
            .collect();
        let found = wires::detect(&Rgba8 { width: 800, height: 600, data }, 50.0);
        assert!(found.len() <= 1, "scene {i}: {} false wires", found.len());
    }
}

#[test]
fn hostile_images_do_not_panic() {
    assert!(wires::detect(&Rgba8 { width: 0, height: 0, data: vec![] }, 50.0).is_empty());
    assert!(wires::detect(&Rgba8 { width: 10, height: 10, data: vec![[0; 4]; 100] }, 50.0).is_empty());
    assert!(wires::detect(&Rgba8 { width: 100, height: 100, data: vec![[0; 4]; 5] }, 50.0).is_empty(), "data shorter than the size");
    let black = Rgba8 { width: 300, height: 200, data: vec![[0, 0, 0, 255]; 60000] };
    assert!(wires::detect(&black, f32::NAN).is_empty());
    // a one-pixel-wide image of noise
    let noise = Rgba8 {
        width: 70,
        height: 4000,
        data: (0..280000u32)
            .map(|i| {
                let v = (i.wrapping_mul(2654435761) >> 24) as u8;
                [v, v, v, 255]
            })
            .collect(),
    };
    let _ = wires::detect(&noise, 100.0);
}

/// Spots from the plans, as the engine will add them, and the Remove tool's own renderer applied to a linear copy.
fn remove(img: &Rgba8, plans: &[&Candidate]) -> Rgb32f {
    let (w, h) = (img.width, img.height);
    let mut lin = Rgb32f::from_fn(w, h, |x, y| {
        let p = img.data[y * w + x];
        [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]
    });
    let spots: Vec<Spot> = plans
        .iter()
        .flat_map(|c| &c.spots)
        .map(|s| Spot {
            mode: SpotMode::Fill,
            points: s.points.iter().map(|p| Point::new(p.0, p.1)).collect(),
            size: s.size,
            feather: SPOT_FEATHER,
            opacity: 100.0,
            source_offset: None,
            ..Default::default()
        })
        .collect();
    let frame = Frame::new(w, h, &DevelopSettings::default(), true);
    crate::spots::apply(&mut lin, &spots, &frame, frame.px_per_long(w));
    lin
}

#[test]
fn removing_a_wire_leaves_the_sky() {
    let (w, h) = (1600, 1000);
    let with = sky(w, h, &[(300.0, 380.0, 60.0, 2.5, 0.25)]);
    let clean = sky(w, h, &[]);
    let found = wires::detect(&with, 50.0);
    let lin = remove(&with, &[&found[0]]);
    // along the wire's whole length, every pixel is back to the sky within a few levels of 255
    let mut worst = 0.0f32;
    for p in &found[0].outline {
        let (x, y) = ((p.0 * w as f64) as usize, (p.1 * h as f64) as usize);
        for yy in y.saturating_sub(4)..(y + 5).min(h) {
            let got = lin.get(x.min(w - 1), yy);
            let want = clean.data[yy * w + x.min(w - 1)];
            worst = worst.max((got[2] - f32::from(want[2]) / 255.0).abs());
        }
    }
    assert!(worst < 0.02, "worst difference {worst}");
    // and nothing far from the wire moved
    for (x, y) in [(100usize, 800usize), (900, 50), (1500, 600)] {
        assert_eq!(lin.get(x, y)[2], f32::from(with.data[y * w + x][2]) / 255.0);
    }
}

#[test]
fn removing_a_person_leaves_the_background() {
    // a gradient wall with a person (head and body) in front of it
    let (w, h) = (900usize, 600usize);
    let wall = |x: usize, y: usize| 0.35 + 0.3 * (x as f32 / w as f32) + 0.1 * (y as f32 / h as f32);
    let person = |x: usize, y: usize| {
        let (x, y) = (x as f64, y as f64);
        ((x - 300.0).hypot(y - 190.0) < 26.0) || ((x - 300.0).abs() < 40.0 && (220.0..520.0).contains(&y))
    };
    let img = Rgba8 {
        width: w,
        height: h,
        data: (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let v = if person(x, y) { 0.1 } else { wall(x, y) };
                let b = (v * 255.0) as u8;
                [b, b, b, 255]
            })
            .collect(),
    };
    // the mask SAM would give: 288 cells over the photo
    let logits: Vec<f32> = (0..288 * 288)
        .map(|i| {
            let (x, y) = (((i % 288) as f64 + 0.5) / 288.0 * w as f64, ((i / 288) as f64 + 0.5) / 288.0 * h as f64);
            if person(x as usize, y as usize) { 5.0 } else { -5.0 }
        })
        .collect();
    let found = people::from_logits(288, &logits, w, h);
    assert_eq!(found.len(), 1);
    let lin = remove(&img, &[&found[0]]);
    let (mut worst, mut mean, mut n) = (0.0f32, 0.0f32, 0);
    for y in 0..h {
        for x in 0..w {
            if person(x, y) {
                let d = (lin.get(x, y)[0] - wall(x, y)).abs();
                worst = worst.max(d);
                mean += d;
                n += 1;
            }
        }
    }
    assert!(mean / (n as f32) < 0.02 && worst < 0.08, "mean {} worst {worst}", mean / n as f32);
}

#[test]
fn thick_and_thin_wires_in_a_noisy_sky() {
    // the noise a real sky has (about one level in a hundred), a 5 px cable and a 1.5 px one close together
    let (w, h) = (1280usize, 960usize);
    let mut img = sky(w, h, &[(500.0, 300.0, 10.0, 5.0, 0.35), (520.0, 322.0, 10.0, 1.5, 0.12)]);
    for (i, p) in img.data.iter_mut().enumerate() {
        let n = ((i as u32).wrapping_mul(2654435761) >> 24) as i32 - 128;
        let d = n / 40; // -3..3 levels
        for c in p.iter_mut().take(3) {
            *c = (i32::from(*c) + d).clamp(0, 255) as u8;
        }
    }
    let found = wires::detect(&img, 50.0);
    assert_eq!(found.len(), 2, "{} wires", found.len());
    assert!(found.iter().all(|c| c.bbox[2] - c.bbox[0] > 0.9));
    // the thick one gets the bigger dabs
    let size = |c: &Candidate| c.spots.iter().map(|s| s.size).fold(0.0, f64::max);
    let (a, b) = (size(&found[0]), size(&found[1]));
    assert!(a.max(b) > a.min(b), "{a} vs {b}");
}

#[test]
fn removing_a_wire_found_through_a_sam_mask_leaves_the_sky() {
    // the "power line" mask SAM 3 gives is a band a few pixels wide around the wire, at 288 cells over the photo
    let (w, h) = (1600usize, 1000usize);
    let img = sky(w, h, &[(300.0, 380.0, 60.0, 2.5, 0.25)]);
    let clean = sky(w, h, &[]);
    let wire_y = |x: f64| 300.0 + 80.0 * (x / w as f64) + 60.0 * 4.0 * (x / w as f64) * (1.0 - x / w as f64);
    let logits: Vec<f32> = (0..288 * 288)
        .map(|i| {
            let (x, y) = (((i % 288) as f64 + 0.5) / 288.0 * w as f64, ((i / 288) as f64 + 0.5) / 288.0 * h as f64);
            if (y - wire_y(x)).abs() < 9.0 { 4.0 } else { -4.0 }
        })
        .collect();
    let found = people::regions_from_logits(Kind::Wire, 288, &logits, w, h);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].kind, found[0].source, found[0].main), (Kind::Wire, "sam3", false));
    let lin = remove(&img, &[&found[0]]);
    let mut worst = 0.0f32;
    for x in (20..w - 20).step_by(7) {
        let y = wire_y(x as f64 + 0.5) as usize;
        for yy in y - 3..=y + 3 {
            worst = worst.max((lin.get(x, yy)[2] - f32::from(clean.data[yy * w + x][2]) / 255.0).abs());
        }
    }
    assert!(worst < 0.03, "worst difference {worst}");
}

#[test]
fn cancelling_stops_the_line_detector() {
    use std::sync::atomic::AtomicBool;
    let img = sky(1600, 1000, &[(300.0, 380.0, 60.0, 2.5, 0.25)]);
    assert_eq!(wires::detect_with(&img, 50.0, &AtomicBool::new(false)).len(), 1);
    let t = std::time::Instant::now();
    assert!(wires::detect_with(&img, 50.0, &AtomicBool::new(true)).is_empty(), "set before it starts: nothing, at once");
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
}

#[test]
fn a_photo_full_of_specks_still_gives_its_wire() {
    // dark specks all over a sky (dust, birds, noise) and one wire: thousands of small components
    let (w, h) = (1600usize, 1000usize);
    let mut img = sky(w, h, &[(300.0, 380.0, 60.0, 2.5, 0.25)]);
    for y in (440..h - 20).step_by(9) {
        for x in (20..w - 20).step_by(11) {
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = &mut img.data[(y + dy) * w + x + dx];
                *p = [p[0] / 3, p[1] / 3, p[2] / 3, 255];
            }
        }
    }
    let found = wires::detect(&img, 50.0);
    assert_eq!(found.len(), 1, "{} wires", found.len());
    assert!(found[0].bbox[2] - found[0].bbox[0] > 0.9);
}
