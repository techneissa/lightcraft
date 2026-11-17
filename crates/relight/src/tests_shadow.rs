//! Shadows (issue #315): a near thing between a light and a far wall shades the wall where it should and nowhere else, a
//! bigger light gives a wider penumbra, no shadow amount changes nothing, and hostile numbers give finite planes.

use crate::field::LENS;
use crate::{Field, Light, LightKind, Softbox, Sun};

const W: usize = 192;
const H: usize = 128;

/// A far wall (log depth 0.2) with a near box (log depth −0.6) over `u` 0.4 .. 0.5 and `v` 0.3 .. 0.7.
fn box_before_wall() -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (0.4..0.5).contains(&u) && (0.3..0.7).contains(&v) { -0.6 } else { 0.2 }
        })
        .collect();
    Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap()
}

fn light(softness: f64, kind: LightKind) -> Light {
    Light { x: 0.1, y: 0.5, depth: 0.0, amount: 1.0, softness, warmth: 0.0, wrap: 1.0, kind }
}

/// The share of the light each pixel of row `v` sees (the visibility brought to the field's size, full amount).
fn row(f: &Field, l: &Light, v: f32) -> Vec<f32> {
    let rig = f.rig(l, 0.0, 0.0);
    let vis = f.visibility(&rig, l).unwrap();
    let ones = f.shadowed(crate::Lobe { m: vec![1.0; W * H] }, &vis, 1.0);
    let y = (v * H as f32) as usize;
    ones.m[y * W..(y + 1) * W].to_vec()
}

/// Where the wall's point in line from the light through the box's middle is, as a share of the photo's width.
fn shadow_middle(f: &Field, l: &Light) -> f32 {
    let rig = f.rig(l, 0.0, 0.0);
    let (lp, b) = (rig.position(), f.point(0.45, 0.5, -0.6));
    let zw = 0.2f32.exp();
    let s = (zw - lp[2]) / (b[2] - lp[2]);
    let x = lp[0] + s * (b[0] - lp[0]);
    x / (zw * LENS) + 0.5
}

#[test]
fn a_near_box_shades_the_far_wall_behind_it_and_not_beside_it() {
    let f = box_before_wall();
    let l = light(0.0, LightKind::Point);
    let wall = row(&f, &l, 0.5);
    let mid = shadow_middle(&f, &l);
    assert!(mid > 0.6 && mid < 0.8, "the shadow should fall right of the box, at {mid}");
    let at = |u: f32| wall[(u * W as f32) as usize];
    assert!(at(mid) < 0.1, "the wall at {mid} should be in shadow: {}", at(mid));
    // beside it: left of the box, between the light and the box, and either side of the shadow
    assert!(at(0.05) > 0.95 && at(0.3) > 0.95, "{} {}", at(0.05), at(0.3));
    // ... and above it, where the ray to the light misses the box
    let top = row(&f, &l, 0.04);
    assert!(top[(mid * W as f32) as usize] > 0.95, "{}", top[(mid * W as f32) as usize]);
}

#[test]
fn a_bigger_light_gives_a_wider_penumbra() {
    let f = box_before_wall();
    // (the pixels in the penumbra: neither lit nor blocked)
    let width = |softness: f64| row(&f, &light(softness, LightKind::Point), 0.5).iter().filter(|v| **v > 0.05 && **v < 0.95).count();
    let (small, big) = (width(0.15), width(0.9));
    assert!(big > small + 6, "small light {small} pixels, big light {big}");
    // a softbox is a light with a size too: a big one is softer than a small one
    let boxed = |w: f64, h: f64| {
        let k = LightKind::Softbox(Softbox { width: w, height: h, rotation: 0.0 });
        row(&f, &light(0.5, k), 0.5).iter().filter(|v| **v > 0.05 && **v < 0.95).count()
    };
    assert!(boxed(0.5, 0.5) > boxed(0.03, 0.03) + 6);
}

#[test]
fn no_shadow_amount_changes_nothing_and_half_is_half() {
    let f = box_before_wall();
    let l = light(0.2, LightKind::Point);
    let rig = f.rig(&l, 0.0, 0.0);
    let lobe = f.lobe(&rig);
    let vis = f.visibility(&rig, &l).unwrap();
    let same = f.shadowed(lobe.clone(), &vis, 0.0);
    assert!(same.m.iter().zip(&lobe.m).all(|(a, b)| a.to_bits() == b.to_bits()));
    // half the amount lights a blocked pixel by half
    let (full, half) = (f.shadowed(lobe.clone(), &vis, 1.0), f.shadowed(lobe.clone(), &vis, 0.5));
    let i = (0.5 * H as f32) as usize * W + (shadow_middle(&f, &l) * W as f32) as usize;
    assert!(
        full.m[i] < 0.1 * lobe.m[i] + 1e-6 && (half.m[i] - 0.5 * lobe.m[i]).abs() < 0.06 * lobe.m[i] + 1e-6,
        "{} {} {}",
        lobe.m[i],
        full.m[i],
        half.m[i]
    );
    // a rim and a fill take none
    for k in [LightKind::Rim(crate::Rim { width: 0.5 }), LightKind::Fill(crate::Fill { near: 0.5 })] {
        let l = light(0.2, k);
        assert!(f.visibility(&f.rig(&l, 0.0, 0.0), &l).is_none());
    }
}

#[test]
fn a_sun_casts_parallel_shadows_and_hostile_numbers_give_finite_planes() {
    let f = box_before_wall();
    // a sun from the left and in front: the box shades the wall to its right
    let sun = light(0.1, LightKind::Sun(Sun { azimuth: -30.0, elevation: 10.0 }));
    let wall = row(&f, &sun, 0.5);
    assert!(wall[(0.1 * W as f32) as usize] > 0.95 && wall.iter().any(|v| *v < 0.1), "{:?}", wall.iter().step_by(12).collect::<Vec<_>>());
    // nothing here is a number
    let bad =
        Light { x: f64::NAN, y: f64::INFINITY, depth: -9.0, amount: 1e9, softness: f64::NAN, warmth: f64::NAN, wrap: 7.0, kind: LightKind::Point };
    for kind in [
        LightKind::Point,
        LightKind::Sun(Sun { azimuth: f64::NAN, elevation: f64::INFINITY }),
        LightKind::Softbox(Softbox { width: f64::NAN, height: -1.0, rotation: f64::INFINITY }),
    ] {
        let l = Light { kind, ..bad };
        let rig = f.rig(&l, f64::NAN, f64::NAN);
        let vis = f.visibility(&rig, &l).unwrap();
        for amount in [f64::NAN, 1e9, -3.0, 1.0] {
            let out = f.shadowed(crate::Lobe { m: vec![1.0; W * H] }, &vis, amount);
            assert!(out.m.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{amount}");
        }
    }
    // a visibility of another field's size adds nothing
    let small = Field::from_parts(8, 8, vec![0.0; 64], vec![-2.0; 64]).unwrap();
    let l = light(0.2, LightKind::Point);
    let vis = small.visibility(&small.rig(&l, 0.0, 0.0), &l).unwrap();
    assert!(f.shadowed(crate::Lobe { m: vec![0.5; W * H] }, &vis, 1.0).m.iter().all(|v| *v == 0.5));
}
