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

/// Where the wall's point in line from the light through the middle of the box is, as a share of the photo's width: the
/// box is taken to be as deep as it is wide (`crate::occluder`), so its middle is half its width behind its front.
fn shadow_middle(f: &Field, l: &Light) -> f32 {
    in_line(f, l, (0.45, 0.5), -0.6, 0.1, 0.2)
}

/// Where the point of a wall at log depth `wall` lies (a share of the photo's width) that is in line from light `l` through
/// the middle of a thing whose front is at `(u, v)` and log depth `front` and which is `width` of the photo wide (and so,
/// taken to be as deep as it is wide, reaches `width · LENS` of its depth behind its front).
fn in_line(f: &Field, l: &Light, (u, v): (f32, f32), front: f32, width: f32, wall: f32) -> f32 {
    let rig = f.rig(l, 0.0, 0.0);
    let (lp, b) = (rig.position(), f.point(u, v, front + (1.0 + 0.5 * width * LENS).ln()));
    let zw = wall.exp();
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
        let k = LightKind::Softbox(Softbox { width: w, height: h, rotation: 0.0, aim_h: 0.0, aim_v: 0.0 });
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
    // (a little above the box: its shadow falls lower on the wall than the box itself)
    let wall = row(&f, &sun, 0.65);
    assert!(wall[(0.1 * W as f32) as usize] > 0.95 && wall.iter().any(|v| *v < 0.1), "{:?}", wall.iter().step_by(12).collect::<Vec<_>>());
    // nothing here is a number
    let bad =
        Light { x: f64::NAN, y: f64::INFINITY, depth: -9.0, amount: 1e9, softness: f64::NAN, warmth: f64::NAN, wrap: 7.0, kind: LightKind::Point };
    for kind in [
        LightKind::Point,
        LightKind::Sun(Sun { azimuth: f64::NAN, elevation: f64::INFINITY }),
        LightKind::Softbox(Softbox { width: f64::NAN, height: -1.0, rotation: f64::INFINITY, aim_h: 0.0, aim_v: 0.0 }),
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
    for kind in [LightKind::Point, LightKind::Softbox(Softbox { width: 0.2, height: 0.2, rotation: 0.0, aim_h: 0.0, aim_v: 0.0 })] {
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
fn a_persons_shadow_on_the_floor_in_front_has_no_step_as_the_light_crosses_the_far_edge_of_their_thickness() {
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
    assert!(floor_light(1.25) < 0.3, "a light a quarter behind the person: the floor sees {}", floor_light(1.25));
    let steps: Vec<f32> = (0..=12).map(|i| floor_light(1.3 + 0.05 * i as f32)).collect();
    for pair in steps.windows(2) {
        assert!((pair[1] - pair[0]).abs() <= 0.1, "the floor's light jumps: {steps:?}");
    }
}

/// A subject (`u` 0.45 .. 0.55, `v` 0.3 .. 0.7, depth 1) in front of a wall `wall` times its depth.
fn subject_before_wall(wall: f32) -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (0.45..0.55).contains(&u) && (0.3..0.7).contains(&v) { 0.0 } else { wall.ln() }
        })
        .collect();
    Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap()
}

/// The share of the light the wall right of the subject sees from a point light at `(x, 0.5)` and `k` times the subject's
/// depth: over the rows 0.45 .. 0.55 and `u` from `from` to `to`.
fn wall_light(f: &Field, x: f64, k: f32, (from, to): (f32, f32)) -> f32 {
    let l = point_at(f, x, k);
    let plane = f.visibility_plane(&f.visibility(&f.rig(&l, 0.0, 0.0), &l, false).unwrap());
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in (H * 45 / 100)..(H * 55 / 100) {
        for xx in ((from * W as f32) as usize)..((to * W as f32) as usize).max((from * W as f32) as usize + 1) {
            sum += plane[y * W + xx];
            n += 1.0;
        }
    }
    sum / n
}

