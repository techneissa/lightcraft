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
    let vis = f.visibility(&rig, l, false).unwrap();
    let ones = f.shadowed(crate::Lobe { m: vec![1.0; W * H] }, &vis, 1.0);
    let y = (v * H as f32) as usize;
    ones.m[y * W..(y + 1) * W].to_vec()
}

/// Where the wall's point in line from the light through the middle of the box (halfway through its assumed thickness) is, as a share of the photo's width.
fn shadow_middle(f: &Field, l: &Light) -> f32 {
    let rig = f.rig(l, 0.0, 0.0);
    let (lp, b) = (rig.position(), f.point(0.45, 0.5, -0.6 + 1.25f32.ln()));
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
    assert!(mid > 0.5 && mid < 0.8, "the shadow should fall right of the box, at {mid}");
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
    let vis = f.visibility(&rig, &l, false).unwrap();
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
    for k in [LightKind::Rim(crate::Rim::default()), LightKind::Fill(crate::Fill { near: 0.5 })] {
        let l = light(0.2, k);
        assert!(f.visibility(&f.rig(&l, 0.0, 0.0), &l, false).is_none());
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
        let vis = f.visibility(&rig, &l, false).unwrap();
        for amount in [f64::NAN, 1e9, -3.0, 1.0] {
            let out = f.shadowed(crate::Lobe { m: vec![1.0; W * H] }, &vis, amount);
            assert!(out.m.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{amount}");
        }
    }
    // a visibility of another field's size adds nothing
    let small = Field::from_parts(8, 8, vec![0.0; 64], vec![-2.0; 64]).unwrap();
    let l = light(0.2, LightKind::Point);
    let vis = small.visibility(&small.rig(&l, 0.0, 0.0), &l, false).unwrap();
    assert!(f.shadowed(crate::Lobe { m: vec![0.5; W * H] }, &vis, 1.0).m.iter().all(|v| *v == 0.5));
}

/// The share of the wall (the pixels outside the box and a margin round it) that sees at least 90 % of the light, with the
/// visibility made as a drag's draft or in full.
fn lit_wall(f: &Field, l: &Light, draft: bool) -> f32 {
    let rig = f.rig(l, 0.0, 0.0);
    let vis = f.visibility(&rig, l, draft).unwrap();
    let v = f.shadowed(crate::Lobe { m: vec![1.0; W * H] }, &vis, 1.0).m;
    let wall: Vec<f32> = (0..W * H)
        .filter(|i| {
            let (u, vv) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            !((0.35..0.6).contains(&u) && (0.25..0.75).contains(&vv))
        })
        .map(|i| v[i])
        .collect();
    wall.iter().filter(|s| **s > 0.9).count() as f32 / wall.len() as f32
}

#[test]
fn a_light_a_little_behind_the_subject_does_not_shade_the_whole_wall() {
    // the light at depth 0.6 lies inside the near box's assumed thickness at its own pixel (the box is 0.55 deep, the light
    // 0.70): every ray to it ran behind that surface and the whole wall went dark
    let f = box_before_wall();
    for kind in [LightKind::Point, LightKind::Softbox(Softbox { width: 0.2, height: 0.2, rotation: 0.0 })] {
        let l = Light { x: 0.5, y: 0.5, depth: 0.6, ..light(0.2, kind) };
        for draft in [false, true] {
            let lit = lit_wall(&f, &l, draft);
            assert!(lit > 0.8, "{kind:?} draft {draft}: only {lit} of the wall sees the light");
        }
    }
}

#[test]
fn a_drags_coarse_shadows_fall_where_the_full_ones_do() {
    let f = box_before_wall();
    let l = light(0.0, LightKind::Point);
    let (rig, mid) = (f.rig(&l, 0.0, 0.0), shadow_middle(&f, &l));
    let vis = f.visibility(&rig, &l, true).unwrap();
    let wall = f.shadowed(crate::Lobe { m: vec![1.0; W * H] }, &vis, 1.0).m;
    let at = |u: f32| wall[(0.5 * H as f32) as usize * W + (u * W as f32) as usize];
    assert!(at(mid) < 0.1 && at(0.05) > 0.95 && at(0.3) > 0.95, "{} {} {}", at(mid), at(0.05), at(0.3));
}

/// A receding stepped floor (a plane under the camera, its depth quantized into 16 terraces, from 4 at the horizon's foot down to 0.45 at the
/// bottom of the picture) with a near box 0.5 nearer in log depth than the floor under it, over `u` 0.42 .. 0.58 and `v` 0.7 .. 0.9. The field
/// is 192 × 128. Returns the field and the box's log depth.
fn stairs_and_box() -> (Field, f32) {
    let ky = crate::field::LENS * H as f32 / W as f32;
    let floor = |v: f32| ((0.173 / ((v.max(0.56) - 0.5) * ky)).ln() * 16.0).round() / 16.0;
    let box_lz = floor(0.8) - 0.5;
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (0.42..0.58).contains(&u) && (0.7..0.9).contains(&v) { box_lz } else { floor(v) }
        })
        .collect();
    (Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap(), box_lz)
}

/// A pole leaning out of a far wall: a bar over `u` 0.4 .. 0.46 whose log depth is 0.4 nearer than the wall (0.2) at the bottom of the picture
/// and 1.1 nearer at the top, so its shadow on the wall (from a light off to the left) starts narrow and softens as it goes.
fn leaning_pole() -> Field {
    // (800 × 512: the march runs on a grid of half that, as a real photo's does of a quarter)
    let (w, h) = (800, 512);
    let lz: Vec<f32> = (0..w * h)
        .map(|i| {
            let (u, v) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            if (0.4..0.46).contains(&u) { 0.2 - 0.4 - 0.7 * (1.0 - v) } else { 0.2 }
        })
        .collect();
    Field::from_parts(w, h, lz, vec![-2.0; w * h]).unwrap()
}

