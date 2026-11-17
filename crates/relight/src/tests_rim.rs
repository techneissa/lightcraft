//! The Rim light (issue #319): a thin bright edge along the near side of a depth step, never patches inside a surface.

use crate::{Field, Light, LightKind, Rim};

const W: usize = 480;
const H: usize = 360;

/// A near disc (log depth −0.5, radius 22 % of the width) in front of a far wall (0.3), centred; no noise inside it but a
/// bump of the kind a depth model gives a face (a nose-sized step of 20 %).
fn disc() -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (x, y) = ((i % W) as f32 / W as f32 - 0.5, ((i / W) as f32 / H as f32 - 0.5) * H as f32 / W as f32);
            let r = (x * x + y * y).sqrt();
            let bump = if (x - 0.05).abs() < 0.04 && (y + 0.02).abs() < 0.05 { -0.2 } else { 0.0 };
            if r < 0.22 { -0.5 + bump } else { 0.3 }
        })
        .collect();
    Field::from_parts(W, H, lz, vec![-2.0; W * H]).unwrap()
}

fn rim(kind: Rim) -> Light {
    Light { x: 0.5, y: 0.5, depth: 0.0, amount: 1.0, softness: 0.4, warmth: 0.0, wrap: 0.0, kind: LightKind::Rim(kind) }
}

#[test]
fn a_rim_lights_a_ring_along_the_edge_of_a_near_disc_and_nothing_else() {
    let f = disc();
    let rig = f.rig(&rim(Rim::default()), 0.0, 0.0);
    let at = |x: f32, y: f32| f.shade(&rig, x, y).stops;
    let px = 1.0 / W as f32;
    // the disc's edge, just inside it, on every side
    let edge = |dx: f32, dy: f32| at(0.5 + dx * (0.22 - 2.5 * px), 0.5 + dy * (0.22 - 2.5 * px) * W as f32 / H as f32);
    let ring = [edge(1.0, 0.0), edge(-1.0, 0.0), edge(0.0, 1.0), edge(0.0, -1.0), edge(0.7, 0.7), edge(-0.7, -0.7)];
    assert!(ring.iter().all(|v| *v > 1.2), "ring {ring:?}");
    // the middle (with the bump) and the far wall, next to the edge and away from it
    // (the bump's pixels, its own edges included: a step of 20 % inside a surface is no outline)
    let bump = (0..=8)
        .flat_map(|i| (0..=8).map(move |j| (0.512 + 0.076 * i as f32 / 8.0, 0.41 + 0.126 * j as f32 / 8.0)))
        .map(|(x, y)| at(x, y))
        .fold(0.0, f32::max);
    let (middle, beside, wall) = (at(0.5, 0.5), at(0.5 + 0.22 + 2.5 * px, 0.5), at(0.95, 0.5));
    assert!(middle < 0.05 && bump < 0.01 && beside < 0.05 && wall < 0.05, "middle {middle} bump {bump} beside {beside} wall {wall}");
    // it fades smoothly inward: no step in the gain between neighbouring pixels, and less every pixel in
    let row: Vec<f32> = (0..30).map(|i| at(0.5 + 0.22 - (2.5 + i as f32) * px, 0.5)).collect();
    assert!(row.windows(2).all(|p| p[1] <= p[0] + 0.02 && p[0] - p[1] < 0.5), "fade {row:?}");
    assert!(row[29] < 0.1 * row[0], "reaches too far {row:?}");
    // a wider rim reaches further in
    let wide = f.rig(&rim(Rim { width: 1.0, ..Rim::default() }), 0.0, 0.0);
    let deep = at(0.5 + 0.22 - 12.0 * px, 0.5);
    let wider = f.shade(&wide, 0.5 + 0.22 - 12.0 * px, 0.5).stops;
    assert!(wider > deep + 0.5, "wide {wider} default {deep}");
}

#[test]
fn a_one_sided_rim_lights_only_the_edge_facing_its_side() {
    let f = disc();
    let from_right = f.rig(&rim(Rim { angle: 90.0, one_sided: 1.0, ..Rim::default() }), 0.0, 0.0);
    let px = 1.0 / W as f32;
    let (right, left, top) = (
        f.shade(&from_right, 0.5 + 0.22 - 2.5 * px, 0.5).stops,
        f.shade(&from_right, 0.5 - 0.22 + 2.5 * px, 0.5).stops,
        f.shade(&from_right, 0.5, 0.5 - (0.22 - 2.5 * px) * W as f32 / H as f32).stops,
    );
    assert!(right > 1.2 && left < 0.1 && top < 0.1, "right {right} left {left} top {top}");
}
