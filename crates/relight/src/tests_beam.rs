//! Beams through haze (issue #317): a nearer subject hides the beam behind it, a blocker cuts a dark shaft in the beam,
//! forward scatter makes a light behind the scene glow more, and hostile numbers stay finite.

use crate::{Effects, Field, Light, LightKind, Sun};

const W: usize = 192;
const H: usize = 128;

/// A far wall (log depth 0.4) with a near box (log depth `front`) over `u` and `v`, and a small patch as near in the
/// bottom left corner (so every scene has the same nearest depth: the air begins at a share of it).
fn scene(boxed: Option<(std::ops::Range<f32>, std::ops::Range<f32>, f32)>) -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            match &boxed {
                Some((us, vs, front)) if us.contains(&u) && vs.contains(&v) => *front,
                Some((_, _, front)) if u < 0.16 && v > 0.84 => *front,
                _ => 0.4,
            }
        })
        .collect();
    // (the far wall a little brighter than what stands before it: it is the open sky the air's rays go out through)
    let lum = lz.iter().map(|l| if *l >= 0.4 { -2.0 } else { -3.0 }).collect();
    Field::from_parts(W, H, lz, lum).unwrap()
}

fn haze(scatter: f64) -> Effects {
    Effects { haze: 0.6, scatter, ..Effects::default() }
}

/// The beam of `l` with effects `e` on `f`, read at normalized `(u, v)` (the reduced field is the field here).
fn beam(f: &Field, l: &Light, e: &Effects) -> impl Fn(f32, f32) -> f32 + use<> {
    let rig = f.rig(l, 0.0, 0.0);
    let shafts = f.shafts(&rig, l, false).unwrap();
    let b = f.beam(&shafts, &rig, e);
    let (w, h) = b.size();
    move |u: f32, v: f32| b.at((u * w as f32) as usize, (v * h as f32) as usize)
}

fn point(x: f64, y: f64, depth: f64) -> Light {
    Light { x, y, depth, amount: 1.0, softness: 0.2, warmth: 0.0, wrap: 0.4, kind: LightKind::Point }
}

#[test]
fn a_nearer_subject_hides_the_beam_behind_it() {
    // a light in the gap between the subject (log depth −0.6) and the wall (0.4), behind the subject's middle: the air
    // lit round the light lies behind the subject, so the subject's own pixels show none of it, and the wall's beside
    // it, farther from the light on the picture, show it
    let f = scene(Some((0.35..0.65, 0.25..0.75, -0.6)));
    // (the light's log depth is near − 0.6 + depth · (far − near + 0.6): 0 here, behind the subject's back)
    let at = beam(&f, &point(0.5, 0.5, 0.75), &haze(0.0));
    let (subject, wall) = (at(0.62, 0.5), at(0.68, 0.5));
    assert!(wall > 0.0 && wall > 5.0 * subject, "subject {subject}, wall beside it {wall}");
    // and the wall's pixels round the subject glow more the nearer they are to the light on the picture
    assert!(at(0.68, 0.5) > at(0.9, 0.5), "{} {}", at(0.68, 0.5), at(0.9, 0.5));
}

#[test]
fn a_blocker_cuts_a_dark_shaft_in_the_beam() {
    // a low sun from behind the scene and above, shining towards the camera: the air in front of a box and under it lies in
    // the box's shadow, so the beam over the wall under the box is darker than with no box
    let sun = Light { kind: LightKind::Sun(Sun { azimuth: 180.0, elevation: 12.0 }), softness: 0.0, ..point(0.5, 0.5, 0.5) };
    let boxed = scene(Some((0.25..0.75, 0.15..0.5, -0.8)));
    let wall = scene(Some((0.0..0.0, 0.0..0.0, -0.8)));
    let (with, without) = (beam(&boxed, &sun, &haze(0.0)), beam(&wall, &sun, &haze(0.0)));
    let mean = |b: &dyn Fn(f32, f32) -> f32, v: f32| (0..10).map(|i| b(0.4 + 0.02 * i as f32, v)).sum::<f32>() / 10.0;
    let ratio = |v: f32| mean(&with, v) / mean(&without, v).max(1e-9);
    assert!(mean(&without, 0.55) > 0.0 && ratio(0.55) < 0.75, "under the box {} of the beam with no box", ratio(0.55));
    // the shaft is the box's shadow through the air: it fades away from the box
    assert!(ratio(0.95) > ratio(0.55) + 0.1, "{} far under the box, {} just under it", ratio(0.95), ratio(0.55));
}