/// A small point light at `(x, 0.5)` and `k` times the depth of the subject at log depth 0.
fn point_at(f: &Field, x: f64, k: f32) -> Light {
    let d = f64::from((k.ln() - f.near + 0.6) / (f.far - f.near + 0.6));
    Light { x, y: 0.5, depth: d, ..light(0.05, LightKind::Point) }
}

#[test]
fn a_wall_a_little_behind_a_subject_is_in_its_shadow_from_a_light_at_or_in_front_of_the_subject() {
    // the wall 33 % and 30 % behind the subject, the light to the left at 0.7, 0.9 and 1 times the subject's depth: the way from the
    // wall where the line from the light through the subject lands crosses the subject (round 3 judged a ray by where it starts and
    // left that wall lit)
    for wall in [1.33f32, 1.3] {
        let f = subject_before_wall(wall);
        // (the light nearer the subject on the picture the nearer it is to the subject's depth: a light level with it throws the
        // shadow far to the side)
        for (k, x) in [(0.7, 0.3), (0.9, 0.3), (1.0, 0.4)] {
            let l = point_at(&f, x, k);
            let mid = in_line(&f, &l, (0.5, 0.5), 0.0, 0.1, wall.ln());
            assert!(mid > 0.56 && mid < 0.95, "wall at {wall}, light at {k}: the shadow's middle at {mid}");
            let lit = wall_light(&f, x, k, ((mid - 0.015).max(0.56), mid + 0.015));
            assert!(lit < 0.1, "wall at {wall}, light at {k}: the wall at {mid} sees {lit} of the light");
        }
    }
}

#[test]
fn a_surface_just_behind_a_subject_changes_smoothly_with_its_depth() {
    // a subject 0.2 of the photo tall (so taken to be about 0.15 of its depth deep, fading out to 0.23) with a surface right of it,
    // the light in front of it and to the left: as the surface goes from 1.13 to 1.33 times the subject's depth, past the subject's
    // back, its light must not step by more than 0.1 per 0.01 of depth (round 3's surface stepped at 1.08 times). (From 1.13: the
    // field itself smooths a step under 11.6 % of depth between neighbours away, and changes at once there.)
    let lit: Vec<f32> = (0..=20)
        .map(|i| {
            let f = subject_beside_a_surface(1.13 + 0.01 * i as f32);
            wall_light(&f, 0.3, 0.8, (0.57, 0.63))
        })
        .collect();
    for pair in lit.windows(2) {
        assert!((pair[1] - pair[0]).abs() <= 0.1, "the surface's light steps: {lit:?}");
    }
    // (and the subject does shade it: smooth because it is shaded and then less so, not because there is no shadow)
    assert!(lit.iter().any(|v| *v < 0.9), "the subject casts no shadow on the surface: {lit:?}");
}

/// A subject (`u` 0.45 .. 0.55, `v` 0.4 .. 0.6, depth 1) with a far wall (4 times its depth) to its left, above and below it, so that it
/// stands out, and to its right a surface `near_side` times its depth.
fn subject_beside_a_surface(near_side: f32) -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (0.45..0.55).contains(&u) && (0.4..0.6).contains(&v) {
                0.0
            } else if u < 0.55 {
                4.0f32.ln()
            } else {
                near_side.ln()
            }
        })
        .collect();
    Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap()
}

