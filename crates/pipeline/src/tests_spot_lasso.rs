//! Brushed spots as one shape and lassos (`fork/brush-area.md`).

use lightcraft_develop::{DevelopSettings, Spot, SpotArea, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use super::*;
use crate::geometry::Frame;
use crate::spots::apply;

const PHOTO: Option<[f64; 4]> = Some([0.0, 0.0, 1.0, 1.0]);

/// A loop of `n` points around `(cx, cy)` (long units), radius `rad`, ending `gap` short of where it started.
fn circle(cx: f64, cy: f64, rad: f64, n: usize, gap: f64) -> Vec<Point> {
    (0..n)
        .map(|i| {
            let t = (i as f64 / (n - 1) as f64) * (std::f64::consts::TAU - gap);
            Point::new(cx + rad * t.cos(), cy + rad * t.sin())
        })
        .collect()
}

fn inside(c: &Cover, p: Point) -> bool {
    let (x, y) = (((p.x - c.x0) / c.cell).floor(), ((p.y - c.y0) / c.cell).floor());
    x >= 0.0 && y >= 0.0 && c.mask.get(x as i64, y as i64)
}

fn covered(points: &[Point], r: f64, lasso_ok: bool) -> Shape {
    shape(points, r, 500, PHOTO, lasso_ok).unwrap()
}

// Given a long stroke, it is one piece with one outline
#[test]
fn a_long_stroke_has_one_outline() {
    let pts: Vec<Point> = (0..300).map(|i| Point::new(0.1 + 0.8 * i as f64 / 299.0, 0.3 + 0.1 * (i as f64 / 20.0).sin())).collect();
    let rings = outline(&pts, 0.02, 8);
    assert_eq!(rings.len(), 1);
    assert!(rings[0].len() > 8);
    for p in &rings[0] {
        assert!(p.x > 0.1 - 0.03 && p.x < 0.9 + 0.03 && p.y > 0.2 - 0.03 && p.y < 0.4 + 0.03, "{p:?}");
    }
}

// Given a stroke, the cover holds the stroke's discs and nothing farther than a radius
#[test]
fn the_cover_is_the_union_of_discs() {
    let pts = vec![Point::new(0.2, 0.5), Point::new(0.5, 0.5)];
    let s = covered(&pts, 0.03, true);
    assert!(!s.lasso);
    let c = &s.cover;
    assert!(inside(c, Point::new(0.35, 0.5)) && inside(c, Point::new(0.35, 0.525)) && inside(c, Point::new(0.21, 0.5)));
    assert!(!inside(c, Point::new(0.35, 0.545)) && !inside(c, Point::new(0.15, 0.5)));
}

// Given a stroke that comes back near its start around a real area, it is a lasso; an arc, a stroke out and back, a
// sliver and a dab are not
#[test]
fn only_a_stroke_that_encloses_something_is_a_loop() {
    let r = 0.02;
    assert!(is_loop(&circle(0.5, 0.5, 0.15, 90, 0.2), r, PHOTO), "nearly closed circle");
    assert!(!is_loop(&circle(0.5, 0.5, 0.15, 90, 3.0), r, PHOTO), "half a circle");
    let out_and_back: Vec<Point> = (0..40).map(|i| Point::new(0.2 + 0.4 * (1.0 - (i as f64 - 20.0).abs() / 20.0), 0.5)).collect();
    assert!(!is_loop(&out_and_back, r, PHOTO), "out and back encloses nothing");
    // a sliver: out along one line and back along another a hair away
    let sliver: Vec<Point> =
        (0..20).map(|i| Point::new(0.2 + 0.02 * i as f64, 0.5)).chain((0..20).map(|i| Point::new(0.58 - 0.02 * i as f64, 0.51))).collect();
    assert!(!is_loop(&sliver, r, PHOTO), "a sliver");
    assert!(!is_loop(&[Point::new(0.5, 0.5); 12], r, PHOTO), "a dab");
    assert!(!is_loop(&[], r, PHOTO));
    assert!(!is_loop(&[Point::new(f64::NAN, 0.1); 20], r, PHOTO));
}

// Given a small circle around a blemish (a hole of under three brush discs), it stays a stroke; a bigger one is a lasso
#[test]
fn a_small_loop_stays_a_stroke() {
    let r = 0.02;
    // loop radius 2 r: the ribbon reaches in to r, the hole has radius r: one disc
    assert!(!is_loop(&circle(0.5, 0.5, 2.0 * r, 40, 0.2), r, PHOTO));
    // loop radius 2.5 r: the hole has radius 1.5 r: 2.25 discs
    assert!(!is_loop(&circle(0.5, 0.5, 2.5 * r, 40, 0.2), r, PHOTO));
    // loop radius 3.5 r: the hole has radius 2.5 r: 6 discs
    assert!(is_loop(&circle(0.5, 0.5, 3.5 * r, 60, 0.2), r, PHOTO));
}

// Given a figure 8 (its signed area cancels), both lobes are filled
#[test]
fn a_figure_eight_fills_both_lobes() {
    let r = 0.01;
    let eight: Vec<Point> = (0..=120)
        .map(|i| {
            let t = i as f64 / 120.0 * std::f64::consts::TAU;
            Point::new(0.5 + 0.2 * t.sin(), 0.5 + 0.12 * (2.0 * t).sin())
        })
        .collect();
    let s = covered(&eight, r, true);
    assert!(s.lasso);
    assert!(inside(&s.cover, Point::new(0.5 + 0.1, 0.5)) && inside(&s.cover, Point::new(0.5 - 0.1, 0.5)), "both lobes");
}

// Given a closed loop, the shape covers the inside; without the lasso, only the ribbon
#[test]
fn a_filled_loop_covers_its_inside() {
    let pts = circle(0.5, 0.5, 0.15, 90, 0.2);
    let (ribbon, filled) = (covered(&pts, 0.02, false), covered(&pts, 0.02, true));
    let mid = Point::new(0.5, 0.5);
    assert!(!inside(&ribbon.cover, mid) && !ribbon.lasso);
    assert!(filled.lasso && inside(&filled.cover, mid));
    assert!(inside(&filled.cover, Point::new(0.65, 0.5)) && !inside(&filled.cover, Point::new(0.5, 0.5 + 0.15 + 0.03)));
}

// Given a loop partly off the photo, the shape stays on the photo (its box and its pin too)
#[test]
fn a_loop_is_clipped_to_the_photo() {
    // a 4:3 photo; a loop around its right edge
    let (ow, oh) = (4000.0, 3000.0);
    let loop_norm: Vec<Point> = circle(1.0, 0.5, 0.14, 90, 0.1).iter().map(|p| Point::new(p.x, p.y)).collect();
    let Lasso::Area(seg) = lasso_area(&loop_norm, ow, oh, 0.02) else { panic!("a lasso") };
    let r = seg.bounds().unwrap();
    assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= 1.0 + 1e-9 && r[3] <= 1.0 + 1e-9, "{r:?}");
    // and the pin (the middle of that box) is on the photo
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    assert!((0.0..=1.0).contains(&cx) && (0.0..=1.0).contains(&cy));
    // a loop wholly off the photo is nothing
    let off: Vec<Point> = circle(3.0, 3.0, 0.14, 90, 0.1);
    assert_eq!(lasso_area(&off, ow, oh, 0.02), Lasso::No);
}

