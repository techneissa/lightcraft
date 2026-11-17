//! Low mist and forest fog (issue #331): the mist is measured from the ground the depth shows, not by the image's row; far
//! things fade more than near ones; with no mist and no fade the beams are what they were.

use crate::{Effects, Field, Light, LightKind, Sun};

const W: usize = 192;
const H: usize = 128;

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The camera-space ray through normalized `(u, v)` (z = 1), as `Field::point` places pixels.
fn ray(u: f32, v: f32) -> [f32; 3] {
    let long = W.max(H) as f32;
    [(u - 0.5) * W as f32 / long * crate::field::LENS, (v - 0.5) * H as f32 / long * crate::field::LENS, 1.0]
}

/// The world's up in camera space for a camera pitched down `pitch` and rolled `roll` (degrees).
fn up(pitch: f32, roll: f32) -> [f32; 3] {
    let (p, r) = (pitch.to_radians(), roll.to_radians());
    [-r.sin() * p.cos(), -r.cos() * p.cos(), -p.sin()]
}

/// Open ground one unit under the camera (world up `u`), the sky at depth 40, and `boxed`: a wall facing the camera at
/// depth `.2` over the image's `.0` and `.1`.
fn ground(u: [f32; 3], boxed: Option<(std::ops::Range<f32>, std::ops::Range<f32>, f32)>) -> Field {
    let lz: Vec<f32> = (0..W * H)
        .map(|i| {
            let (x, y) = ((i % W) as f32 / W as f32, (i / W) as f32 / H as f32);
            if let Some((us, vs, z)) = &boxed
                && us.contains(&x)
                && vs.contains(&y)
            {
                return z.ln();
            }
            let k = dot(u, ray(x, y));
            // (the ray meets the ground where up · p = −1)
            let z = if k < -1.0 / 40.0 { -1.0 / k } else { 40.0 };
            z.ln()
        })
        .collect();
    // (the sky brighter than the ground: it is the open sky the air's rays go out through)
    let lum = lz.iter().map(|l| if *l >= 40f32.ln() - 1e-3 { -0.5 } else { -2.5 }).collect();
    Field::from_parts(W, H, lz, lum).unwrap()
}

#[test]
fn mist_is_thicker_low_than_high_on_a_tilted_ground_not_by_the_images_row() {
    // a camera pitched down 12° and rolled 30°: the horizon runs across the picture on a slant
    let truth = up(12.0, 30.0);
    let f = ground(truth, None);
    let g = f.ground();
    assert!(dot(g.up, truth) > 0.98, "the ground's up {:?}, the truth {truth:?}", g.up);
    assert!((g.level - 1.0).abs() < 0.25, "the camera stands {} over the ground, not 1", g.level);
    let m = f.medium(&Effects { mist: 0.8, mist_height: 0.3, ..Effects::default() }).unwrap();
    // two points of the air 1.5 units out: `a` higher in the picture than `b`, but nearer the ground (the roll lowers the
    // picture's right)
    let at = |u: f32, v: f32| ray(u, v).map(|c| c * 1.5);
    let (a, b) = (at(0.9, 0.4), at(0.1, 0.5));
    let height = |q: [f32; 3]| dot(truth, q) + 1.0;
    assert!(height(a) + 0.3 < height(b), "the test's points are not as meant: {} {}", height(a), height(b));
    assert!(m.at(a) > 1.5 * m.at(b), "low {} high {}", m.at(a), m.at(b));
    // and higher up the air is clearer still, while the haze alone is even
    assert!(m.at(at(0.2, 0.0)) < m.at(b));
    let even = f.medium(&Effects { haze: 0.5, ..Effects::default() }).unwrap();
    assert_eq!(even.at(a), even.at(b));
}