#[test]
fn forward_scatter_brightens_a_beam_from_behind_and_dims_one_from_the_front() {
    let f = scene(Some((0.1..0.3, 0.3..0.7, -0.6)));
    // behind: in the gap in front of the wall, so its light travels towards the camera
    let behind = point(0.6, 0.5, 0.75);
    let (even, onward) = (beam(&f, &behind, &haze(0.0)), beam(&f, &behind, &haze(0.8)));
    assert!(onward(0.65, 0.5) > 1.5 * even(0.65, 0.5), "{} {}", onward(0.65, 0.5), even(0.65, 0.5));
    // a sun from behind the scene shines towards the camera, one from the camera's side away from it
    let sun = |azimuth: f64| Light { kind: LightKind::Sun(Sun { azimuth, elevation: 20.0 }), softness: 0.0, ..behind };
    let (even, onward) = (beam(&f, &sun(180.0), &haze(0.0)), beam(&f, &sun(180.0), &haze(0.8)));
    assert!(onward(0.65, 0.5) > 1.5 * even(0.65, 0.5), "{} {}", onward(0.65, 0.5), even(0.65, 0.5));
    let (even, onward) = (beam(&f, &sun(0.0), &haze(0.0)), beam(&f, &sun(0.0), &haze(0.8)));
    assert!(onward(0.65, 0.5) < 0.5 * even(0.65, 0.5), "{} {}", onward(0.65, 0.5), even(0.65, 0.5));
}

#[test]
fn no_haze_no_beam_and_hostile_numbers_stay_finite() {
    let f = scene(Some((0.35..0.65, 0.25..0.75, -0.6)));
    let l = point(0.5, 0.5, 0.75);
    let rig = f.rig(&l, 0.0, 0.0);
    let shafts = f.shafts(&rig, &l, false).unwrap();
    let none = f.beam_plane(&f.beam(&shafts, &rig, &Effects::default()));
    assert!(none.m.iter().all(|v| *v == 0.0));
    // a rim and a fill have no place: no beam
    assert!(f.shafts(&rig, &Light { kind: LightKind::Fill(crate::Fill { near: 0.5 }), ..l }, false).is_none());
    let bad = Effects { haze: f64::INFINITY, scatter: f64::NAN, dust: 1e30, ..Effects::default() };
    for light in [
        Light { x: f64::NAN, y: 1e30, depth: f64::NEG_INFINITY, softness: f64::NAN, ..l },
        Light { kind: LightKind::Sun(Sun { azimuth: f64::NAN, elevation: -1e9 }), ..l },
    ] {
        for draft in [false, true] {
            let rig = f.rig(&light, 0.0, f64::NAN);
            let shafts = f.shafts(&rig, &light, draft).unwrap();
            let lobe = f.beam_plane(&f.beam(&shafts, &rig, &bad));
            assert!(lobe.m.len() == W * H && lobe.m.iter().all(|v| v.is_finite() && *v >= 0.0));
        }
    }
    // dust varies the beam, fixed to the scene: the same numbers make the same beam
    let dusty = Effects { dust: 1.0, ..haze(0.5) };
    let (a, b, plain) = (beam(&f, &l, &dusty), beam(&f, &l, &dusty), beam(&f, &l, &haze(0.5)));
    assert!((0..20).all(|i| a(0.7 + 0.01 * i as f32, 0.4) == b(0.7 + 0.01 * i as f32, 0.4)));
    assert!((0..20).any(|i| (a(0.7 + 0.01 * i as f32, 0.4) - plain(0.7 + 0.01 * i as f32, 0.4)).abs() > 1e-4));
}