// Given a loop around most of the photo, it is refused (as Remove This Area does)
#[test]
fn a_loop_over_most_of_the_photo_is_too_big() {
    let big = circle(0.5, 0.5, 0.48, 120, 0.04);
    assert_eq!(lasso_area(&big, 1000.0, 1000.0, 0.02), Lasso::TooBig);
    assert!(matches!(lasso_area(&circle(0.5, 0.5, 0.25, 120, 0.04), 1000.0, 1000.0, 0.02), Lasso::Area(_)));
}

// Given a loop of any size, the stored mask has the same resolution across it: about 840 cells (a full-frame export is
// not soft), never more than the 1,024 a stored shape may have
#[test]
fn the_stored_mask_is_fine_whatever_the_loop() {
    for rad in [0.08, 0.2, 0.4] {
        let Lasso::Area(seg) = lasso_area(&circle(0.5, 0.5, rad, 160, 0.02), 6000.0, 4000.0, 0.01) else { panic!("a lasso of {rad}") };
        assert!((800..=1024).contains(&seg.side), "{rad}: {}", seg.side);
    }
}

// Given a hostile stroke (100k points, NaN, infinities, far off), the work stays bounded and nothing panics
#[test]
fn a_hostile_stroke_is_bounded() {
    let mut pts: Vec<Point> = (0..100_000)
        .map(|i| match i % 5 {
            0 => Point::new(f64::NAN, 0.5),
            1 => Point::new(f64::INFINITY, f64::NEG_INFINITY),
            2 => Point::new(1e300, -1e300),
            _ => Point::new((i % 97) as f64 / 97.0, (i % 89) as f64 / 89.0),
        })
        .collect();
    pts.push(pts[3]);
    for r in [f64::NAN, -1.0, 0.0, 1e300, 0.02] {
        for ok in [false, true] {
            if let Some(s) = shape(&pts, r, OUTLINE_SIDE, PHOTO, ok) {
                let c = s.cover;
                assert!(c.mask.w <= OUTLINE_SIDE + 2 && c.mask.h <= OUTLINE_SIDE + 2);
                assert_eq!(c.mask.bits.len(), c.mask.w * c.mask.h);
            }
        }
        let _ = outline(&pts, r, 8);
        let _ = is_loop(&pts, r, PHOTO);
        let _ = lasso_area(&pts, 1000.0, 800.0, r);
    }
    assert_eq!(lasso_area(&pts, f64::NAN, 1.0, 0.02), Lasso::No);
    assert_eq!(lasso_area(&pts, 0.0, 1.0, 0.02), Lasso::No);
    assert!(shape(&[], 0.02, 100, PHOTO, true).is_none());
    assert!(shape(&[Point::new(0.5, 0.5)], 0.02, 2, PHOTO, true).is_none());
}

