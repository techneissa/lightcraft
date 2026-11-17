//! Stage beams (issue #334): a narrower cone than a spot made before, a sharp-edged cone, and a light's source seen as a
//! bright point when it hangs inside the photo.

use crate::{Effects, Field, Light, LightKind, Spot};

const W: usize = 192;
const H: usize = 128;

/// A far wall (log depth 0.4) with a near patch in the bottom left corner (so the air begins at a fixed depth), and a near
/// `blocker` over `u` and `v` when given.
fn scene(blocker: Option<(std::ops::Range<f32>, std::ops::Range<f32>)>) -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (u, v) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            match &blocker {
                Some((us, vs)) if us.contains(&u) && vs.contains(&v) => -0.6,
                _ if u < 0.16 && v > 0.84 => -0.6,
                _ => 0.4,
            }
        })
        .collect();
    let lum = lz.iter().map(|l| if *l >= 0.4 { -2.0 } else { -3.0 }).collect();
    Field::from_parts(W, H, lz, lum).unwrap()
}

fn spot(angle: f64, edge: f64, sharp: bool) -> Light {
    Light {
        x: 0.5,
        y: 0.05,
        depth: 0.6,
        amount: 1.0,
        softness: 0.2,
        warmth: 0.0,
        wrap: 0.4,
        kind: LightKind::Spot(Spot { aim_x: 0.5, aim_y: 0.95, angle, edge, sharp }),
    }
}

/// The beam of `l` in haze, one row across it (at `v`), the left edge of the picture to the right.
fn row(f: &Field, l: &Light, v: f32) -> Vec<f32> {
    let rig = f.rig(l, 0.0, 0.0);
    let shafts = f.shafts(&rig, l, false).unwrap();
    let b = f.beam(&shafts, &rig, &Effects { haze: 0.6, ..Effects::default() });
    let (w, h) = b.size();
    (0..w).map(|x| b.at(x, (v * h as f32) as usize)).collect()
}

fn peak(r: &[f32]) -> f32 {
    r.iter().copied().fold(0.0, f32::max)
}

#[test]
fn a_narrow_spots_cone_in_haze_is_narrower_than_a_wide_ones() {
    // the narrowest cone used to stop at 5°: 2° is narrower, and 12° wider
    let f = scene(None);
    let width = |angle: f64| {
        let r = row(&f, &spot(angle, 0.0, false), 0.6);
        r.iter().filter(|v| **v > 0.3 * peak(&r)).count()
    };
    let (narrow, old_narrowest, wide) = (width(2.0), width(5.0), width(12.0));
    assert!(narrow + 2 <= old_narrowest, "2° is {narrow} pixels wide, 5° {old_narrowest}");
    assert!(old_narrowest + 2 <= wide, "5° is {old_narrowest} pixels wide, 12° {wide}");
    assert!(narrow >= 1);
}

/// A spot at the left edge of the far wall's plane aimed along it to the right, so a camera sees its cone from the side.
fn sideways(angle: f64, edge: f64, sharp: bool) -> Light {
    Light { x: 0.1, y: 0.5, depth: 1.0, kind: LightKind::Spot(Spot { aim_x: 0.9, aim_y: 0.5, angle, edge, sharp }), ..spot(angle, edge, sharp) }
}

#[test]
fn a_sharp_cones_edge_is_harder_than_a_soft_ones() {
    // the pool a cone leaves on a wall has the cone's edge: with a sharp cone it is a hard circle, with Cone Edge at 60 % a
    // soft one (steepest step between neighbouring pixels along a row, as a share of the pool's peak). In haze the beam's edge
    // is round whatever the cone's (a cylinder of lit air seen from the side), so only the pool shows it.
    let f = scene(None);
    let edge = |l: &Light| {
        let m = f.lobe(&f.rig(l, 0.0, 0.0)).m;
        let r: Vec<f32> = (0..W).map(|x| m[(0.5 * H as f32) as usize * W + x]).collect();
        let top = peak(&r).max(1e-9);
        r.windows(2).map(|p| (p[1] - p[0]).abs() / top).fold(0.0, f32::max)
    };
    let front = |edge: f64, sharp: bool| Light { depth: 0.2, ..sideways(20.0, edge, sharp) };
    let (hard, soft) = (edge(&front(0.6, true)), edge(&front(0.6, false)));
    assert!(hard > 2.0 * soft, "a sharp cone's pool edge steps {hard} of its peak per pixel, a soft one's {soft}");
    // (a sharp cone ignores Cone Edge)
    let across = |l: &Light| {
        let rig = f.rig(l, 0.0, 0.0);
        let b = f.beam(&f.shafts(&rig, l, false).unwrap(), &rig, &Effects { haze: 0.6, ..Effects::default() });
        let (w, h) = b.size();
        (0..h).map(|y| b.at(w / 2, y)).collect::<Vec<f32>>()
    };
    assert_eq!(across(&sideways(10.0, 0.0, true)), across(&sideways(10.0, 1.0, true)));
    assert!(across(&sideways(10.0, 0.0, true)) != across(&sideways(10.0, 1.0, false)));
}

