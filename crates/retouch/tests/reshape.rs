//! Tests of the reshape feature (`lightcraft_retouch::reshape`): the warp's sliders move the face the way their
//! names say, never fold, stay local, and are the same at every size.

mod common;

use lightcraft_retouch::reshape::{Field, MESH, Reshape, sketch_mesh};
use lightcraft_retouch::{Feature, NormRect};

const FACE: NormRect = NormRect::new(0.35, 0.2, 0.65, 0.7);

fn with(f: impl Fn(&mut Reshape)) -> Reshape {
    let mut s = Reshape::default();
    f(&mut s);
    s
}

fn field(s: &Reshape, w: f64, h: f64) -> Option<Field> {
    let mesh = sketch_mesh(FACE);
    Field::build(&[(&mesh, s)], w, h)
}

/// Mesh point `i` of the sketched face.
fn point(i: usize) -> (f64, f64) {
    sketch_mesh(FACE)[MESH + i]
}

/// Every slider at its full value, each way, alone and all at once.
fn every_setting() -> Vec<Reshape> {
    let mut out = Vec::new();
    for c in Reshape::CONTROLS {
        for v in [-100.0, 100.0] {
            let mut s = Reshape::default();
            c.write(&mut s, v);
            out.push(s);
        }
    }
    for v in [-100.0, 100.0] {
        let mut s = Reshape::default();
        for c in Reshape::CONTROLS {
            c.write(&mut s, v);
        }
        out.push(s);
    }
    out
}

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Reshape>();
    // at 0 there is no warp at all: the pipeline's frame is the frame without it
    assert!(field(&Reshape::default(), 300.0, 200.0).is_none());
    assert!(field(&with(|s| s.chin = f64::NAN), 300.0, 200.0).is_none(), "a value that is not a number is 0");
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Reshape>();
    let s = with(|s| s.slim_face = 100.0);
    let many: Vec<Vec<(f64, f64)>> = (0..50)
        .map(|i| {
            let (x, y) = ((i % 10) as f64 / 10.0, (i / 10) as f64 / 5.0);
            sketch_mesh(NormRect::new(x, y, x + 0.1, y + 0.2))
        })
        .collect();
    let list: Vec<(&[(f64, f64)], &Reshape)> = many.iter().map(|m| (m.as_slice(), &s)).collect();
    let f = Field::build(&list, 600.0, 400.0).expect("50 faces move");
    assert!(f.keeps_cells(0.2));
    // a face cut by the photo's edge, a collapsed one, one that is not a number, one with five landmarks only
    let cut = sketch_mesh(NormRect::new(-0.2, -0.1, 0.15, 0.4));
    let collapsed = vec![(0.5, 0.5); MESH + 478];
    let nan = vec![(f64::NAN, 0.5); MESH + 478];
    let five = vec![(0.4, 0.4); 5];
    for (mesh, moves) in [(&cut, true), (&collapsed, false), (&nan, false), (&five, false), (&Vec::new(), false)] {
        assert_eq!(Field::build(&[(mesh, &s)], 600.0, 400.0).is_some(), moves);
    }
    for (w, h) in [(1.0, 1.0), (0.0, 0.0), (f64::NAN, 3.0), (1e9, 1.0)] {
        let _ = Field::build(&[(&cut, &s)], w, h);
    }
}

/// Each slider moves its part of the face the way its name says (`source`: where the reshaped photo reads from, so a
/// part that moves inward reads from further out).
#[test]
fn each_slider_moves_its_part_of_the_face() {
    let (w, h) = (300.0, 200.0);
    let src = |s: Reshape, p: (f64, f64)| field(&s, w, h).expect("it moves").source(p.0, p.1);
    let cheek = point(93);
    // Slim Face +: the cheek's edge shows what was further out (left of the subject's right cheek)
    assert!(src(with(|s| s.slim_face = 100.0), cheek).0 < cheek.0 - 0.003);
    assert!(src(with(|s| s.slim_face = -100.0), cheek).0 > cheek.0 + 0.003);
    // Jaw +: wider at the corners
    let corner = point(136);
    assert!(src(with(|s| s.jaw = 100.0), corner).0 > corner.0 + 0.003);
    // Chin +: longer, the chin's place now shows what was above it
    let chin = point(152);
    assert!(src(with(|s| s.chin = 100.0), chin).1 < chin.1 - 0.005);
    // Double Chin +: under the chin shows what was lower (the soft part is lifted)
    let under = (chin.0, chin.1 + 0.05);
    assert!(src(with(|s| s.double_chin = 100.0), under).1 > under.1 + 0.003);
    // Nose +: the wings move out, so the wing's place shows what was nearer the middle
    let wing = point(64);
    assert!(src(with(|s| s.nose = 100.0), wing).0 > wing.0 + 0.001);
    // Forehead +: the top of the face moves up
    let top = point(10);
    assert!(src(with(|s| s.forehead = 100.0), top).1 > top.1 + 0.005);
    // Eye Size +: a point beside the eye's centre reads from nearer it
    let (a, b) = (point(33), point(133));
    let centre = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let beside = (centre.0 + 0.01, centre.1);
    let bigger = src(with(|s| s.eye_size = 100.0), beside);
    assert!(bigger.0 < beside.0 - 0.001 && bigger.0 > centre.0);
    let smaller = src(with(|s| s.eye_size = -100.0), beside);
    assert!(smaller.0 > beside.0 + 0.001);
    // the eyes' and the mouth's middles stay where they are under the contour sliders
    let mouth = ((point(61).0 + point(291).0) / 2.0, point(61).1);
    for s in [with(|s| s.slim_face = 100.0), with(|s| s.forehead = 100.0), with(|s| s.nose = 100.0)] {
        let m = src(s, mouth);
        assert!((m.0 - mouth.0).abs() < 1e-4 && (m.1 - mouth.1).abs() < 2e-3, "{m:?}");
    }
}

