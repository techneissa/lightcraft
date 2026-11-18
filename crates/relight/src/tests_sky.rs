//! Rays in the sky (issue #330): Sky Rays shaped by the user and cut by clouds and near things, and the bands opposite.

use crate::field::Field;
use crate::sky::SkyRays;
use crate::sun_rays::{RayField, RayShape};

/// A display whose white is linear 1 and no tone curve.
fn plain(y: f32) -> f32 {
    (1.0 - y) / 0.18
}

impl Field {
    fn opposite_plain(&self, centre: (f32, f32)) -> Option<SkyRays> {
        self.opposite_rays(centre, &plain)
    }

    fn ray_field_plain(&self, sun: (f32, f32)) -> RayField {
        self.ray_field(sun, &plain).unwrap()
    }
}

/// A fan of rays so dense it is nearly even all round, reaching far: what the clouds and near things cut shows plainly.
fn even(blocking: f32) -> RayShape {
    RayShape { count: 60, length: 1.0, width: 1.0, variation: 0.0, direction: 0.0, spread: std::f32::consts::TAU, softness: 1.0, blocking, seed: 1 }
}

const W: usize = 240;
const H: usize = 160;
/// The sky's lowest row, and the cloud band's rows.
const HORIZON: usize = 100;
const CLOUD: (usize, usize) = (25, 45);

