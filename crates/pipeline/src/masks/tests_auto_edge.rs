//! Auto Mask brush on synthetic edges (issue #280): how much paint leaks onto the far side of an
//! edge, and how much of the near side, inside the brush, is left unpainted.

use super::super::*;
use lightcraft_develop::DevelopSettings;

const W: usize = 400;
const H: usize = 200;

/// Which side of the scene's edge a pixel is on.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    /// The surface under the stroke.
    Near,
    /// Across the edge.
    Far,
    /// On the edge itself (counted neither way).
    Edge,
}

/// A deterministic value in -1..1 for pixel `(x, y)` and channel `k`.
fn noise(x: usize, y: usize, k: usize) -> f32 {
    let mut v = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (k as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    v ^= v >> 31;
    v = v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v ^= v >> 29;
    (v % 20_001) as f32 / 10_000.0 - 1.0
}

/// A scene: the photo, and the side of each pixel.
struct Scene {
    img: Rgb32f,
    side: Vec<Side>,
}

/// A vertical edge at `x = W / 2`: `near(x, y)` left of it, `far(x, y)` right of it.
fn split(near: impl Fn(usize, usize) -> [f32; 3], far: impl Fn(usize, usize) -> [f32; 3]) -> Scene {
    let e = W / 2;
    let img = Rgb32f::from_fn(W, H, |x, y| if x < e { near(x, y) } else { far(x, y) });
    let side = (0..W * H)
        .map(|i| {
            let x = i % W;
            if x + 1 < e {
                Side::Near
            } else if x > e {
                Side::Far
            } else {
                Side::Edge
            }
        })
        .collect();
    Scene { img, side }
}

/// Multiply each channel by `2^(ev · noise)` (photon-like, per channel).
fn grain(c: [f32; 3], x: usize, y: usize, ev: f32, chroma: f32) -> [f32; 3] {
    let l = ev * noise(x, y, 9);
    [0, 1, 2].map(|k| c[k] * (l + chroma * noise(x, y, k)).exp2())
}

/// The named scenes.
fn scenes() -> Vec<(&'static str, Scene)> {
    let grey = |v: f32| move |_: usize, _: usize| [v; 3];
    vec![
        ("hard edge (3.6 EV)", split(grey(0.03), |_, _| [0.6, 0.5, 0.4])),
        ("colour only (same Y)", split(|_, _| [0.30, 0.12, 0.08], |_, _| [0.10, 0.17, 0.12])),
        ("low contrast (0.5 EV)", split(grey(0.18), |_, _| [0.27, 0.25, 0.23])),
        ("noisy surface", split(|x, y| grain([0.12, 0.10, 0.08], x, y, 0.35, 0.15), |x, y| grain([0.5, 0.45, 0.4], x, y, 0.1, 0.05))),
        (
            "textured surface",
            split(
                |x, y| {
                    let t = if (x / 3 + y / 3) % 2 == 0 { 1.0 } else { 0.6 };
                    grain([0.2 * t, 0.25 * t, 0.1 * t], x, y, 0.15, 0.05)
                },
                |x, y| grain([0.6, 0.55, 0.5], x, y, 0.05, 0.02),
            ),
        ),
        ("blurred edge (2 EV)", blurred()),
        ("dark noisy vs darker", split(|x, y| grain([0.02, 0.018, 0.015], x, y, 0.5, 0.3), |x, y| grain([0.004; 3], x, y, 0.5, 0.3))),
    ]
}

/// A 2 EV edge blurred over about 4 px (a lens's), with a little grain; the 3 px either side of
/// its middle count as the edge.
fn blurred() -> Scene {
    let e = (W / 2) as f32;
    let img = Rgb32f::from_fn(W, H, |x, y| {
        let t = 1.0 / (1.0 + (-(x as f32 + 0.5 - e) / 1.0).exp());
        let (a, b) = ([0.08, 0.1, 0.12], [0.32, 0.36, 0.3]);
        grain([0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t), x, y, 0.05, 0.02)
    });
    let side = (0..W * H)
        .map(|i| {
            let x = (i % W) as f32 + 0.5;
            if x < e - 3.0 {
                Side::Near
            } else if x > e + 3.0 {
                Side::Far
            } else {
                Side::Edge
            }
        })
        .collect();
    Scene { img, side }
}

/// A soft horizontal gradient over the whole frame: every pixel is "near".
fn gradient() -> Scene {
    let img = Rgb32f::from_fn(W, H, |x, _| {
        let t = x as f32 / W as f32;
        [0.05 + 0.5 * t, 0.06 + 0.4 * t, 0.08 + 0.3 * t]
    });
    Scene { img, side: vec![Side::Near; W * H] }
}

/// A flat surface crossed by a dark line 2 px wide at `x = W / 2`; past it, the same surface.
fn thin_line() -> Scene {
    let e = W / 2;
    let img = Rgb32f::from_fn(W, H, |x, y| if (e..e + 2).contains(&x) { [0.01; 3] } else { grain([0.3, 0.28, 0.25], x, y, 0.05, 0.02) });
    let side = (0..W * H)
        .map(|i| {
            let x = i % W;
            if x + 1 < e {
                Side::Near
            } else if (e..e + 2).contains(&x) {
                Side::Far
            } else {
                // (past the line: the same surface; whether it is painted is not measured)
                Side::Edge
            }
        })
        .collect();
    Scene { img, side }
}

fn frame() -> Frame {
    Frame::new(W, H, &DevelopSettings::default(), true)
}

/// A vertical stroke `gap` px left of the edge (its centre line), radius `r` px, from `y = 0.15` to `0.85`;
/// `wobble` px of sideways zigzag.
fn stroke(gap: f64, r: f64, wobble: f64, auto_mask: bool) -> BrushStroke {
    let x = |dx: f64| ((W / 2) as f64 - gap + dx) / W as f64;
    let n = 24;
    let points = (0..=n)
        .map(|k| {
            let dx = if k % 2 == 0 { -wobble } else { wobble };
            Point::new(x(dx), 0.15 + 0.7 * k as f64 / n as f64)
        })
        .collect();
    BrushStroke { points, size: r / W as f64, feather: 50.0, auto_mask, ..Default::default() }
}

/// What a stroke painted on a scene: (leak, holes), both percentages. Leak: the paint on the far side
/// as a share of the far side the plain brush covers. Holes: the share of the near side under the plain
/// brush's full-strength core left below half strength.
fn measure(scene: &Scene, s: &BrushStroke) -> (f64, f64) {
    let f = frame();
    let l = scene.img.map(crate::local::log_lum);
    let auto = shape_alpha(&MaskShape::Brush { strokes: vec![s.clone()] }, &f, W, H, &scene.img, &l, 0.0, None);
    let plain = shape_alpha(&MaskShape::Brush { strokes: vec![BrushStroke { auto_mask: false, ..s.clone() }] }, &f, W, H, &scene.img, &l, 0.0, None);
    let (mut leak, mut far, mut holes, mut core) = (0.0f64, 0usize, 0usize, 0usize);
    for i in 0..W * H {
        let (a, p) = (auto.data[i], plain.data[i]);
        match scene.side[i] {
            Side::Far => {
                leak += f64::from(a);
                far += usize::from(p > 0.0);
            }
            Side::Near if p > 0.95 => {
                core += 1;
                holes += usize::from(a < 0.5);
            }
            _ => {}
        }
    }
    (100.0 * leak / far.max(1) as f64, 100.0 * holes as f64 / core.max(1) as f64)
}

/// Prints the leak and holes of every scene (`cargo test -p lightcraft-pipeline --lib auto_edge_report -- --ignored --nocapture`).
#[test]
#[ignore = "a report, not a check"]
fn auto_edge_report() {
    let r = 20.0;
    let mut rows = Vec::new();
    for (name, sc) in scenes() {
        for gap in [0.6 * r, 0.3 * r, 0.1 * r] {
            let (leak, holes) = measure(&sc, &stroke(gap, r, 0.0, true));
            rows.push(format!("{name:<24} gap {gap:>4.0}px         leak {leak:6.2}%  holes {holes:6.2}%"));
        }
        let (leak, holes) = measure(&sc, &stroke(0.4 * r, r, 0.6 * r, true));
        rows.push(format!("{name:<24} gap {:>4.0}px wobble {:>2.0} leak {leak:6.2}%  holes {holes:6.2}%", 0.4 * r, 0.6 * r));
    }
    let (_, holes) = measure(&gradient(), &stroke(0.0, r, 0.0, true));
    rows.push(format!("{:<24}                     holes {holes:6.2}%", "soft gradient"));
    for gap in [0.6 * r, 0.3 * r] {
        let (leak, holes) = measure(&thin_line(), &stroke(gap, r, 0.0, true));
        rows.push(format!("{:<24} gap {gap:>4.0}px         leak {leak:6.2}%  holes {holes:6.2}%", "thin dark line"));
    }
    println!("{}", rows.join("\n"));
}

/// Issue #280: a stroke along an edge paints its own side right up to the edge and nothing past it, with the
/// centre close to the edge (2 px), on the blur of a soft edge, and when the centre slips over a clear edge.
#[test]
fn a_stroke_along_an_edge_paints_one_side_only() {
    let r = 20.0;
    for (name, sc) in scenes().into_iter().filter(|(n, _)| !n.starts_with("low contrast")) {
        for (gap, wobble) in [(0.1 * r, 0.0), (0.3 * r, 0.0), (0.4 * r, 0.6 * r)] {
            let (leak, holes) = measure(&sc, &stroke(gap, r, wobble, true));
            assert!(leak < 1.0 && holes < 1.0, "{name}, {gap} px from the edge, wobble {wobble}: leak {leak:.2} %, holes {holes:.2} %");
        }
    }
    // a 0.5 EV edge holds while the centre stays on its side
    let (_, sc) = scenes().into_iter().find(|(n, _)| n.starts_with("low contrast")).expect("scene");
    let (leak, holes) = measure(&sc, &stroke(0.1 * r, r, 0.0, true));
    assert!(leak < 1.0 && holes < 1.0, "low contrast: leak {leak:.2} %, holes {holes:.2} %");
}

/// A stroke across a soft gradient paints all of it, and one that moves on to another surface and stays there
/// paints that surface too.
#[test]
fn a_stroke_follows_a_gradient_and_a_move_to_another_surface() {
    let r = 20.0;
    let across = |auto_mask| BrushStroke {
        points: vec![Point::new(0.05, 0.5), Point::new(0.95, 0.5)],
        size: r / W as f64,
        feather: 50.0,
        auto_mask,
        ..Default::default()
    };
    let (_, holes) = measure(&gradient(), &across(true));
    assert!(holes < 1.0, "gradient: holes {holes:.2} %");
    let (_, sc) = scenes().into_iter().next().expect("scene");
    let f = frame();
    let l = sc.img.map(crate::local::log_lum);
    let a = shape_alpha(&MaskShape::Brush { strokes: vec![across(true)] }, &f, W, H, &sc.img, &l, 0.0, None);
    assert!(a.get(60, 100) > 0.99 && a.get(340, 100) > 0.99, "{} {}", a.get(60, 100), a.get(340, 100));
}