#[test]
fn a_wall_with_a_window_lets_a_shaft_through_and_the_air_beside_it_stays_clear() {
    // a wall facing the camera (log depth 0) with a window onto open sky (log depth 0.8), and a sun behind the wall and
    // above it: the air in front of the wall is lit only where its ray to the sun goes out through the window, a shaft
    // falling forwards and down from it; beside the shaft the wall holds the light back, so that air gets none
    let (lz, lum): (Vec<f32>, Vec<f32>) = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (0.4..0.6).contains(&u) && (0.15..0.4).contains(&v) { (0.8, 1.0) } else { (0.0, -3.0) }
        })
        .unzip();
    let f = Field::from_parts(W, H, lz, lum).unwrap();
    let sun = Light { kind: LightKind::Sun(Sun { azimuth: 180.0, elevation: 25.0 }), softness: 0.0, ..point(0.5, 0.5, 0.5) };
    let at = beam(&f, &sun, &haze(0.5));
    let column = |u: f32| (0..8).map(|i| at(u, 0.5 + 0.05 * i as f32)).fold(0f32, f32::max);
    let (shaft, beside) = (column(0.5), column(0.15).max(column(0.85)));
    assert!(shaft > 0.0 && shaft > 6.0 * beside, "in the shaft {shaft}, beside it {beside}");
}

/// The beam of light `l` with `e` on field `f` over its whole reduced field: its width, height and values.
fn whole(f: &Field, l: &Light, e: &Effects) -> (usize, usize, Vec<f32>) {
    let rig = f.rig(l, 0.0, 0.0);
    let shafts = f.shafts(&rig, l, false).unwrap();
    let b = f.beam(&shafts, &rig, e);
    let (w, h) = b.size();
    (w, h, (0..w * h).map(|i| b.at(i % w, i / w)).collect())
}

#[test]
fn a_near_subject_takes_almost_no_haze_from_a_sun_behind_the_scene() {
    // a dark backdrop with a bright gap onto the sky near the top, a near subject in the lower middle, and a sun behind and
    // above: light reaches the air only through the gap, in a shaft falling forwards and down, so the air in front of the
    // subject and of the dark backdrop elsewhere gets next to none
    let (lz, lum): (Vec<f32>, Vec<f32>) = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (0.4..0.6).contains(&u) && (0.1..0.25).contains(&v) {
                (0.9, 1.0)
            } else if (0.3..0.7).contains(&u) && v > 0.45 {
                (-0.8, -3.0)
            } else {
                (0.6, -3.0)
            }
        })
        .unzip();
    let f = Field::from_parts(W, H, lz, lum).unwrap();
    let sun = Light { kind: LightKind::Sun(Sun { azimuth: 180.0, elevation: 25.0 }), softness: 0.0, ..point(0.5, 0.5, 0.5) };
    let (w, h, s) = whole(&f, &sun, &haze(0.5));
    let top = s.iter().copied().fold(0f32, f32::max);
    let at = |u: f32, v: f32| s[(v * h as f32) as usize * w + (u * w as f32) as usize];
    assert!(top > 0.0, "no shaft at all");
    for (u, v) in [(0.5, 0.75), (0.35, 0.9), (0.1, 0.3), (0.9, 0.6)] {
        // (round 5: the shaft's soft edge, fanning out towards the camera, reaches the backdrop's far side at 3.2 %)
        assert!(at(u, v) < 0.04 * top, "({u}, {v}) takes {} of a shaft's {top}", at(u, v));
    }
}

#[test]
fn a_spots_beam_is_brightest_near_its_source_and_widens_along_its_axis() {
    // a spot high in the middle in front of a far wall, aimed straight down the wall: across the beam, it is brighter near
    // the light than far down its axis, and wider far down than near
    let f = scene(Some((0.0..0.0, 0.0..0.0, -0.6)));
    let spot =
        Light { kind: LightKind::Spot(crate::Spot { aim_x: 0.5, aim_y: 0.95, angle: 15.0, edge: 0.6 }), softness: 0.2, ..point(0.5, 0.05, 0.6) };
    let (w, h, s) = whole(&f, &spot, &haze(0.0));
    let row = |v: f32| -> Vec<f32> {
        let y = (v * h as f32) as usize;
        s[y * w..(y + 1) * w].to_vec()
    };
    let (near, far) = (row(0.2), row(0.8));
    let peak = |r: &[f32]| r.iter().copied().fold(0f32, f32::max);
    let width = |r: &[f32]| r.iter().filter(|v| **v > 0.3 * peak(r)).count();
    assert!(peak(&near) > 1.5 * peak(&far), "near the source {}, far down {}", peak(&near), peak(&far));
    assert!(width(&far) > width(&near) + 3, "{} pixels wide near the source, {} far down", width(&near), width(&far));
}