fn lum(c: [f32; 3]) -> f32 {
    ((0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(1e-6) / 0.18).log2()
}

#[test]
fn a_photo_without_sky_has_no_rays() {
    // a near green wall everywhere
    let colour = vec![[0.06, 0.14, 0.03]; W * H];
    let lz = (0..W * H).map(|i| -0.5 + 0.2 * ((i % W) as f32 / W as f32)).collect();
    let l = colour.iter().map(|c| lum(*c)).collect();
    let f = Field::from_parts(W, H, lz, l).unwrap().with_colour(&colour).unwrap();
    assert!(!f.has_sky());
    assert!(f.ray_field((0.5, 0.1), &plain).is_none());
    assert!(f.opposite_plain((0.5, 0.9)).is_none());
}

/// An open blue sky over green land.
fn open_sky() -> Field {
    scene_with(|_, _| [0.5, 0.6, 0.9], |_| false)
}

#[test]
fn sky_rays_stay_inside_the_spread() {
    // the sun near the top's middle; a fan 60° wide pointing down, then the same fan pointing right
    let f = open_sky();
    let field = f.ray_field_plain((0.5, 0.05));
    let (sx, sy) = (0.5 * W as f32, 0.05 * H as f32);
    let ring = |m: &[f32], deg: f32| {
        let a = deg.to_radians();
        let (x, y) = (sx + 50.0 * a.sin(), sy + 50.0 * a.cos());
        m[y as usize * W + x as usize]
    };
    let shape = RayShape { spread: 60f32.to_radians(), blocking: 0.0, length: 1.0, ..RayShape::default() };
    let down = field.shape(&shape).0.m;
    let inside = (-25..=25).map(|d| ring(&down, d as f32)).fold(0.0, f32::max);
    assert!(inside > 0.05, "no rays inside the spread: {inside}");
    for d in (-90..=-32).chain(32..=90) {
        assert_eq!(ring(&down, d as f32), 0.0, "a ray at {d}° from straight down, outside the 60° spread");
    }
    let right = field.shape(&RayShape { direction: 90f32.to_radians(), ..shape }).0.m;
    assert!((65..=115).map(|d| ring(&right, d as f32)).fold(0.0, f32::max) > 0.05, "no rays to the right");
    assert_eq!(ring(&right, 0.0), 0.0, "a ray straight down with the fan pointing right");
}

#[test]
fn length_limits_how_far_sky_rays_reach() {
    // the sun at the top's middle; at Length 10 the rays end about 47 pixels from it (0.195 of the long side), fading towards
    // their end
    let f = open_sky();
    let field = f.ray_field_plain((0.5, 0.0));
    let at = |m: &[f32], d: usize| m[d * W + W / 2];
    let short = field.shape(&RayShape { length: 0.1, ..even(0.0) }).0.m;
    let long = field.shape(&RayShape { length: 1.0, ..even(0.0) }).0.m;
    assert!(
        at(&short, 30) > 0.0 && at(&short, 30) > at(&short, 42),
        "rays at Length 10 don't reach 30 px, or don't fade: {} {}",
        at(&short, 30),
        at(&short, 42)
    );
    for d in 48..HORIZON {
        assert_eq!(at(&short, d), 0.0, "a ray at Length 10 still reaches {d} px from the sun");
    }
    assert!(at(&long, 60) > 0.05, "rays at Length 100 don't reach 60 px: {}", at(&long, 60));
}

/// An open sky with a thick cloud lump over columns 100..140 of the cloud band, and a near pillar (columns 180..190, rows
/// 60..100) standing up into the sky.
fn cloud_and_pillar() -> Field {
    let (mut lz, mut colour) = (Vec::with_capacity(W * H), Vec::with_capacity(W * H));
    for y in 0..H {
        for x in 0..W {
            if (180..190).contains(&x) && (60..HORIZON).contains(&y) {
                lz.push(-0.5);
                colour.push([0.08, 0.06, 0.04]);
            } else if y < HORIZON {
                lz.push(1.5);
                colour.push(if (CLOUD.0..CLOUD.1).contains(&y) && (100..140).contains(&x) { [0.06, 0.06, 0.065] } else { [0.5, 0.6, 0.9] });
            } else {
                lz.push(0.6 - 1.6 * (y - HORIZON) as f32 / (H - HORIZON) as f32);
                colour.push([0.06, 0.14, 0.03]);
            }
        }
    }
    let l = colour.iter().map(|c| lum(*c)).collect();
    Field::from_parts(W, H, lz, l).unwrap().with_colour(&colour).unwrap()
}

#[test]
fn blocking_cuts_sky_rays_behind_a_cloud_and_a_near_thing_only_when_raised() {
    // the sun above the frame's middle: (120, 80) looks at it through the cloud lump, (200, 90) through the pillar, (40, 80)
    // and (230, 90) through open sky
    let f = cloud_and_pillar();
    let field = f.ray_field_plain((0.5, -0.3));
    let (none, full) = (field.shape(&even(0.0)).0.m, field.shape(&even(1.0)).0.m);
    let at = |m: &[f32], x: usize, y: usize| m[y * W + x];
    for (x, y) in [(120, 80), (200, 90), (40, 80), (230, 90)] {
        assert!(at(&none, x, y) > 0.05, "Blocking 0 leaves no ray at ({x}, {y}): {}", at(&none, x, y));
    }
    for (x, y, what) in [(120, 80, "cloud"), (200, 90, "pillar")] {
        assert!(
            at(&full, x, y) < 0.2 * at(&none, x, y),
            "Blocking 100 doesn't cut the ray behind the {what}: {} against {}",
            at(&full, x, y),
            at(&none, x, y)
        );
    }
    for (x, y) in [(40, 80), (230, 90)] {
        assert!(
            at(&full, x, y) > 0.6 * at(&none, x, y),
            "Blocking 100 cuts a ray through open sky at ({x}, {y}): {} against {}",
            at(&full, x, y),
            at(&none, x, y)
        );
    }
}

#[test]
fn opposite_rays_are_dark_bands_on_open_sky_converging_on_their_point() {
    // a clear sky with two clouds at its top: the point opposite the sun low in the middle
    let colour: Vec<[f32; 3]> = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            if y >= HORIZON {
                [0.06, 0.14, 0.03]
            } else if y < 15 && ((30..60).contains(&x) || (170..200).contains(&x)) {
                [0.55, 0.55, 0.55]
            } else {
                [0.25, 0.4, 0.95]
            }
        })
        .collect();
    let lz = (0..W * H).map(|i| if i / W < HORIZON { 1.5 } else { 0.6 - 1.6 * (i / W - HORIZON) as f32 / (H - HORIZON) as f32 }).collect();
    let l = colour.iter().map(|c| lum(*c)).collect();
    let f = Field::from_parts(W, H, lz, l).unwrap().with_colour(&colour).unwrap();
    let rays = f.opposite_plain((0.5, 0.75)).unwrap();
    // the band from the left cloud towards the point is darker than the open sky beside it
    let shade = |x: usize, y: usize| rays.shade.m[y * W + x];
    // (the line from the point (120, 120) through the cloud's middle (45, 7) passes x ≈ 80 at row 60)
    let band = shade(80, 60);
    assert!(band > 0.1 && band > 3.0 * shade(120, 60), "no dark band: {band} against {}", shade(120, 60));
}