fn photo(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        if (u - 0.5).abs() < 0.1 && (v - 0.5).abs() < 0.1 { [0.02, 0.02, 0.03] } else { [0.4 + 0.1 * u as f32, 0.45, 0.5] }
    })
}

fn lasso_spot(pts: &[Point], size: f64, feather: f64) -> Spot {
    let Lasso::Area(seg) = lasso_area(pts, 200.0, 200.0, size) else { panic!("a lasso") };
    Spot { mode: SpotMode::Fill, points: vec![Point::new(0.5, 0.5)], size, feather, area: Some(SpotArea::new("lasso", seg)), ..Default::default() }
}

// Given a loop around a dark block: as a brushed stroke it changes only the ribbon (the block stays); as a lasso its
// inside is rebuilt, so the block goes and a pixel well past the ribbon is untouched
#[test]
fn a_lasso_removes_what_it_surrounds_and_a_stroke_only_its_ribbon() {
    let (w, h) = (200usize, 200usize);
    let original = photo(w, h);
    let frame = Frame::new(w, h, &DevelopSettings::default(), true);
    let pts = circle(0.5, 0.5, 0.25, 120, 0.15);
    let size = 0.025;
    assert!(is_loop(&pts, size, PHOTO));
    let stroke = Spot { mode: SpotMode::Fill, points: pts.clone(), size, feather: 0.0, ..Default::default() };
    let mut a = original.clone();
    apply(&mut a, &[stroke], &frame, frame.px_per_long(w));
    assert_eq!(a.get(100, 100), original.get(100, 100), "an open stroke leaves the inside alone");
    assert_ne!(a.get(100 + 50, 100), original.get(100 + 50, 100), "the ribbon is covered");

    let mut b = original.clone();
    apply(&mut b, &[lasso_spot(&pts, size, 0.0)], &frame, frame.px_per_long(w));
    let c = b.get(100, 100);
    assert!(c[1] > 0.35, "the block inside the loop is gone: {c:?}");
    assert_ne!(b.get(100 + 50, 100), original.get(100 + 50, 100), "the ribbon is covered too");
    // (the loop is at 0.25 of the picture, 50 px out; the spot reaches one brush radius, 5 px, past it)
    assert_eq!(b.get(100 + 50 + 9, 100), original.get(100 + 50 + 9, 100), "past the ribbon nothing changes");
    assert!(b.data.iter().flatten().all(|v| v.is_finite()));
}

