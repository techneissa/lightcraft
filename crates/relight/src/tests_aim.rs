//! A softbox's aim (issue #335): it lights the side it points to, its beam in haze slants along it, a patterned one shines
//! along it, and a softbox with no aim is exactly what it always was.

use crate::kinds::{BOX_GRID_TEST, Shape};
use crate::{Effects, Field, Light, LightKind, Pattern, Softbox};

const W: usize = 192;
const H: usize = 128;

fn boxed(aim_h: f64, aim_v: f64) -> Light {
    let kind = LightKind::Softbox(Softbox { width: 0.2, height: 0.2, rotation: 0.0, aim_h, aim_v });
    Light { x: 0.5, y: 0.5, depth: 0.3, amount: 1.0, softness: 0.4, warmth: 0.0, wrap: 0.0, kind }
}

/// A wall facing the camera (log depth 0.4), two stops under grey.
fn wall() -> Field {
    Field::from_parts(W, H, vec![0.4; W * H], vec![-2.0; W * H]).unwrap()
}

fn gain(f: &Field, l: &Light, u: f32, v: f32) -> f32 {
    f.shade(&f.rig(l, 0.0, 0.0), u, v).stops
}

#[test]
fn an_aimed_softbox_lights_the_side_it_points_to_more_than_the_other() {
    let f = wall();
    // straight in: both sides alike
    let straight = boxed(0.0, 0.0);
    assert!((gain(&f, &straight, 0.15, 0.5) - gain(&f, &straight, 0.85, 0.5)).abs() < 0.02);
    assert!((gain(&f, &straight, 0.5, 0.15) - gain(&f, &straight, 0.5, 0.85)).abs() < 0.02);
    // turned to the right (and then left, down, up): the side it points to is the brighter
    let right = boxed(50.0, 0.0);
    let (to, away) = (gain(&f, &right, 0.85, 0.5), gain(&f, &right, 0.15, 0.5));
    assert!(to > away + 0.3, "right: lit {to}, the other side {away}");
    let left = boxed(-50.0, 0.0);
    assert!(gain(&f, &left, 0.15, 0.5) > gain(&f, &left, 0.85, 0.5) + 0.3);
    let down = boxed(0.0, 50.0);
    assert!(gain(&f, &down, 0.5, 0.85) > gain(&f, &down, 0.5, 0.15) + 0.3);
    let up = boxed(0.0, -50.0);
    assert!(gain(&f, &up, 0.5, 0.15) > gain(&f, &up, 0.5, 0.85) + 0.3);
}

#[test]
fn an_aimed_softbox_beam_in_haze_slants_along_the_aim() {
    let f = wall();
    let e = Effects { haze: 0.6, ..Effects::default() };
    let beam = |l: &Light| {
        let rig = f.rig(l, 0.0, 0.0);
        let b = f.beam(&f.shafts(&rig, l, false).unwrap(), &rig, &e);
        let (w, h) = b.size();
        move |u: f32, v: f32| b.at((u * w as f32) as usize, (v * h as f32) as usize)
    };
    let straight = beam(&boxed(0.0, 0.0));
    let (l, r) = (straight(0.15, 0.5), straight(0.85, 0.5));
    assert!(l > 0.0 && (l - r).abs() < 0.02 * l.max(r), "straight in: left {l}, right {r}");
    let right = beam(&boxed(50.0, 0.0));
    let (l, r) = (right(0.15, 0.5), right(0.85, 0.5));
    assert!(r > 1.3 * l, "aimed right: left {l}, right {r}");
    let up = beam(&boxed(0.0, -50.0));
    let (top, bottom) = (up(0.5, 0.15), up(0.5, 0.85));
    assert!(top > 1.3 * bottom, "aimed up: top {top}, bottom {bottom}");
}

#[test]
fn a_patterned_aimed_softbox_shines_from_against_its_aim() {
    let f = wall();
    let e = Effects { pattern: Pattern::Panes, ..Effects::default() };
    let source = |l: &Light| f.pattern_rig(&f.rig(l, 0.0, 0.0), &e).unwrap().position();
    let (straight, right, left) = (source(&boxed(0.0, 0.0)), source(&boxed(50.0, 0.0)), source(&boxed(-50.0, 0.0)));
    // (light that travels to the right comes from the left of where it lands, and the other way)
    assert!(right[0] < straight[0] - 0.1 * straight[2] && left[0] > straight[0] + 0.1 * straight[2], "{straight:?} {right:?} {left:?}");
}

/// The softbox's light as it was before it could be aimed: the box's points shine straight into the scene.
fn old_light_at(rig: &crate::Rig, p: [f32; 3], n: [f32; 3]) -> f32 {
    let Shape::Box { c, tx, ty, hw, hh, eps2, .. } = rig.shape else { return f32::NAN };
    let g = BOX_GRID_TEST;
    let mut sum = 0.0f32;
    for j in 0..g {
        let b = ((j as f32 + 0.5) / g as f32 * 2.0 - 1.0) * hh;
        for i in 0..g {
            let a = ((i as f32 + 0.5) / g as f32 * 2.0 - 1.0) * hw;
            let s = [c[0] + tx[0] * a + ty[0] * b, c[1] + tx[1] * a + ty[1] * b, c[2]];
            let d = [s[0] - p[0], s[1] - p[1], s[2] - p[2]];
            let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let r = r2.sqrt().max(1e-9);
            let emit = (-d[2] / r).max(0.0);
            let cos = (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) / r;
            sum += crate::light::facing(rig.face, rig.wrap, cos) * emit * (eps2 / (eps2 + r2));
        }
    }
    sum / (g * g) as f32
}

#[test]
fn a_softbox_with_no_aim_gives_the_same_light_to_the_last_bit() {
    let lz: Vec<f32> = (0..W * H).map(|i| 0.2 + 0.3 * ((i % W) as f32 / W as f32) + 0.1 * ((i / W) as f32 / H as f32)).collect();
    let f = Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap();
    for rotation in [0.0, 25.0] {
        let l = Light { kind: LightKind::Softbox(Softbox { width: 0.3, height: 0.15, rotation, aim_h: 0.0, aim_v: 0.0 }), ..boxed(0.0, 0.0) };
        let rig = f.rig(&l, 0.0, 0.0);
        for k in 0..400 {
            let (u, v) = (((k * 7) % 20) as f32 / 20.0 + 0.025, (k / 20) as f32 / 20.0 + 0.025);
            let (lz, n, _) = f.at(u, v);
            let now = f.light_at(&rig, u, v, lz, n);
            let before = old_light_at(&rig, f.point(u, v, lz), n);
            assert_eq!(now.to_bits(), before.to_bits(), "({u}, {v}) rotation {rotation}: {now} vs {before}");
        }
    }
}

#[test]
fn hostile_aims_give_finite_light() {
    let f = wall();
    for (h, v) in [(f64::NAN, 1e9), (f64::INFINITY, f64::NEG_INFINITY), (90.0, 90.0), (-1e300, 1e300)] {
        let l = boxed(h, v);
        let rig = f.rig(&l, 0.0, 0.0);
        let p = f.shading(&rig).planes(W, H, &|x, y| (x / W as f64, y / H as f64));
        assert!(p.stops.iter().chain(&p.share).all(|v| v.is_finite()), "aim {h} {v}");
        assert!(f.shafts(&rig, &l, true).is_some());
    }
}