/// A landscape with the sun ahead whose sky colour is `sky(x, y)` and whose cloud band (rows [`CLOUD`]) is thick where
/// `thick(x)`.
fn scene_with(sky: impl Fn(usize, usize) -> [f32; 3], thick: impl Fn(usize) -> bool) -> Field {
    let (mut lz, mut colour) = (Vec::with_capacity(W * H), Vec::with_capacity(W * H));
    for y in 0..H {
        for x in 0..W {
            if y < HORIZON {
                lz.push(1.5);
                colour.push(if (CLOUD.0..CLOUD.1).contains(&y) && thick(x) { [0.06, 0.06, 0.065] } else { sky(x, y) });
            } else {
                lz.push(0.6 - 1.6 * (y - HORIZON) as f32 / (H - HORIZON) as f32);
                colour.push([0.06, 0.14, 0.03]);
            }
        }
    }
    let l = colour.iter().map(|c| lum(*c)).collect();
    Field::from_parts(W, H, lz, l).unwrap().with_colour(&colour).unwrap()
}

/// The steepest step of what comes through (Blocking 100 against 0) along row `y` over columns `xs`.
fn steepest(none: &[f32], full: &[f32], y: usize, xs: std::ops::Range<usize>) -> f32 {
    let v = |x: usize| full[y * W + x] / none[y * W + x].max(1e-6);
    xs.map(|x| (v(x + 1) - v(x)).abs()).fold(0.0, f32::max)
}

#[test]
fn a_cut_edge_softens_with_distance_from_the_cloud_that_casts_it() {
    // (issue #330, round 4) a cloud edge, the sun far above it: just below the cloud the cut's edge is fairly sharp, far below
    // it much softer (the sun's disc and the haze widen the penumbra with distance), not one knife edge all the way
    let f = scene_with(|_, _| [0.5, 0.6, 0.9], |x| x < 120);
    let field = f.ray_field_plain((0.5, -1.0));
    let (none, full) = (field.shape(&even(0.0)).0.m, field.shape(&even(1.0)).0.m);
    let (near, far) = (steepest(&none, &full, CLOUD.1 + 4, 95..145), steepest(&none, &full, HORIZON - 4, 95..145));
    assert!(near > 0.02, "no cut's edge below the cloud: {near}");
    assert!(far < 0.7 * near, "the edge is as sharp far from the cloud ({far}) as near it ({near})");
}

#[test]
fn opposite_rays_have_no_steps_along_the_photos_edges() {
    // (issue #330, round 4) over the open sky the bands meet the photo's left and right edges smoothly, without a stair of
    // blocks (a cloud's own edge, where the sky opens below it, is its own)
    // (clouds at the edges themselves, where the lines leave the photo)
    let f = scene_with(|_, _| [0.25, 0.4, 0.95], |x| x < 25 || (90..110).contains(&x) || x > 215);
    let rays = f.opposite_plain((0.5, 1.2)).unwrap();
    for x in [0, 1, W - 2, W - 1] {
        let v = |y: usize| rays.light.m[y * W + x] - rays.shade.m[y * W + x];
        let step = (CLOUD.1 + 6..HORIZON - 2).map(|y| (v(y + 1) - v(y)).abs()).fold(0.0, f32::max);
        let col: Vec<i32> = (0..HORIZON).step_by(2).map(|y| (v(y) * 100.0) as i32).collect();
        assert!(step < 0.05, "a step of {step} down column {x}: {col:?}");
    }
}
