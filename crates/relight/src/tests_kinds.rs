//! Light types (issue #300): each lights what it should on a synthetic field, and hostile numbers give finite planes.

use crate::{Field, Fill, Light, LightKind, Rim, Softbox, Spot, Sun};

fn light(kind: LightKind) -> Light {
    Light { x: 0.5, y: 0.5, depth: 0.0, amount: 1.0, softness: 0.4, warmth: 0.0, wrap: 0.0, kind }
}

/// A wall facing the camera, two stops under grey, with the left half `near` nearer.
fn wall(near: f32) -> Field {
    let (w, h) = (96, 64);
    let lz: Vec<f32> = (0..w * h).map(|i| if i % w < w / 2 { -near } else { 0.0 }).collect();
    Field::from_parts(w, h, lz, vec![-2.0; w * h]).unwrap()
}

fn stops(f: &Field, l: &Light, u: f32, v: f32) -> f32 {
    f.shade(&f.rig(l, 0.0, 0.0), u, v).stops
}

#[test]
fn a_spot_lights_inside_its_cone_and_not_outside() {
    let f = wall(0.0);
    let spot = |angle: f64, edge: f64| light(LightKind::Spot(Spot { aim_x: 0.9, aim_y: 0.5, angle, edge, sharp: false }));
    let l = spot(15.0, 0.2);
    let (inside, outside) = (stops(&f, &l, 0.9, 0.5), stops(&f, &l, 0.2, 0.5));
    assert!(inside > 1.0 && outside < 0.05, "inside {inside} outside {outside}");
    // a wider cone reaches what a narrow one leaves dark
    let wide = spot(70.0, 0.2);
    assert!(stops(&f, &wide, 0.5, 0.5) > stops(&f, &l, 0.5, 0.5) + 0.5);
}

#[test]
fn a_sun_lights_facing_surfaces_at_any_distance() {
    let f = wall(0.7);
    let sun = light(LightKind::Sun(Sun { azimuth: 0.0, elevation: 20.0 }));
    let rig = f.rig(&sun, 0.0, 0.0);
    let (near, far) = (f.shade(&rig, 0.2, 0.5).stops, f.shade(&rig, 0.8, 0.5).stops);
    assert!(near > 1.0 && (near - far).abs() < 0.05, "near {near} far {far}");
    // the same wall under a point light falls off with distance; a sun from behind the wall gives it nothing
    assert!(stops(&f, &light(LightKind::Point), 0.4, 0.5) > stops(&f, &light(LightKind::Point), 0.6, 0.5) + 0.3);
    let behind = light(LightKind::Sun(Sun { azimuth: 180.0, elevation: 10.0 }));
    assert!(stops(&f, &behind, 0.5, 0.5) < 0.05);
}

#[test]
fn a_rim_lights_the_edges_that_turn_away_and_not_the_middle_or_the_background() {
    // a dome standing out of a far background: its middle faces the camera, its edge turns away and steps to the background
    let (w, h) = (128usize, 96usize);
    let lz: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32 - 0.5, (i / w) as f32 / h as f32 - 0.5);
            let r = (x * x + (y * h as f32 / w as f32).powi(2)).sqrt() / 0.25;
            if r < 1.0 { -0.4 - 0.4 * (1.0 - r * r).sqrt() } else { 0.4 }
        })
        .collect();
    let f = Field::from_parts(w, h, lz, vec![-2.0; w * h]).unwrap();
    let l = light(LightKind::Rim(Rim { width: 0.6, ..Rim::default() }));
    let rig = f.rig(&l, 0.0, 0.0);
    let at = |dx: f32| f.shade(&rig, 0.5 + dx, 0.5).stops;
    // (the brightest of the dome's last few pixels: the rim is a thin line)
    let edge = (0..=10).map(|i| at(0.2 + 0.005 * i as f32)).fold(0.0, f32::max);
    let (middle, background) = (at(0.0), at(0.42));
    assert!((1.2..=2.6).contains(&edge) && middle < 0.05 && background < 0.05, "middle {middle} edge {edge} background {background}");
}