// Given a lasso and a stroke with the same brush and feather, the soft edge past the loop fades over the same
// distance (Feather × the brush radius), not over a fixed margin
#[test]
fn a_lasso_feathers_like_a_stroke() {
    let (w, h) = (400usize, 400usize);
    let frame = Frame::new(w, h, &DevelopSettings::default(), true);
    let ppl = frame.px_per_long(w);
    let (rad, size, feather) = (0.25, 0.04, 80.0);
    let pts = circle(0.5, 0.5, rad, 200, 0.1);
    let stroke = Spot { mode: SpotMode::Fill, points: pts.clone(), size, feather, ..Default::default() };
    let lasso = lasso_spot_sized(&pts, size, feather, w);
    let alpha_of = |s: &Spot| {
        let l = crate::spots::layout(s, &frame, w, h, ppl).unwrap();
        let a = crate::spot_area::alpha(s, &frame, w, h, (l.bx, l.by, l.bw, l.bh), l.r, l.inner, l.opacity);
        let flat = a.unwrap_or_else(|| {
            let mut v = vec![0.0f32; l.bw * l.bh];
            for y in 0..l.bh {
                for x in 0..l.bw {
                    let (px, py) = ((l.bx + x) as f32 + 0.5, (l.by + y) as f32 + 0.5);
                    let d = l.pts.iter().map(|p| ((px - p.0).powi(2) + (py - p.1).powi(2)).sqrt()).fold(f32::MAX, f32::min);
                    v[y * l.bw + x] = (1.0 - crate::spots::smooth(l.inner, l.r, d)) * l.opacity;
                }
            }
            v
        });
        (l.bx, l.by, l.bw, flat)
    };
    let (sa, la) = (alpha_of(&stroke), alpha_of(&lasso));
    let at = |a: &(usize, usize, usize, Vec<f32>), x: usize, y: usize| a.3.get((y - a.1) * a.2 + (x - a.0)).copied().unwrap_or(0.0);
    // along a ray out of the loop's right-hand side, from the loop's edge to a radius past it
    let edge = (0.5 + rad) * w as f64;
    let r_px = size * ppl;
    let mut fading = 0;
    for k in 1..=10 {
        let x = (edge + r_px * k as f64 / 10.0) as usize;
        let (s, l) = (at(&sa, x, 200), at(&la, x, 200));
        assert!((s - l).abs() < 0.08, "at {k}/10 of a radius past the loop: stroke {s}, lasso {l}");
        if s > 0.05 && s < 0.95 {
            fading += 1;
        }
    }
    assert!(fading >= 3, "the edge really fades over several samples");
}

fn lasso_spot_sized(pts: &[Point], size: f64, feather: f64, w: usize) -> Spot {
    let Lasso::Area(seg) = lasso_area(pts, w as f64, w as f64, size) else { panic!("a lasso") };
    Spot { mode: SpotMode::Fill, points: vec![Point::new(0.5, 0.5)], size, feather, area: Some(SpotArea::new("lasso", seg)), ..Default::default() }
}