#[test]
fn a_sun_through_window_panes_makes_shafts_of_the_windows_shape() {
    // a low sun from behind through Window Panes, the camera looking along its shafts: the air's light takes the panes and
    // bars, so across the picture the beam goes bright and dark against the same sun's beam with no pattern
    let f = scene(Some((0.0..0.0, 0.0..0.0, -0.6)));
    let sun = Light { kind: LightKind::Sun(Sun { azimuth: 160.0, elevation: 20.0 }), softness: 0.0, ..point(0.5, 0.3, 0.3) };
    let panes = Effects { pattern: crate::Pattern::Panes, scale: 1.0, softness: 0.1, ..haze(0.5) };
    let row = |e: &Effects| {
        let (w, h, s) = whole(&f, &sun, e);
        let y = h / 2;
        s[y * w + w / 10..y * w + 9 * w / 10].to_vec()
    };
    // (each pixel's beam through the panes over its beam with no pattern: the panes alone)
    let (panes, plain) = (row(&panes), row(&haze(0.5)));
    let q: Vec<f32> = panes.iter().zip(&plain).filter(|(_, b)| **b > 1e-4).map(|(a, b)| a / b).collect();
    let (hi, lo) = (q.iter().copied().fold(0f32, f32::max), q.iter().copied().fold(f32::MAX, f32::min));
    assert!(q.len() > 50 && hi > 1.25 * lo, "through the panes the beam runs {lo} .. {hi} of the plain one");
}

#[test]
fn a_narrow_gaps_shaft_is_wider_and_dimmer_far_from_the_gap() {
    // a wall facing the camera (log depth 0) with a narrow slit onto open sky, the sun straight behind it: across the shaft
    // that comes through, the air's light has soft edges that widen with the distance from the slit (the sun's disc and the
    // haze spread them), so far from it the shaft is wider and its middle no longer gets all of the light
    let (lz, lum): (Vec<f32>, Vec<f32>) = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if (0.48..0.52).contains(&u) && (0.2..0.8).contains(&v) { (0.8, 1.0) } else { (0.0, -3.0) }
        })
        .unzip();
    let f = Field::from_parts(W, H, lz, lum).unwrap();
    let sun = Light { kind: LightKind::Sun(Sun { azimuth: 180.0, elevation: 0.0 }), softness: 0.0, ..point(0.5, 0.5, 0.5) };
    let rig = f.rig(&sun, 0.0, 0.0);
    let march = f.march(&rig, &sun, false).unwrap();
    // (the air's light across the shaft, at depth `z`: the slit's middle is straight in front of the camera)
    let across = |z: f32| -> Vec<f32> { (-50..=50).map(|i| march.air_seen([0.002 * i as f32, 0.0, z], 0, 64, 0.0)).collect() };
    let (near, far) = (across(0.9), across(0.4));
    let peak = |p: &[f32]| p.iter().copied().fold(0f32, f32::max);
    let width = |p: &[f32]| p.iter().filter(|v| **v > 0.05).count();
    assert!(peak(&near) > 0.9, "next to the slit the shaft's middle gets {}", peak(&near));
    assert!(width(&far) > width(&near) + 6, "{} samples wide next to the slit, {} far from it", width(&near), width(&far));
    assert!(peak(&far) < 0.9 * peak(&near), "the shaft's middle gets {} far from the slit, {} next to it", peak(&far), peak(&near));
}

/// The beam over the whole reduced field, as a drag (`draft`) or a release makes it.
fn whole_as(f: &Field, l: &Light, e: &Effects, draft: bool) -> (usize, usize, Vec<f32>) {
    let rig = f.rig(l, 0.0, 0.0);
    let shafts = f.shafts(&rig, l, draft).unwrap();
    let b = f.beam(&shafts, &rig, e);
    let (w, h) = b.size();
    (w, h, (0..w * h).map(|i| b.at(i % w, i / w)).collect())
}