#[test]
fn far_things_fade_more_than_near_ones() {
    // a level camera a little pitched down, open ground to the horizon, and a near wall standing on it in the middle
    let f = ground(up(8.0, 0.0), Some((0.4..0.6, 0.45..0.95, 1.5)));
    let fog = Effects { haze: 0.3, mist: 0.6, mist_height: 0.3, fade: 1.0, ..Effects::default() };
    let tau = f.fade(&fog).unwrap();
    let at = |u: f32, v: f32| tau.m[(v * H as f32) as usize * W + (u * W as f32) as usize];
    let (far_ground, near_ground, wall) = (at(0.2, 0.38), at(0.2, 0.97), at(0.5, 0.7));
    assert!(far_ground > 0.25, "the far ground hardly fades: {far_ground}");
    assert!(far_ground > 3.0 * near_ground, "far {far_ground}, near ground {near_ground}");
    // no flat veil over near things: the wall is near and stands out of the mist
    assert!(wall < 0.2 * far_ground, "the near wall {wall}, the far ground {far_ground}");
    // the haze alone fades by distance too, and less with less Distance Fade
    let haze = Effects { haze: 0.6, fade: 1.0, ..Effects::default() };
    let t = f.fade(&haze).unwrap();
    let at2 = |u: f32, v: f32| t.m[(v * H as f32) as usize * W + (u * W as f32) as usize];
    assert!(at2(0.2, 0.38) > 3.0 * at2(0.5, 0.7));
    let half = f.fade(&Effects { fade: 0.5, ..haze }).unwrap();
    assert!(half.m[60 * W + 30] < t.m[60 * W + 30]);
}

#[test]
fn no_mist_and_no_fade_leave_the_beam_as_it_was() {
    let f = ground(up(8.0, 0.0), Some((0.4..0.6, 0.45..0.95, 1.5)));
    let sun = Light {
        x: 0.5,
        y: 0.5,
        depth: 0.5,
        amount: 1.0,
        softness: 0.0,
        warmth: 0.0,
        wrap: 0.4,
        kind: LightKind::Sun(Sun { azimuth: 160.0, elevation: 15.0 }),
    };
    let rig = f.rig(&sun, 0.0, 0.0);
    let shafts = f.shafts(&rig, &sun, false).unwrap();
    let haze = Effects { haze: 0.6, dust: 0.4, ..Effects::default() };
    let plain = f.beam_plane(&f.beam(&shafts, &rig, &haze));
    // Mist Height and Distance Fade moved, Mist off: the beam to the bit, and nothing fades
    let moved = Effects { mist_height: 0.9, fade: 0.0, ..haze };
    assert!(f.beam_plane(&f.beam(&shafts, &rig, &moved)).m == plain.m, "Mist Height changed a beam with no mist");
    assert!(f.fade(&moved).is_none() && f.fade(&Effects { fade: 1.0, ..Effects::default() }).is_none());
    assert!(f.medium(&Effects { mist_height: 1.0, fade: 1.0, ..Effects::default() }).is_none());
    // with mist the beam changes, and mist alone (no haze) makes one
    let misty = f.beam_plane(&f.beam(&shafts, &rig, &Effects { mist: 0.7, ..haze }));
    let sum = |m: &[f32]| m.iter().sum::<f32>();
    assert!(sum(&plain.m) > 0.0 && misty.m != plain.m, "{} {}", sum(&plain.m), sum(&misty.m));
    let only = f.beam_plane(&f.beam(&shafts, &rig, &Effects { mist: 0.7, ..Effects::default() }));
    assert!(only.m.iter().any(|v| *v > 0.0));
}

#[test]
fn hostile_numbers_and_a_photo_with_no_ground_stay_finite() {
    // a wall facing the camera everywhere: no ground; the plane lies under the lowest thing
    let f = Field::from_parts(W, H, vec![0.0; W * H], vec![-2.0; W * H]).unwrap();
    let g = f.ground();
    assert!(g.level > 0.0 && g.up.iter().all(|c| c.is_finite()), "{g:?}");
    let bad = Effects { haze: f64::NAN, mist: 1e9, mist_height: f64::NEG_INFINITY, fade: f64::NAN, ..Effects::default() };
    if let Some(t) = f.fade(&Effects { fade: 1.0, ..bad }) {
        assert!(t.m.iter().all(|v| v.is_finite() && *v >= 0.0));
    }
    let c = f.air_colour([f32::NAN, 1.0, -3.0]);
    assert!(c.iter().all(|v| v.is_finite() && *v >= 0.0), "{c:?}");
}
