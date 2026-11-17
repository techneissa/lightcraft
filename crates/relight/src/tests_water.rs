//! Light through moving water (issue #333): the net lies on what faces up and not on a wall, and is sharp near the surface and
//! soft deeper.

use crate::caustics::Surface;
use crate::effects::{Effects, Pattern};
use crate::field::Field;
use crate::kinds::{LightKind, Sun};
use crate::light::Light;

const W: usize = 384;
const H: usize = 256;

/// A wall facing the camera over the top half, and a floor under it that runs away from the camera (camera 0.3 above it).
fn seabed() -> Field {
    let ky = crate::field::LENS * H as f32 / W as f32;
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let v = (i / W) as f32 / H as f32;
            if v < 0.5 { 1.5f32.ln() } else { (0.3 / ((v - 0.5).max(0.02) * ky)).ln() }
        })
        .collect();
    Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap()
}

/// The sun from above and behind the scene, as the water's own.
fn sun() -> Light {
    Light {
        x: 0.5,
        y: 0.5,
        depth: 0.3,
        amount: 1.0,
        softness: 0.5,
        warmth: 0.0,
        wrap: 0.0,
        kind: LightKind::Sun(Sun { azimuth: 180.0, elevation: 62.0 }),
    }
}

fn caustics() -> Effects {
    Effects { pattern: Pattern::Caustics, ..Effects::default() }
}

#[test]
fn caustics_light_a_floor_that_faces_up_and_not_a_wall() {
    let f = seabed();
    let rig = f.rig(&sun(), 0.0, 0.0);
    let (lobe, through) = f.patterned(&rig, f.lobe(&rig), &caustics(), false);
    assert!(through.is_none(), "the net has no material of its own");
    let mean = |v0: f32, v1: f32| {
        let (y0, y1) = ((v0 * H as f32) as usize, (v1 * H as f32) as usize);
        let rows: Vec<f32> = (y0..y1).flat_map(|y| (0..W).map(move |x| (x, y))).map(|(x, y)| lobe.m[y * W + x]).collect();
        rows.iter().sum::<f32>() / rows.len() as f32
    };
    let (wall, floor) = (mean(0.05, 0.4), mean(0.65, 0.95));
    assert!(floor > 0.02 && floor > 8.0 * wall, "floor {floor}, wall {wall}");
    // a net: lines on the floor, not an even wash
    let (lo, hi) = ((0.65 * H as f32) as usize, (0.95 * H as f32) as usize);
    let v: Vec<f32> = (lo..hi).flat_map(|y| (0..W).map(move |x| (x, y))).map(|(x, y)| lobe.m[y * W + x]).collect();
    let m = v.iter().sum::<f32>() / v.len() as f32;
    let sd = (v.iter().map(|a| (a - m) * (a - m)).sum::<f32>() / v.len() as f32).sqrt();
    assert!(sd > 0.3 * m, "an even wash: mean {m}, spread {sd}");
}

#[test]
fn the_net_is_sharp_near_the_surface_and_soft_deeper() {
    let f = seabed();
    let rig = f.rig(&sun(), 0.0, 0.0);
    let s = Surface::new(&rig, &caustics(), 0.0).unwrap();
    // points on a horizontal line a short way below the surface and a long way below it (camera space, y down, surface at −0.9)
    let spread = |y: f32| {
        let v: Vec<f32> = (0..600).map(|i| s.at([-1.0 + i as f32 * 0.0033, y, 1.5])).collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|a| (a - m) * (a - m)).sum::<f32>() / v.len() as f32).sqrt() / m.max(1e-6)
    };
    let (near, deep) = (spread(-0.7), spread(3.0));
    assert!(near > 1.6 * deep, "near the surface {near}, deep {deep}");
    // above the surface there is nothing to be lit by
    assert_eq!(s.at([0.0, -2.0, 1.0]), 0.0);
}