#[test]
fn a_fill_lifts_shadows_clearly_and_fades_out_above_mid_grey() {
    // a flat wall in bands of luminance (stops under or over 18 % grey), full Amount
    let (w, h) = (96usize, 64usize);
    let bands = [-4.0f32, -2.0, 0.0, 1.0, 2.5];
    let lum: Vec<f32> = (0..w * h).map(|i| bands[(i % w) * bands.len() / w]).collect();
    let f = Field::from_parts(w, h, vec![0.0; w * h], lum).unwrap();
    let rig = f.rig(&light(LightKind::Fill(Fill { near: 0.5 })), 0.0, 0.0);
    let at = |k: usize| f.shade(&rig, (k as f32 + 0.5) / bands.len() as f32, 0.5).stops;
    let s: Vec<f32> = (0..bands.len()).map(at).collect();
    assert!((1.0..=1.6).contains(&s[0]), "deep shadow {s:?}");
    assert!(s[0] > s[1] && s[1] > s[2] && s[2] > s[3] && s[2] > 0.2, "fading out {s:?}");
    assert!(s[4] < 0.05, "highlight {s:?}");
    // the photo's exposure counts: the same wall at −2 stops is in deeper shadow
    let dark = f.rig(&light(LightKind::Fill(Fill { near: 0.5 })), 0.0, -2.0);
    assert!(f.shade(&dark, 0.5, 0.5).stops > s[2] + 0.3, "exposure not read: {} {}", f.shade(&dark, 0.5, 0.5).stops, s[2]);
    // and near things more than far ones
    let lz: Vec<f32> = (0..w * h).map(|i| if i % w < w / 2 { -0.7 } else { 0.0 }).collect();
    let f = Field::from_parts(w, h, lz, vec![-4.0; w * h]).unwrap();
    let rig = f.rig(&light(LightKind::Fill(Fill { near: 1.0 })), 0.0, 0.0);
    assert!(f.shade(&rig, 0.2, 0.5).stops > f.shade(&rig, 0.8, 0.5).stops + 0.5);
}

#[test]
fn a_bigger_softbox_is_softer_and_reaches_further() {
    let f = wall(0.0);
    let boxed =
        |s: f64| Light { x: 0.2, softness: 0.5, ..light(LightKind::Softbox(Softbox { width: s, height: s, rotation: 0.0, aim_h: 0.0, aim_v: 0.0 })) };
    // (the ratio of the gain, in stops, a point far across the wall gets to the most lit point's)
    let share = |s: f64| {
        let rig = f.rig(&boxed(s), 0.0, 0.0);
        f.shade(&rig, 0.9, 0.5).stops / f.shade(&rig, 0.2, 0.5).stops.max(1e-3)
    };
    assert!(share(0.8) > share(0.1) + 0.05, "small {} big {}", share(0.1), share(0.8));
    assert!(stops(&f, &boxed(0.3), 0.2, 0.5) > 1.0);
}

#[test]
fn hostile_numbers_give_finite_planes_for_every_type() {
    let f = wall(0.5);
    let bad = f64::NAN;
    let kinds = [
        LightKind::Spot(Spot { aim_x: bad, aim_y: f64::INFINITY, angle: -9.0, edge: 1e9, sharp: false }),
        LightKind::Sun(Sun { azimuth: f64::NEG_INFINITY, elevation: bad }),
        LightKind::Softbox(Softbox { width: bad, height: -3.0, rotation: f64::INFINITY, aim_h: 0.0, aim_v: 0.0 }),
        LightKind::Softbox(Softbox { width: 1e300, height: 1e300, rotation: 1e300, aim_h: 0.0, aim_v: 0.0 }),
        LightKind::Rim(Rim { width: bad, angle: f64::INFINITY, one_sided: 1e9 }),
        LightKind::Rim(Rim { width: -1e300, angle: bad, one_sided: bad }),
        LightKind::Fill(Fill { near: f64::INFINITY }),
    ];
    for kind in kinds {
        let l = Light { x: bad, y: f64::INFINITY, depth: -5.0, amount: 1e9, softness: bad, warmth: 9.0, wrap: 7.0, kind };
        let sh = f.shading(&f.rig(&l, -3.0, f64::NAN));
        let p = sh.planes(96, 64, &|x, y| (x / 96.0, y / 64.0));
        assert!(p.stops.iter().chain(&p.share).all(|v| v.is_finite()), "{kind:?}");
    }
}
