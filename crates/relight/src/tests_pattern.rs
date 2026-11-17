//! Patterns as real light through real things (issue #316): each test is one realism rule a mistake would break.

use crate::effects::{Effects, Pattern};
use crate::field::Field;
use crate::kinds::LightKind;
use crate::light::Light;

const W: usize = 192;
const H: usize = 128;

fn flat(lz: impl Fn(usize, usize) -> f32) -> Field {
    let lz: Vec<f32> = (0..W * H).map(|i| lz(i % W, i / W)).collect();
    #[allow(clippy::unwrap_used)]
    Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap()
}

fn light() -> Light {
    Light { x: 0.5, y: 0.5, depth: 0.0, amount: 1.0, softness: 0.5, warmth: 0.0, wrap: 0.4, kind: LightKind::Point }
}

const ALL: [Pattern; 4] = [Pattern::Blinds, Pattern::Panes, Pattern::Leaves, Pattern::Dapple];

fn light_plane(f: &Field, l: &Light, e: &Effects) -> Vec<f32> {
    f.pattern_planes(&f.rig(l, 0.0, 0.0), e, false).map(|p| p.light).unwrap_or_default()
}

/// The row indices where the plane at column `x` crosses 0.5 going up or down.
fn edges(plane: &[f32], x: usize) -> Vec<usize> {
    (1..H).filter(|&y| (plane[(y - 1) * W + x] > 0.5) != (plane[y * W + x] > 0.5)).collect()
}

#[test]
fn a_pattern_stays_inside_its_lights_opening() {
    // a wall facing the camera, the light in the middle: the window (or the canopy's gap) is lit in a pattern, the wall
    // round it gets none of the light; a pattern used to fill the whole photo like wallpaper
    let wall = flat(|_, _| 0.0);
    for pattern in ALL {
        let plane = light_plane(&wall, &light(), &Effects { pattern, softness: 0.1, ..Effects::default() });
        let at = |x: usize, y: usize| plane[y * W + x];
        let corners = [(2, 2), (W - 3, 2), (2, H - 3), (W - 3, H - 3)];
        assert!(corners.iter().all(|&(x, y)| at(x, y) < 0.02), "{pattern:?}: the corners are lit {:?}", corners.map(|(x, y)| at(x, y)));
        let inside: Vec<f32> = (H / 2 - 30..H / 2 + 30).flat_map(|y| (W / 2 - 30..W / 2 + 30).map(move |x| (x, y))).map(|(x, y)| at(x, y)).collect();
        let (lit, shade) = (inside.iter().filter(|v| **v > 0.6).count(), inside.iter().filter(|v| **v < 0.3).count());
        assert!(lit > inside.len() / 10 && shade > inside.len() / 20, "{pattern:?}: {lit} lit, {shade} shaded of {}", inside.len());
    }
}

#[test]
fn edges_blur_with_the_distance_past_the_gobo() {
    // the left half of a wall near, the right half far (the same blinds on both): the share of a stripe's period that is
    // in its penumbra grows with the distance, as an edge is the light's size times the distance past the slats; it used
    // to be one fixed share of a cell everywhere
    let f = flat(|x, _| if x < W / 2 { -0.4 } else { 0.6 });
    let e = Effects { pattern: Pattern::Blinds, scale: 0.8, softness: 0.6, ..Effects::default() };
    let plane = light_plane(&f, &light(), &e);
    let soft_share = |x0: usize, x1: usize| {
        let (mut soft, mut all) = (0usize, 0usize);
        for y in H / 2 - 30..H / 2 + 30 {
            for x in x0..x1 {
                let v = plane[y * W + x];
                all += 1;
                soft += usize::from(v > 0.15 && v < 0.75);
            }
        }
        soft as f32 / all.max(1) as f32
    };
    let (near, far) = (soft_share(W / 2 - 20, W / 2 - 4), soft_share(W / 2 + 4, W / 2 + 20));
    assert!(far > 1.4 * near && far > 0.2, "penumbra share: near {near}, far {far}");
}

#[test]
fn a_lights_depth_crossing_the_scenes_mean_moves_no_pattern_to_the_other_side() {
    // the probe of the audit of #309: blinds on a scene whose left half is near (log depth −0.6) and right half far (0);
    // light depth 0.75 → 0.80 used to move the pattern from the far half to the near half in one step (the other half
    // flat); now each half keeps its pattern's contrast, and a small step slides the stripes a little
    let f = flat(|x, _| if x < W / 2 { -0.6 } else { 0.0 });
    let e = Effects { pattern: Pattern::Blinds, softness: 0.1, ..Effects::default() };
    let contrast = |plane: &[f32], xs: std::ops::Range<usize>| {
        let v: Vec<f32> = (H / 4..3 * H / 4).flat_map(|y| xs.clone().map(move |x| (x, y))).map(|(x, y)| plane[y * W + x]).collect();
        let m = v.iter().sum::<f32>() / v.len().max(1) as f32;
        (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len().max(1) as f32).sqrt()
    };
    let at = |depth: f64| light_plane(&f, &Light { depth, ..light() }, &e);
    let (a, b) = (at(0.75), at(0.80));
    for (side, xs) in [("near", W / 2 - 40..W / 2 - 4), ("far", W / 2 + 4..W / 2 + 40)] {
        let (ca, cb) = (contrast(&a, xs.clone()), contrast(&b, xs));
        assert!(ca > 0.2 && cb > 0.2 && (ca - cb).abs() < 0.08, "the {side} half's pattern: contrast {ca} at 0.75, {cb} at 0.80");
    }
    let flipped = |p: &[f32], q: &[f32]| p.iter().zip(q).filter(|(p, q)| (*p - *q).abs() > 0.5).count() as f32 / p.len() as f32;
    let step = flipped(&at(0.77), &at(0.78));
    // (the window follows the light's pool, so the stripes slide a little; the old jump changed half the photo)
    assert!(step < 0.02, "depth 0.77 → 0.78: {:.1} % of the pixels flip", 100.0 * step);
}

