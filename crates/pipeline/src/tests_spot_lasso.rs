//! Brushed spots as one shape and lassos (`fork/brush-area.md`).

use lightcraft_develop::{DevelopSettings, Spot, SpotArea, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use super::*;
use crate::geometry::Frame;
use crate::spots::apply;

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

// Given a long stroke, it is one piece with one outline
#[test]
fn a_long_stroke_has_one_outline() {
    let pts: Vec<Point> = (0..300).map(|i| Point::new(0.1 + 0.8 * i as f64 / 299.0, 0.3 + 0.1 * (i as f64 / 20.0).sin())).collect();
    let rings = outline(&pts, 0.02, false, 8);
    assert_eq!(rings.len(), 1);
    assert!(rings[0].len() > 8);
    // the ring stays within a radius of the stroke's box
    for p in &rings[0] {
        assert!(p.x > 0.1 - 0.03 && p.x < 0.9 + 0.03 && p.y > 0.2 - 0.03 && p.y < 0.4 + 0.03, "{p:?}");
    }
}

// Given a stroke, the cover holds the stroke's discs and nothing farther than a radius
#[test]
fn the_cover_is_the_union_of_discs() {
    let pts = vec![Point::new(0.2, 0.5), Point::new(0.5, 0.5)];
    let c = cover(&pts, 0.03, false, 400).unwrap();
    assert!(inside(&c, Point::new(0.35, 0.5)));
    assert!(inside(&c, Point::new(0.35, 0.525)));
    assert!(inside(&c, Point::new(0.21, 0.5)));
    assert!(!inside(&c, Point::new(0.35, 0.545)));
    assert!(!inside(&c, Point::new(0.15, 0.5)));
}

// Given a stroke that comes back near its start, it is a loop; an arc, a stroke out and back, and a dab are not
#[test]
fn only_a_stroke_that_encloses_something_is_a_loop() {
    let r = 0.02;
    assert!(is_loop(&circle(0.5, 0.5, 0.15, 90, 0.2), r), "nearly closed circle");
    assert!(!is_loop(&circle(0.5, 0.5, 0.15, 90, 3.0), r), "half a circle");
    let out_and_back: Vec<Point> = (0..40).map(|i| Point::new(0.2 + 0.4 * (1.0 - (i as f64 - 20.0).abs() / 20.0), 0.5)).collect();
    assert!(!is_loop(&out_and_back, r), "out and back encloses nothing");
    assert!(!is_loop(&[Point::new(0.5, 0.5); 12], r), "a dab");
    assert!(!is_loop(&[], r));
    assert!(!is_loop(&[Point::new(f64::NAN, 0.1); 20], r));
}

// Given a closed loop, filling covers the inside; without it only the ribbon
#[test]
fn a_filled_loop_covers_its_inside() {
    let pts = circle(0.5, 0.5, 0.15, 90, 0.2);
    let ribbon = cover(&pts, 0.02, false, 500).unwrap();
    let filled = cover(&pts, 0.02, true, 500).unwrap();
    let mid = Point::new(0.5, 0.5);
    assert!(!inside(&ribbon, mid));
    assert!(inside(&filled, mid));
    assert!(inside(&filled, Point::new(0.5 + 0.15, 0.5)));
    assert!(!inside(&filled, Point::new(0.5, 0.5 + 0.15 + 0.03)));
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
        for fill in [false, true] {
            if let Some(c) = cover(&pts, r, fill, OUTLINE_SIDE) {
                assert!(c.mask.w <= OUTLINE_SIDE + 2 && c.mask.h <= OUTLINE_SIDE + 2);
                assert_eq!(c.mask.bits.len(), c.mask.w * c.mask.h);
            }
            let _ = outline(&pts, r, fill, 8);
        }
        let _ = is_loop(&pts, r);
        let _ = lasso_area(&pts, 1000.0, 800.0, r);
    }
    assert!(lasso_area(&pts, f64::NAN, 1.0, 0.02).is_none());
    assert!(lasso_area(&pts, 0.0, 1.0, 0.02).is_none());
    assert!(cover(&[], 0.02, true, 100).is_none());
    assert!(cover(&[Point::new(0.5, 0.5)], 0.02, true, 2).is_none());
}

fn photo(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        if (u - 0.5).abs() < 0.1 && (v - 0.5).abs() < 0.1 { [0.02, 0.02, 0.03] } else { [0.4 + 0.1 * u as f32, 0.45, 0.5] }
    })
}

// Given a loop around a dark block: as a brushed stroke it changes only the ribbon (the block stays); as a lasso its
// inside is rebuilt, so the block goes and a pixel just outside the ribbon is untouched
#[test]
fn a_lasso_removes_what_it_surrounds_and_a_stroke_only_its_ribbon() {
    let (w, h) = (200usize, 200usize);
    let original = photo(w, h);
    let frame = Frame::new(w, h, &DevelopSettings::default(), true);
    let pts_long = circle(0.5, 0.5, 0.25, 120, 0.15);
    let pts: Vec<Point> = pts_long.clone();
    let size = 0.025;
    assert!(is_loop(&pts_long, size));
    let stroke = Spot { mode: SpotMode::Fill, points: pts.clone(), size, feather: 0.0, ..Default::default() };
    let mut a = original.clone();
    apply(&mut a, &[stroke], &frame, frame.px_per_long(w));
    assert_eq!(a.get(100, 100), original.get(100, 100), "an open stroke leaves the inside alone");
    assert_ne!(a.get(100 + 50, 100), original.get(100 + 50, 100), "the ribbon is covered");

    let seg = lasso_area(&pts, w as f64, h as f64, size - 0.005).unwrap();
    let lasso = Spot {
        mode: SpotMode::Fill,
        points: vec![Point::new(0.5, 0.5)],
        size: 0.005,
        feather: 0.0,
        area: Some(SpotArea::new("lasso", seg)),
        ..Default::default()
    };
    let mut b = original.clone();
    apply(&mut b, &[lasso], &frame, frame.px_per_long(w));
    let c = b.get(100, 100);
    assert!(c[1] > 0.35, "the block inside the loop is gone: {c:?}");
    assert_ne!(b.get(100 + 50, 100), original.get(100 + 50, 100), "the ribbon is covered too");
    let far = (100 + 50 + 12, 100);
    assert_eq!(b.get(far.0, far.1), original.get(far.0, far.1), "past the ribbon nothing changes");
    assert!(b.data.iter().flatten().all(|v| v.is_finite()));
}
