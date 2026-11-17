//! The light along an outline (issue #322): each side of a step between near and far has its own light, and nothing
//! outside the range of the two sides appears beside the outline.

use crate::{Field, Light, LightKind, Sun};

const W: usize = 320;
const H: usize = 200;
const EDGES: (usize, usize) = (110, 210);
const ROWS: (usize, usize) = (50, 150);
/// Stops: the pixels of an outline itself take their normal from both sides, about 0.09 stop off here; the halo this guards against was 0.26.
const TOLERANCE: f32 = 0.12;

/// A near box (log depth −1, `EDGES` across, `ROWS` down) in front of a far wall (0.5), the box much darker than the
/// wall and the wall's luminance in soft bands, as a stairs' or a wall's paint is.
fn box_on_wall() -> Field {
    let inside = |x: usize, y: usize| (EDGES.0..EDGES.1).contains(&x) && (ROWS.0..ROWS.1).contains(&y);
    let lz: Vec<f32> = (0..W * H).map(|i| if inside(i % W, i / W) { -1.0 } else { 0.5 }).collect();
    let lum: Vec<f32> = (0..W * H)
        .map(|i| if inside(i % W, i / W) { -2.5 } else { 0.7 + 1.5 * (((i / W) / 6 % 2) as f32 - 0.5) + 0.3 * (((i % W) / 9 % 2) as f32 - 0.5) })
        .collect();
    Field::from_parts(W, H, lz, lum).unwrap()
}

#[test]
fn along_an_outline_the_light_stays_within_the_range_of_its_two_sides() {
    let f = box_on_wall();
    let sun = Light {
        x: 0.5,
        y: 0.5,
        depth: 0.0,
        amount: 0.8,
        softness: 0.3,
        warmth: 0.0,
        wrap: 0.3,
        kind: LightKind::Sun(Sun { azimuth: -50.0, elevation: 40.0 }),
    };
    let sh = f.shading(&f.rig(&sun, 0.0, 0.0));
    let p = sh.planes(W, H, &|x, y| (x / W as f64, y / H as f64));
    let g = |x: usize, y: usize| p.stops[y * W + x];
    // each side's own light: a column well away from the outline (the wall's bands move it a little)
    let rows = ROWS.0 + 10..ROWS.1 - 10;
    for (name, edge, wall_side, box_side) in
        [("left", EDGES.0, edge_cols(EDGES.0, -1), edge_cols(EDGES.0, 1)), ("right", EDGES.1, edge_cols(EDGES.1, 1), edge_cols(EDGES.1, -1))]
    {
        let side = |cols: &[usize]| {
            let v: Vec<f32> = rows.clone().flat_map(|y| cols.iter().map(move |&x| g(x, y))).collect();
            (v.iter().copied().fold(f32::MAX, f32::min), v.iter().copied().fold(f32::MIN, f32::max))
        };
        let (wall, boxed) = (side(&wall_side), side(&box_side));
        let (lo, hi) = (wall.0.min(boxed.0), wall.1.max(boxed.1));
        assert!(hi - lo > 0.3, "{name}: the two sides should differ ({wall:?} {boxed:?})");
        for y in rows.clone() {
            for x in edge - 14..edge + 14 {
                let v = g(x, y);
                assert!(v > lo - TOLERANCE && v < hi + TOLERANCE, "{name} edge at ({x}, {y}): {v} outside {lo}..{hi} (wall {wall:?}, box {boxed:?})");
            }
        }
    }
}

/// The columns 20..30 pixels from `edge` towards `dir`.
fn edge_cols(edge: usize, dir: i32) -> Vec<usize> {
    (20..30).map(|d| (edge as i32 + dir * d) as usize).collect()
}