/// The warp never folds: with every slider at full strength, each way, alone and together, every cell of each face's
/// grid stays the same way round with most of its area, and the warp, sampled finely across the face, keeps a
/// positive Jacobian.
#[test]
fn never_folds() {
    for s in every_setting() {
        let f = field(&s, 300.0, 200.0).expect("it moves");
        assert!(f.keeps_cells(0.2), "{s:?}");
        let n = 120;
        let d = 1e-4;
        for j in 0..n {
            for i in 0..n {
                let (x, y) = (0.25 + 0.5 * i as f64 / n as f64, 0.05 + 0.85 * j as f64 / n as f64);
                let p = f.source(x, y);
                let (px, py) = (f.source(x + d, y), f.source(x, y + d));
                let det = (px.0 - p.0) * (py.1 - p.1) - (px.1 - p.1) * (py.0 - p.0);
                assert!(det > 0.1 * d * d, "{s:?}: folds at {x},{y}: {det}");
            }
        }
    }
}

/// Far from the face nothing moves: a straight line there stays straight, bit for bit, and so does every point
/// outside the areas the warp reports.
#[test]
fn a_straight_line_far_from_the_face_stays_straight() {
    let all = with(|s| {
        for c in Reshape::CONTROLS {
            c.write(s, 100.0);
        }
    });
    let f = field(&all, 300.0, 200.0).expect("it moves");
    let areas = f.areas();
    let [x0, y0, x1, y1] = areas.first().copied().expect("one area");
    // the area is around the face, a little larger than it, never the whole photo
    assert!(x0 > 0.15 && x1 < 0.85 && y0 > -0.2 && y1 < 1.1 && x0 < FACE.x0 && x1 > FACE.x1, "{areas:?}");
    for i in 0..=200 {
        let t = i as f64 / 200.0;
        for p in [(0.1, t), (0.92, t), (t, 0.0), (t, 0.99)] {
            let inside = p.0 > x0 && p.0 < x1 && p.1 > y0 && p.1 < y1;
            if !inside {
                assert_eq!(f.source(p.0, p.1), p, "a point away from the face moved");
            }
        }
    }
}

/// Preview equals export: the warp is in fractions of the photo and of the face, so a preview, the loupe and a
/// full-size export (the same photo at another size, with the rounding a resize makes) read from the same places.
#[test]
fn the_same_at_every_size() {
    let s = with(|s| {
        s.slim_face = 70.0;
        s.eye_size = 50.0;
        s.chin = -40.0;
    });
    let small = field(&s, 600.0, 400.0).expect("it moves");
    for (w, h) in [(2400.0, 1600.0), (6000.0, 4000.0), (601.0, 400.0)] {
        let big = field(&s, w, h).expect("it moves");
        for j in 0..40 {
            for i in 0..40 {
                let (x, y) = (0.3 + 0.4 * i as f64 / 40.0, 0.15 + 0.6 * j as f64 / 40.0);
                let (a, b) = (small.source(x, y), big.source(x, y));
                assert!((a.0 - b.0).abs() < 2e-4 && (a.1 - b.1).abs() < 2e-4, "{w}×{h} at {x},{y}: {a:?} vs {b:?}");
            }
        }
    }
}

/// A face shows the same change whichever other faces are in the photo, as long as they are away from it; and its
/// own settings move it only.
#[test]
fn each_face_moves_alone() {
    let (left, right) = (sketch_mesh(NormRect::new(0.05, 0.2, 0.3, 0.7)), sketch_mesh(NormRect::new(0.65, 0.2, 0.9, 0.7)));
    let (on, off) = (with(|s| s.slim_face = 100.0), Reshape::default());
    let both = Field::build(&[(&left, &on), (&right, &on)], 400.0, 200.0).expect("it moves");
    let alone = Field::build(&[(&left, &on), (&right, &off)], 400.0, 200.0).expect("it moves");
    assert_eq!(alone.areas().len(), 1, "a face at 0 has no warp");
    let p = left[MESH + 93];
    assert_eq!(both.source(p.0, p.1), alone.source(p.0, p.1));
    let q = right[MESH + 93];
    assert_eq!(alone.source(q.0, q.1), q, "the other face did not move");
    assert_ne!(both.source(q.0, q.1), q);
}
