//! Shaped Remove spots (`fork/object-remove.md`).

use lightcraft_develop::{DevelopSettings, SegMask, Spot, SpotArea, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use super::*;
use crate::spots::apply;

/// A gentle gradient with a dark block (the "object") at `x0..x1 × y0..y1`, as fractions of the picture.
fn photo(w: usize, h: usize, obj: [f64; 4]) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        if u >= obj[0] && u < obj[2] && v >= obj[1] && v < obj[3] { [0.02, 0.02, 0.03] } else { [0.4 + 0.1 * u as f32, 0.45, 0.5] }
    })
}

/// The object's coverage on a `w × h` grid over the picture.
fn coverage(w: usize, h: usize, obj: [f64; 4]) -> Vec<f32> {
    (0..w * h)
        .map(|i| {
            let (u, v) = (((i % w) as f64 + 0.5) / w as f64, ((i / w) as f64 + 0.5) / h as f64);
            if u >= obj[0] && u < obj[2] && v >= obj[1] && v < obj[3] { 1.0 } else { 0.0 }
        })
        .collect()
}

/// The object's shape, from its coverage on a `w × h` render of a `w × h` photo.
fn shape_of(w: usize, h: usize, obj: [f64; 4]) -> SegMask {
    let frame = Frame::new(w, h, &DevelopSettings::default(), false);
    let (to_norm, to_px) = (frame.out_to_norm(w, h), frame.norm_to_out(w, h));
    area_from_coverage(&coverage(w, h, obj), w, h, |p| to_norm.apply(p), |p| to_px.apply(p)).unwrap()
}

fn shaped(seg: SegMask, expand: f64) -> Spot {
    let r = seg.bounds().unwrap();
    Spot {
        mode: SpotMode::Remove,
        points: vec![Point::new((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)],
        size: expand,
        feather: 0.0,
        area: Some(SpotArea::new("object", seg)),
        ..Default::default()
    }
}

const OBJ: [f64; 4] = [0.4, 0.35, 0.6, 0.65];

// Given a click's shape, the spot removes the object and leaves the photo beyond its outline and the Expand margin as it was
#[test]
fn a_shaped_spot_removes_the_object_and_nothing_past_its_margin() {
    let (w, h) = (200usize, 150usize);
    let original = photo(w, h, OBJ);
    let mut img = original.clone();
    let frame = Frame::new(w, h, &DevelopSettings::default(), true);
    let expand_px = 4.0;
    let spot = shaped(shape_of(w, h, OBJ), expand_px / w as f64);
    apply(&mut img, &[spot], &frame, frame.px_per_long(w));
    let (ox0, oy0, ox1, oy1) = (OBJ[0] * w as f64, OBJ[1] * h as f64, OBJ[2] * w as f64, OBJ[3] * h as f64);
    let mut changed_outside = 0;
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let dx = (ox0 - px).max(px - ox1).max(0.0);
            let dy = (oy0 - py).max(py - oy1).max(0.0);
            if dx.hypot(dy) > expand_px + 1.5 && img.get(x, y) != original.get(x, y) {
                changed_outside += 1;
            }
        }
    }
    assert_eq!(changed_outside, 0, "pixels changed past the margin");
    // the dark block is gone: its middle now looks like the surroundings
    let c = img.get(100, 75);
    assert!(c[1] > 0.35, "{c:?}");
    assert!(img.data.iter().flatten().all(|v| v.is_finite()));
}

// Given the same shape at preview and export sizes, the alpha covers the same part of the photo
#[test]
fn preview_and_export_cover_the_same_area() {
    let seg = shape_of(400, 300, OBJ);
    let spot = Spot { feather: 50.0, ..shaped(seg, 0.01) };
    let cover = |w: usize, h: usize| {
        let frame = Frame::new(w, h, &DevelopSettings::default(), true);
        let ppl = frame.px_per_long(w);
        let l = crate::spots::layout(&spot, &frame, w, h, ppl).unwrap();
        let a = alpha(&spot, &frame, w, h, (l.bx, l.by, l.bw, l.bh), l.r, l.inner, l.opacity).unwrap();
        let mut full = vec![0.0f32; w * h];
        for y in 0..l.bh {
            for x in 0..l.bw {
                full[(l.by + y) * w + l.bx + x] = a[y * l.bw + x];
            }
        }
        full
    };
    let (big, small) = (cover(800, 600), cover(200, 150));
    let (mut err, mut n) = (0.0f32, 0);
    for y in 0..150 {
        for x in 0..200 {
            let b: f32 =
                (0..4).flat_map(|dy| (0..4).map(move |dx| (dx, dy))).map(|(dx, dy)| big[(4 * y + dy) * 800 + 4 * x + dx]).sum::<f32>() / 16.0;
            err += (b - small[y * 200 + x]).abs();
            n += 1;
        }
    }
    let covered: f32 = small.iter().sum();
    assert!(covered > 0.15 * 0.25 * 200.0 * 150.0 * 0.9, "covers at least the object: {covered}");
    assert!(err / (n as f32) < 0.01, "mean difference {}", err / n as f32);
}