#[test]
fn a_drags_beam_matches_the_releases_and_stays_on_its_side_of_an_outline() {
    // a bank of leaves (log depth about 0, rippling from pixel to pixel by up to ±0.15) with three tall gaps onto open sky,
    // a near subject in the lower middle with a soft outline, and a sun behind and above: shafts fall forwards and down from the gaps across
    // the subject's outline. The drag's coarser beam is the release's away from the outline, and along it neither spills
    // onto the subject nor leaves a dark rim on the leaves
    let lz_at = |x: usize, y: usize| -> f32 {
        let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
        // (the subject's outline is soft, as a depth map's is: from the leaves' depth to its own over 4 pixels)
        let inside = ((u - 0.35).min(0.65 - u).min(v - 0.5) * W as f32 / 4.0).clamp(0.0, 1.0);
        if inside > 0.0 {
            -0.8 * inside
        } else if v < 0.3 && [0.2f32, 0.45, 0.7].iter().any(|a| (*a..a + 0.1).contains(&u)) {
            0.8
        } else {
            0.15 * (crate::shadow::spread_noise(x, y) * 2.0 - 1.0)
        }
    };
    let (lz, lum): (Vec<f32>, Vec<f32>) = (0..W * H)
        .map(|i| {
            let z = lz_at(i % W, i / W);
            (z, if z > 0.5 { 1.0 } else { -3.0 })
        })
        .unzip();
    let f = Field::from_parts(W, H, lz.clone(), lum).unwrap();
    let sun = Light { kind: LightKind::Sun(Sun { azimuth: 170.0, elevation: 20.0 }), softness: 0.2, ..point(0.5, 0.5, 0.5) };
    let e = haze(0.7);
    let (w, h, full) = whole_as(&f, &sun, &e, false);
    let (_, _, drag) = whole_as(&f, &sun, &e, true);
    assert_eq!((w, h), (W, H));
    // (which thing each pixel is on: the leaves, a gap, the subject)
    let z = |x: usize, y: usize| {
        let l = lz.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or(0.0);
        if l > 0.5 {
            1.0
        } else if l < -0.5 {
            -1.0
        } else {
            0.0
        }
    };
    // (how far each pixel is from an outline, in pixels, up to 10)
    let gap = |x: usize, y: usize| -> usize {
        (1..10)
            .find(|r| {
                let (x0, y0, x1, y1) = (x.saturating_sub(*r), y.saturating_sub(*r), x + r, y + r);
                [(x0, y), (x1, y), (x, y0), (x, y1)].iter().any(|(a, b)| z(*a, *b) != z(x, y))
            })
            .unwrap_or(10)
    };
    let wall: Vec<usize> = (0..w * h).filter(|i| z(i % w, i / w) == 0.0).collect();
    let mean_wall = wall.iter().map(|i| full[*i]).sum::<f32>() / wall.len() as f32;
    assert!(mean_wall > 0.0, "no beam");
    let mean_diff = |pick: &dyn Fn(usize) -> bool| {
        let px: Vec<usize> = (0..w * h).filter(|i| pick(*i)).collect();
        px.iter().map(|i| (drag[*i] - full[*i]).abs()).sum::<f32>() / px.len().max(1) as f32
    };
    let away = mean_diff(&|i| gap(i % w, i / w) >= 6);
    // (round 4's drag: 9.8 % away from the outlines and 4.7 % along the leaves' side of the subject's; round 5's 7.5 % and 2.2 %)
    assert!(away < 0.085 * mean_wall, "away from the outlines the drag differs by {away}, the wall's beam is {mean_wall}");
    // along the subject's outline, on both sides
    let near_subject = |i: usize| (0.3..0.7).contains(&((i % w) as f32 / w as f32)) && i / w > h * 2 / 5;
    let wall_edge = mean_diff(&|i| near_subject(i) && z(i % w, i / w) == 0.0 && gap(i % w, i / w) <= 2);
    let subject_edge = mean_diff(&|i| near_subject(i) && z(i % w, i / w) < 0.0 && gap(i % w, i / w) <= 2);
    assert!(wall_edge < 0.035 * mean_wall, "the wall along the outline is off by {wall_edge} (beam {mean_wall})");
    assert!(subject_edge < 0.035 * mean_wall, "the subject along the outline is off by {subject_edge} (beam {mean_wall})");
}