#[test]
fn a_things_shadow_from_a_small_light_is_dark_in_full() {
    // a box and a pole (a tenth and a twenty-fifth of the photo wide) in front of a wall, lit from the left by a small point
    // light, a small softbox and a sun: at Cast Shadows 100 the middle of the shadow keeps at most a tenth of the light (round 4
    // faded a ray at its samples' place in a thing's depth and left a third more, and lost a thin thing's shadow)
    for width in [0.1f32, 0.04] {
        let lz: Vec<f32> = (0..W * H)
            .map(|i| {
                let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
                // (tall enough to be more than 2 % of the photo: the nearest depth a light can take is under its own)
                if (0.45..0.45 + width).contains(&u) && (0.2..0.8).contains(&v) { -0.6 } else { 0.2 }
            })
            .collect();
        let f = Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap();
        // (the light at 0.6 times the thing's depth, left of it)
        let depth = f64::from(((0.6f32).ln() - 0.6 - f.near + 0.6) / (f.far - f.near + 0.6));
        for kind in [
            LightKind::Point,
            LightKind::Softbox(Softbox { width: 0.03, height: 0.03, rotation: 0.0, aim_h: 0.0, aim_v: 0.0 }),
            LightKind::Sun(Sun { azimuth: -40.0, elevation: 0.0 }),
        ] {
            let l = Light { depth, ..light(0.05, kind) };
            let rig = f.rig(&l, 0.0, 0.0);
            // (where the shadow's middle lands: in line from the light, or along the sun's direction, through the thing's middle)
            let mid = match kind {
                LightKind::Sun(_) => {
                    let b = f.point(0.45 + width / 2.0, 0.5, -0.6 + (1.0 + 0.5 * width * LENS).ln());
                    let to = crate::kinds::sun_vector(-40.0, 0.0);
                    let zw = 0.2f32.exp();
                    let x = b[0] - to[0] * (zw - b[2]) / -to[2];
                    x / (zw * LENS) + 0.5
                }
                _ => in_line(&f, &l, (0.45 + width / 2.0, 0.5), -0.6, width, 0.2),
            };
            assert!(mid > 0.45 + width && mid < 0.95, "{kind:?}, width {width}: the shadow's middle at {mid}");
            for draft in [false, true] {
                let vis = f.visibility(&rig, &l, draft).unwrap();
                let lobe = f.lobe(&rig);
                let shaded = f.shadowed(lobe.clone(), &vis, 1.0);
                let i = (H / 2) * W + (mid * W as f32) as usize;
                let kept = shaded.m[i] / lobe.m[i].max(1e-9);
                assert!(kept <= 0.1, "{kind:?}, width {width}, draft {draft}: the shadow's middle at {mid} keeps {kept} of the light");
            }
        }
    }
}

#[test]
fn a_thin_poles_shadow_is_whole_along_its_length() {
    // a pole two pixels wide on the 400-pixel grid the march runs on (one on the drag's 200), 1.4 times nearer than the wall,
    // lit from its left by the smallest point light: every row of its shadow is about as dark as the darkest (a ray that steps
    // over the pole sideways left rows of it lit: a dashed shadow)
    let (w, h) = (400usize, 256usize);
    let wall = 1.4f32.ln() - 0.6;
    let lz: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, v) = (i % w, (i / w) as f32 / h as f32);
            // (and a near patch in a corner, out of the way, so the light can come in front of the pole)
            if ((200..202).contains(&x) && (0.2..0.8).contains(&v)) || (x < w / 4 && v >= 0.9) { -0.6 } else { wall }
        })
        .collect();
    let f = Field::from_parts(w, h, lz, vec![-2.0; w * h]).unwrap();
    let l = Light { x: 0.3, y: 0.5, depth: 0.0, ..light(0.0, LightKind::Point) };
    let rig = f.rig(&l, 0.0, 0.0);
    for draft in [false, true] {
        let plane = f.visibility_plane(&f.visibility(&rig, &l, draft).unwrap());
        // the darkest of each row's pixels right of the pole, over the rows the pole's shadow covers
        let rows: Vec<f32> = (h * 30 / 100..h * 70 / 100).map(|y| plane[y * w + 202..y * w + 240].iter().copied().fold(1.0, f32::min)).collect();
        let (lo, hi) = (rows.iter().copied().fold(1.0, f32::min), rows.iter().copied().fold(0.0, f32::max));
        assert!(hi <= 0.5 && hi - lo <= 0.15, "draft {draft}: the shadow's darkest per row from {lo} to {hi}: {rows:?}");
    }
}