/// The plane of `l`'s source, `None` when it shows none.
fn glow(f: &Field, l: &Light, strength: f64) -> Option<Vec<f32>> {
    f.source(l, &f.rig(l, 0.0, 0.0), strength).map(|lobe| lobe.m)
}

#[test]
fn a_source_inside_the_photo_shows_a_bright_point_and_one_outside_shows_none() {
    let f = scene(None);
    let at = |m: &[f32], x: f64, y: f64| m[(y * H as f64) as usize * W + (x * W as f64) as usize];
    let inside = glow(&f, &spot(10.0, 0.0, true), 0.6).unwrap();
    let (centre, far) = (at(&inside, 0.5, 0.05), at(&inside, 0.5, 0.6));
    assert!(centre > 4.0 && far == 0.0, "the point is {centre}, a spot far from it {far}");
    // it falls off round the point
    assert!(at(&inside, 0.5, 0.05) > at(&inside, 0.52, 0.05) && at(&inside, 0.52, 0.05) > at(&inside, 0.56, 0.05));
    // a light above the photo, or off its side, or with no strength, shows no point
    for light in [Light { y: -0.2, ..spot(10.0, 0.0, true) }, Light { x: 1.3, ..spot(10.0, 0.0, true) }] {
        assert!(glow(&f, &light, 0.6).is_none());
    }
    assert!(glow(&f, &spot(10.0, 0.0, true), 0.0).is_none());
    // a bigger Source is a bigger point
    let (small, big) = (glow(&f, &spot(10.0, 0.0, true), 0.1).unwrap(), glow(&f, &spot(10.0, 0.0, true), 1.0).unwrap());
    let lit = |m: &[f32]| m.iter().filter(|v| **v > 0.5).count();
    assert!(lit(&big) > 2 * lit(&small), "{} {}", lit(&small), lit(&big));
}

#[test]
fn a_sun_has_no_source_to_show() {
    // a Sun shines from a direction, not from its handle's place: Visible Source adds nothing for it
    let f = scene(None);
    let sun = Light { kind: LightKind::Sun(crate::Sun { azimuth: 30.0, elevation: 40.0 }), ..spot(10.0, 0.0, true) };
    assert!(glow(&f, &sun, 0.7).is_none());
    assert!(glow(&f, &spot(10.0, 0.0, true), 0.7).is_some());
}

#[test]
fn a_nearer_thing_hides_the_source() {
    // the light hangs in front of the far wall; a thing nearer than the light stands over it
    let light = spot(10.0, 0.0, true);
    let open = glow(&scene(None), &light, 0.6).unwrap();
    let covered = glow(&scene(Some((0.4..0.6, 0.0..0.2))), &light, 0.6).unwrap();
    let centre = (0.05 * H as f32) as usize * W + W / 2;
    assert!(open[centre] > 4.0 && covered[centre] < 0.05 * open[centre], "{} {}", open[centre], covered[centre]);
}

#[test]
fn hostile_numbers_give_a_finite_source() {
    let f = scene(None);
    for light in [
        Light { x: f64::NAN, ..spot(10.0, 0.0, true) },
        Light { depth: f64::NAN, ..spot(f64::NAN, f64::NAN, true) },
        Light { depth: -1e9, y: 1e9, ..spot(1e9, 1e9, false) },
    ] {
        for strength in [f64::NAN, f64::INFINITY, 1e30, -1.0, 0.5] {
            if let Some(m) = glow(&f, &light, strength) {
                assert!(m.len() == W * H && m.iter().all(|v| v.is_finite() && *v >= 0.0));
            }
        }
    }
}