// Given a window of a render, a shaped spot reaching into it grows the window to hold the spot and what it reads
#[test]
fn a_window_holds_a_shaped_spot_and_its_surroundings() {
    use crate::PixelWindow;
    let (w, h) = (4000usize, 3000usize);
    let frame = Frame::new(w, h, &DevelopSettings::default(), true);
    let ppl = frame.px_per_long(w);
    let spot = shaped(shape_of(400, 300, OBJ), 0.005);
    let s = DevelopSettings { spots: vec![spot.clone()], ..Default::default() };
    let win = PixelWindow { x: 1900, y: 1400, w: 200, h: 200 };
    let g = crate::spots::window_for_reads(&s, &frame, w, h, ppl, win);
    let l = crate::spots::layout(&spot, &frame, w, h, ppl).unwrap();
    assert!(g.x <= l.bx && g.y <= l.by && g.x + g.w >= l.bx + l.bw && g.y + g.h >= l.by + l.bh, "{g:?} vs {l:?}");
    // and the box takes in the surroundings, not only the object
    assert!(l.bw as f64 > 0.2 * w as f64 * 1.4, "{l:?}");
    assert!(l.fill && !l.heal, "rebuilt from the surroundings, no source");
}

// Given damaged or absurd shapes, rendering neither panics nor draws anything wrong
#[test]
fn hostile_shapes_draw_nothing_and_never_panic() {
    let (w, h) = (64usize, 48usize);
    let frame = Frame::new(w, h, &DevelopSettings::default(), true);
    let good = shape_of(64, 48, OBJ);
    let bad = [
        SegMask { side: 1024, data: "not base64".into(), rect: Some([f64::NAN, 0.0, 2.0, -1.0]), model: None },
        SegMask { side: 0, data: String::new(), rect: None, model: None },
        SegMask { rect: Some([0.5, 0.5, 0.5, 0.5]), ..good.clone() },
        SegMask { rect: Some([-1e300, -1e300, 1e300, 1e300]), ..good.clone() },
        SegMask { rect: Some([5.0, 5.0, 6.0, 6.0]), ..good.clone() },
    ];
    for seg in bad {
        for size in [0.0, 0.01, 2.0, f64::NAN, f64::INFINITY] {
            let original = photo(w, h, OBJ);
            let mut img = original.clone();
            let spot = Spot { size, ..shaped(good.clone(), 0.01) };
            let spot = Spot { area: Some(SpotArea::new("mask", seg.clone())), ..spot };
            apply(&mut img, &[spot], &frame, frame.px_per_long(w));
            assert!(img.data.iter().flatten().all(|v| v.is_finite()));
        }
    }
    // a shape whose data is damaged but whose rectangle is fine selects nothing
    let mut img = photo(w, h, OBJ);
    let damaged = SegMask { data: "AAAA".into(), ..good };
    apply(&mut img, &[shaped(damaged, 0.01)], &frame, frame.px_per_long(w));
    assert_eq!(img.data, photo(w, h, OBJ).data);
}

#[test]
fn coverage_with_nothing_inside_makes_no_shape() {
    let frame = Frame::new(20, 10, &DevelopSettings::default(), false);
    let (to_norm, to_px) = (frame.out_to_norm(20, 10), frame.norm_to_out(20, 10));
    assert!(area_from_coverage(&[0.0; 200], 20, 10, |p| to_norm.apply(p), |p| to_px.apply(p)).is_none());
    assert!(area_from_coverage(&[f32::NAN; 200], 20, 10, |p| to_norm.apply(p), |p| to_px.apply(p)).is_none());
    assert!(area_from_coverage(&[1.0; 10], 20, 10, |p| to_norm.apply(p), |p| to_px.apply(p)).is_none(), "wrong length");
    let all = area_from_coverage(&[1.0; 200], 20, 10, |p| to_norm.apply(p), |p| to_px.apply(p)).unwrap();
    assert_eq!(all.bounds(), Some([0.0, 0.0, 1.0, 1.0]));
}

// Given a flipped photo, the shape stays on the object (spot coordinates are before the flip)
#[test]
fn a_shape_from_a_flipped_render_lands_on_the_object() {
    let (w, h) = (200usize, 150usize);
    let mut s = DevelopSettings::default();
    s.crop.flip_h = true;
    let frame = Frame::new(w, h, &s, false);
    let (to_norm, to_px) = (frame.out_to_norm(w, h), frame.norm_to_out(w, h));
    // the render shows the object mirrored
    let mirrored = [1.0 - OBJ[2], OBJ[1], 1.0 - OBJ[0], OBJ[3]];
    let seg = area_from_coverage(&coverage(w, h, mirrored), w, h, |p| to_norm.apply(p), |p| to_px.apply(p)).unwrap();
    let r = seg.bounds().unwrap();
    assert!(r[0] < OBJ[0] && r[2] > OBJ[2] && r[0] > 0.3 && r[2] < 0.7, "{r:?}");
}