#[test]
fn a_shadows_edge_is_sharp_where_it_leaves_the_object_and_softens_with_distance() {
    let f = leaning_pole();
    let l = Light { x: 0.0, y: 0.5, depth: 0.0, wrap: 1.0, ..light(0.5, LightKind::Point) };
    let rig = f.rig(&l, 0.0, 0.0);
    let vis = f.visibility(&rig, &l, false).unwrap();
    let plane = f.visibility_plane(&vis);
    // the penumbra of the shadow's right edge in the row at `v`: the pixels neither lit nor blocked right of the darkest one
    let width = |v: f32| -> usize {
        let (w, h) = (f.width, f.height);
        let y = (v * h as f32) as usize;
        let row = &plane[y * w..(y + 1) * w];
        let (dark, _) = row.iter().enumerate().skip(w * 2 / 5).fold((0, 2.0f32), |m, (x, s)| if *s < m.1 { (x, *s) } else { m });
        row.iter().skip(dark).take_while(|s| **s < 0.95).filter(|s| **s > 0.05).count()
    };
    let (near_the_wall, far_from_it) = (width(0.9), width(0.15));
    assert!(far_from_it > 2 * near_the_wall + 4, "penumbra {near_the_wall} pixels where the pole is near the wall, {far_from_it} where it is far");
}

#[test]
fn a_light_behind_a_near_box_keeps_the_stepped_floor_lit_outside_the_boxs_shadow() {
    let (f, blz) = stairs_and_box();
    let ((bu0, bu1), (bv0, bv1)) = ((0.42f32, 0.58f32), (0.7f32, 0.9f32));
    // the light over the box, a little behind its front (inside the thickness a thing is taken to have)
    let d = f64::from((blz + 0.25 - f.near + 0.6) / (f.far - f.near + 0.6));
    for (softness, draft) in [(0.05, false), (0.5, false), (0.5, true)] {
        let l = Light { x: 0.5, y: 0.8, depth: d, ..light(softness, LightKind::Point) };
        let vis = f.visibility(&f.rig(&l, 0.0, 0.0), &l, draft).unwrap();
        let plane = f.visibility_plane(&vis);
        let (mut outside, mut lit) = (0usize, 0usize);
        for y in 0..H {
            for x in 0..W {
                let (u, v) = ((x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32);
                // (the box's geometric shadow: where the way to the light crosses the box on the picture, a margin round it)
                let shaded = (1..=40).any(|k| {
                    let t = 0.8 * k as f32 / 40.0;
                    let (pu, pv) = (u + (0.5 - u) * t, v + (0.8 - v) * t);
                    (bu0 - 0.04..bu1 + 0.04).contains(&pu) && (bv0 - 0.04..bv1 + 0.04).contains(&pv)
                });
                if shaded {
                    continue;
                }
                outside += 1;
                // (0.1 stop: 0.933 of the light)
                lit += usize::from(plane[y * W + x] >= 0.933);
            }
        }
        let share = lit as f32 / outside as f32;
        assert!(share >= 0.95, "softness {softness}, draft {draft}: only {share} of the floor outside the box's shadow keeps its light");
    }
}

/// A person (`u` 0.45 .. 0.55, `v` 0.2 .. 0.7, depth 1) in front of a wall (depth 4) over a floor in front of them (the rows from `v` 0.7 down,
/// depth 0.5).
fn person_and_floor() -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if v >= 0.7 {
                0.5f32.ln()
            } else if (0.45..0.55).contains(&u) && v >= 0.2 {
                0.0
            } else {
                4.0f32.ln()
            }
        })
        .collect();
    Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap()
}

#[test]
fn a_lights_shadow_of_a_person_changes_smoothly_as_the_light_crosses_the_edge_of_their_thickness() {
    let f = person_and_floor();
    // the mean share of the light the floor under the person (the rows from 0.75 down, `u` 0.42 .. 0.58) sees, for a light over the
    // person at `k` times their depth
    let floor_light = |k: f32| -> f32 {
        let d = f64::from((k.ln() - f.near + 0.6) / (f.far - f.near + 0.6));
        let l = Light { x: 0.5, y: 0.4, depth: d, ..light(0.05, LightKind::Point) };
        let vis = f.visibility(&f.rig(&l, 0.0, 0.0), &l, false).unwrap();
        let plane = f.visibility_plane(&vis);
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in (H * 3 / 4)..H {
            for x in (W * 42 / 100)..(W * 58 / 100) {
                sum += plane[y * W + x];
                n += 1.0;
            }
        }
        sum / n
    };
    // (just behind the person the floor in front of them is in their shadow, and the light's depth crossing 1.5 times theirs, the edge of
    // the thickness a thing is taken to have, changes that by no more than 0.1 in any step of 0.05, from 1.3 to 1.9 times)
    assert!(floor_light(1.15) < 0.3, "a light just behind the person: the floor sees {}", floor_light(1.15));
    let steps: Vec<f32> = (0..=12).map(|i| floor_light(1.3 + 0.05 * i as f32)).collect();
    for pair in steps.windows(2) {
        assert!((pair[1] - pair[0]).abs() <= 0.1, "the floor's light jumps: {steps:?}");
    }
}