#[test]
fn blinds_are_level_on_a_wall_and_a_step_in_depth_moves_them() {
    let e = Effects { pattern: Pattern::Blinds, scale: 0.75, softness: 0.0, ..Effects::default() };
    let wall = flat(|_, _| 0.0);
    let plane = light_plane(&wall, &light(), &e);
    let (a, b) = (edges(&plane, W / 2 - 10), edges(&plane, W / 2 + 10));
    assert!(a.len() >= 4, "{a:?}");
    assert!(a.len() == b.len() && a.iter().zip(&b).all(|(p, q)| p.abs_diff(*q) <= 1), "not level: {a:?} {b:?}");
    // the wall's right half farther: each side level on its own, the far side's stripes elsewhere
    let f = flat(|x, _| if x < W / 2 { 0.0 } else { 0.4 });
    let plane = light_plane(&f, &light(), &e);
    let same = |a: &[usize], b: &[usize]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 1);
    let (n1, n2, f1, f2) = (edges(&plane, W / 2 - 16), edges(&plane, W / 2 - 6), edges(&plane, W / 2 + 6), edges(&plane, W / 2 + 16));
    assert!(same(&n1, &n2) && same(&f1, &f2), "{n1:?} {n2:?} {f1:?} {f2:?}");
    assert!(!same(&n1, &f1), "the step moved nothing: {n1:?} {f1:?}");
}

#[test]
fn opening_the_blinds_lets_more_through_and_shade_is_never_black() {
    let [shut, half, wide] = [0.0, 0.5, 1.0].map(|o| crate::pattern::mean_passed(Pattern::Blinds, o, 0.01));
    assert!(shut.0 < half.0 && half.0 < wide.0, "{shut:?} {half:?} {wide:?}");
    // inside the opening the shade still gets some light: a slat's bounce, a leaf's glow
    for pattern in ALL {
        let (light, through) = crate::pattern::mean_passed(pattern, 0.5, 0.01);
        assert!(light > 0.08 && light < 0.9 && through > 0.003, "{pattern:?}: light {light}, through {through}");
    }
}

/// A wall at log depth 0.3 with a near box (−0.5) in the middle, its depth blending into the wall over `halo` pixels
/// round it, as a depth model's map does.
fn subject(halo: f32) -> Field {
    let (x0, x1, y0, y1) = (84.0, 108.0, 40.0, 104.0);
    flat(|x, y| {
        let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
        let d = (x0 - x).max(x - x1).max(0.0).hypot((y0 - y).max(y - y1).max(0.0));
        let t = if halo > 0.0 {
            (1.0 - d / halo).clamp(0.0, 1.0)
        } else if d <= 0.0 {
            1.0
        } else {
            0.0
        };
        0.3 - 0.8 * t
    })
}

fn sun() -> Light {
    Light { kind: LightKind::Sun(crate::kinds::Sun { azimuth: -40.0, elevation: 45.0 }), ..light() }
}

#[test]
fn the_pattern_lands_on_a_near_subject_in_front_of_the_wall() {
    // the first surface the light reaches takes the pattern: the box in front of the wall is striped too, from a lamp and
    // from the sun (the sun's slanted parallel rays used to move the box out of the window: it got none)
    let f = subject(0.0);
    let e = Effects { pattern: Pattern::Blinds, scale: 0.8, softness: 0.1, ..Effects::default() };
    for (name, l) in [("lamp", light()), ("sun", sun())] {
        let plane = light_plane(&f, &l, &e);
        let on_box: Vec<f32> = (44..100).flat_map(|y| (88..104).map(move |x| (x, y))).map(|(x, y)| plane[y * W + x]).collect();
        let (lit, shade) = (on_box.iter().filter(|v| **v > 0.6).count(), on_box.iter().filter(|v| **v < 0.3).count());
        assert!(lit > on_box.len() / 5 && shade > on_box.len() / 5, "{name}: {lit} lit, {shade} shaded of {}", on_box.len());
    }
}

#[test]
fn stripes_run_straight_across_the_wall_beside_a_subject() {
    // the box's depth blends into the wall over 14 pixels: the stripes on the wall just beside it are where they are
    // far from it (they used to bend round its outline into a halo, the depth map's blur turned into a shift)
    let f = subject(14.0);
    let e = Effects { pattern: Pattern::Blinds, scale: 0.8, softness: 0.0, ..Effects::default() };
    for (name, l) in [("lamp", light()), ("sun", sun())] {
        let plane = light_plane(&f, &l, &e);
        let (beside, away) = (edges(&plane, 77), edges(&plane, 60));
        // (the projector sits a little in front of the camera, so depth shifts the stripes gently: a step in depth still
        // moves them, a blurred outline by a row or two)
        let near = |a: &[usize], b: &[usize]| a.len() == b.len() && a.iter().zip(b).all(|(p, q)| p.abs_diff(*q) <= 2);
        assert!(beside.len() >= 4 && near(&beside, &away), "{name}: beside the box {beside:?}, away from it {away:?}");
    }
}
