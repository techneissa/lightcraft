//! The field at half its size (a drag's renders and the small pictures): it lights the photo as the whole field does.

use crate::{Field, Light, LightKind, Softbox, Spot};

/// A dome standing out of a far wall, `w × h`, the wall's luminance a gradient.
fn dome(w: usize, h: usize) -> Field {
    let lz: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32 - 0.5, (i / w) as f32 / h as f32 - 0.5);
            let r = (x * x + (y * h as f32 / w as f32).powi(2)).sqrt() / 0.3;
            if r < 1.0 { -0.3 - 0.4 * (1.0 - r * r).sqrt() } else { 0.4 }
        })
        .collect();
    let lum: Vec<f32> = (0..w * h).map(|i| -3.0 + 2.0 * (i % w) as f32 / w as f32).collect();
    Field::from_parts(w, h, lz, lum).unwrap()
}

fn light(kind: LightKind) -> Light {
    Light { x: 0.3, y: 0.35, depth: -0.2, amount: 1.0, softness: 0.4, warmth: 0.0, wrap: 0.0, kind }
}

#[test]
fn the_half_field_lights_the_photo_as_the_whole_one_does() {
    let full = dome(256, 192);
    let half = full.halved().unwrap();
    assert_eq!((half.width, half.height), (128, 96));
    let lights = [
        light(LightKind::Point),
        light(LightKind::Spot(Spot { aim_x: 0.5, aim_y: 0.5, angle: 30.0, edge: 0.3, sharp: false })),
        light(LightKind::Softbox(Softbox { width: 0.3, height: 0.4, rotation: 0.0, aim_h: 20.0, aim_v: -10.0 })),
    ];
    for (k, l) in lights.iter().enumerate() {
        let (a, b) = (full.rig(l, 0.5, 0.0), half.rig(l, 0.5, 0.0));
        let mut worst = Vec::new();
        for j in 0..20 * 15 {
            let (u, v) = ((j % 20) as f32 / 20.0 + 0.025, (j / 20) as f32 / 15.0 + 0.033);
            // (the dome's outline, a step in depth, is softer at half the size: off it the light is the same)
            let r = ((u - 0.5).powi(2) + ((v - 0.5) * 0.75).powi(2)).sqrt() / 0.3;
            if (r - 1.0).abs() < 0.12 {
                continue;
            }
            let (x, y) = (full.shade(&a, u, v).stops, half.shade(&b, u, v).stops);
            assert!(x.is_finite() && y.is_finite());
            worst.push((x - y).abs());
        }
        worst.sort_by(f32::total_cmp);
        let p95 = worst[worst.len() * 95 / 100];
        assert!(p95 < 0.15, "light {k}: 95 % of the points within {p95} stops, not 0.15");
    }
}

#[test]
fn a_small_or_odd_field_halves_or_says_it_cannot() {
    assert!(dome(100, 80).halved().is_none(), "too small to halve");
    let odd = dome(259, 131).halved().unwrap();
    assert_eq!((odd.width, odd.height), (129, 65));
    let l = light(LightKind::Point);
    assert!(odd.shade(&odd.rig(&l, 0.5, 0.0), 0.999, 0.999).stops.is_finite());
}
