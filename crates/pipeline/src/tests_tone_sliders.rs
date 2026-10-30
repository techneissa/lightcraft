//! Contrast, Whites and Blacks are strong sliders on every tone path: camera-look raws, raws
//! without a look and display-referred files (JPEG, PNG) respond alike. Process V1 and V2 keep
//! their original (weaker) display and camera response (`docs/process-versions.md`).

use lightcraft_develop::{DevelopSettings, Process};

use crate::SourceInfo;
use crate::finish::base_tone;
use crate::tone::{CameraTone, ToneMap};

#[derive(Clone, Copy, Debug)]
enum Path {
    Camera,
    Raw,
    Display,
}

const PATHS: [Path; 3] = [Path::Camera, Path::Raw, Path::Display];

fn camera() -> CameraTone {
    // A camera-like curve: a gentle S through the midtones, rolling off to 1.
    let knots = std::array::from_fn(|i| {
        let x = 0.004 * 1.18f32.powi(i as i32);
        [x, 1.0 - (-2.2 * x).exp()]
    });
    CameraTone::new(knots).expect("valid knots")
}

fn tone(process: Process, path: Path, contrast: f64, whites: f64, blacks: f64) -> ToneMap {
    let mut s = DevelopSettings { process: process.version(), ..DevelopSettings::default() };
    s.light.contrast = contrast;
    s.light.whites = whites;
    s.light.blacks = blacks;
    let info = match path {
        Path::Camera => SourceInfo { raw: true, camera_tone: Some(camera()), ..SourceInfo::default() },
        Path::Raw => SourceInfo { raw: true, ..SourceInfo::default() },
        Path::Display => SourceInfo::default(),
    };
    base_tone(&s, &info, 1.0)
}

/// d log(output) / d log(input) at middle grey, over ±0.25 EV.
fn grey_slope(t: &ToneMap) -> f32 {
    let (lo, hi) = (0.18 * 2f32.powf(-0.25), 0.18 * 2f32.powf(0.25));
    (t.apply(hi) / t.apply(lo)).log2() / 0.5
}

fn at(process: Process, path: Path, c: f64, w: f64, b: f64, y: f32) -> f32 {
    tone(process, path, c, w, b).apply(y)
}

fn contrast_ratio(process: Process, path: Path, c: f64) -> f32 {
    grey_slope(&tone(process, path, c, 0.0, 0.0)) / grey_slope(&tone(process, path, 0.0, 0.0, 0.0))
}

/// Prints the measurements (`cargo test -p lightcraft-pipeline tests_tone_sliders -- --nocapture`).
#[test]
fn slider_response_report() {
    for process in [Process::V2, Process::V3] {
        for path in PATHS {
            let f = |c, w, b, y| at(process, path, c, w, b, y);
            eprintln!(
                "{process:?} {path:?}: contrast slope x{:.2} / x{:.2} | whites@0.7 {:.3} {:.3} {:.3} @0.9 {:.3} {:.3} {:.3} | blacks@0.01 {:.4} {:.4} {:.4} @0.03 {:.4} {:.4} {:.4}",
                contrast_ratio(process, path, 100.0),
                contrast_ratio(process, path, -100.0),
                f(0., -100., 0., 0.7),
                f(0., 0., 0., 0.7),
                f(0., 100., 0., 0.7),
                f(0., -100., 0., 0.9),
                f(0., 0., 0., 0.9),
                f(0., 100., 0., 0.9),
                f(0., 0., -100., 0.01),
                f(0., 0., 0., 0.01),
                f(0., 0., 100., 0.01),
                f(0., 0., -100., 0.03),
                f(0., 0., 0., 0.03),
                f(0., 0., 100., 0.03),
            );
        }
    }
}

/// Visible at +-30, strong but not destructive at +-100, on every path.
#[test]
fn v3_sliders_are_visible_and_bounded_on_every_path() {
    let p = Process::V3;
    for path in PATHS {
        let f = |c, w, b, y| at(p, path, c, w, b, y);
        let n = |y| f(0., 0., 0., y);
        // Contrast: clearly visible at +-30, a strong but bounded S-curve at +-100
        let (c30, c100) = (contrast_ratio(p, path, 30.0), contrast_ratio(p, path, 100.0));
        let (m30, m100) = (contrast_ratio(p, path, -30.0), contrast_ratio(p, path, -100.0));
        assert!(c30 >= 1.1 && (1.45..=1.7).contains(&c100), "{path:?}: contrast + x{c30} x{c100}");
        assert!(m30 <= 0.92 && (0.5..=0.7).contains(&m100), "{path:?}: contrast - x{m30} x{m100}");
        // Whites: +100 pushes bright tones into clipping but not the midtones; -100 pulls white down
        assert!(f(0., 30., 0., 0.8) >= n(0.8) * 1.1 || n(0.8) >= 0.97, "{path:?}: whites +30");
        assert!(f(0., -30., 0., 0.8) <= n(0.8) * 0.9, "{path:?}: whites -30");
        for y in [0.7, 0.9] {
            assert!(f(0., 100., 0., y) >= 0.97, "{path:?}: whites +100 at {y}: {}", f(0., 100., 0., y));
            assert!(f(0., -100., 0., y) <= 0.7 * n(y) && f(0., -100., 0., y) >= 0.3 * n(y), "{path:?}: whites -100 at {y}");
        }
        assert!(f(0., 100., 0., 0.18) < 0.9 && f(0., 100., 0., 0.18) <= 2.6 * n(0.18), "{path:?}: whites +100 spares the midtones");
        assert!(f(0., -100., 0., 0.18) >= 0.75 * n(0.18), "{path:?}: whites -100 spares the midtones");
        // Blacks: -30 visible; -100 crushes the deepest tones only; +100 lifts them visibly
        assert!(f(0., 0., -30., 0.01) <= 0.75 * n(0.01), "{path:?}: blacks -30");
        assert!(f(0., 0., -100., 0.01) <= 0.1 * n(0.01).max(0.01), "{path:?}: blacks -100 at 0.01");
        assert!(f(0., 0., -100., 0.03) <= 0.7 * n(0.03), "{path:?}: blacks -100 at 0.03");
        assert!(f(0., 0., -100., 0.18) >= 0.8 * n(0.18), "{path:?}: blacks -100 spares the midtones");
        for y in [0.01, 0.03] {
            assert!(f(0., 0., 30., y) >= n(y) + 0.008, "{path:?}: blacks +30 at {y}");
            assert!(f(0., 0., 100., y) >= n(y) + 0.02, "{path:?}: blacks +100 at {y}");
        }
        assert!(f(0., 0., 100., 0.18) <= 1.25 * n(0.18), "{path:?}: blacks +100 spares the midtones");
    }
}

/// The three paths agree on how strong each slider is.
#[test]
fn v3_paths_agree_within_tolerance() {
    let p = Process::V3;
    for c in [100.0, -100.0] {
        let r: Vec<f32> = PATHS.iter().map(|&x| contrast_ratio(p, x, c)).collect();
        assert!(r.iter().all(|v| (v - r[1]).abs() < 0.2), "contrast {c}: {r:?}");
    }
    for w in [100.0, -100.0] {
        // the change in EV of a bright tone
        let r: Vec<f32> = PATHS.iter().map(|&x| (at(p, x, 0., w, 0., 0.8) / at(p, x, 0., 0., 0., 0.8)).log2().clamp(-3.0, 1.0)).collect();
        assert!(r.iter().all(|v| (v - r[1]).abs() < 0.6), "whites {w}: {r:?}");
    }
}

/// Neutral sliders are the identity of each path's base curve, white stays white (issue #1945),
/// and every setting is monotone, finite and in range.
#[test]
fn v3_neutral_is_identity_and_curves_are_sane() {
    for path in PATHS {
        let neutral = tone(Process::V3, path, 0.0, 0.0, 0.0);
        let legacy = tone(Process::V2, path, 0.0, 0.0, 0.0);
        // equal up to the legacy path's gamma round trip
        assert!(neutral.lut().iter().zip(legacy.lut()).all(|(a, b)| (a - b).abs() <= 1e-5 * b.max(1.0)), "{path:?}: neutral differs from V2");
        for (c, w, b) in [
            (100., 0., 0.),
            (-100., 0., 0.),
            (0., 100., 0.),
            (0., -100., 0.),
            (0., 0., 100.),
            (0., 0., -100.),
            (100., 100., -100.),
            (-100., -100., 100.),
            (40., -30., 20.),
        ] {
            let t = tone(Process::V3, path, c, w, b);
            let mut prev = -1.0f32;
            for i in 0..2400 {
                let y = 1e-5 * 1.01f32.powi(i);
                let o = t.apply(y);
                assert!(o.is_finite() && (0.0..=1.0).contains(&o), "{path:?} {c} {w} {b} at {y}: {o}");
                assert!(o >= prev - 1e-5, "{path:?} {c} {w} {b} at {y}: {o} < {prev}");
                prev = o;
            }
        }
    }
    let display = tone(Process::V3, Path::Display, 0.0, 0.0, 0.0);
    assert!((display.apply(1.0) - 1.0).abs() < 1e-3);
}

/// Whites +100 reaches clipping in a bright tone that is below white in the file.
#[test]
fn v3_whites_reach_clipping_on_display_sources() {
    let t = tone(Process::V3, Path::Display, 0.0, 100.0, 0.0);
    assert!(t.apply(0.7) > 0.9999);
    assert!(t.apply(0.3) < 0.8, "the midtones are not clipped");
}

/// HDR renders keep working: bounded by the peak, monotone, SDR range as SDR.
#[test]
fn v3_hdr_peak_is_bounded_and_monotone() {
    for path in PATHS {
        for (c, w, b) in [(0., 0., 0.), (100., 100., -100.), (-60., -100., 100.)] {
            let mut s = DevelopSettings { process: Process::V3.version(), ..DevelopSettings::default() };
            (s.light.contrast, s.light.whites, s.light.blacks) = (c, w, b);
            let info = match path {
                Path::Camera => SourceInfo { raw: true, camera_tone: Some(camera()), ..SourceInfo::default() },
                Path::Raw => SourceInfo { raw: true, ..SourceInfo::default() },
                Path::Display => SourceInfo::default(),
            };
            let t = base_tone(&s, &info, 4.0);
            let mut prev = -1.0f32;
            for i in 0..2400 {
                let o = t.apply(1e-5 * 1.01f32.powi(i));
                assert!(o.is_finite() && (0.0..=4.0).contains(&o) && o >= prev - 1e-5, "{path:?} {c} {w} {b}: {o} < {prev}");
                prev = o;
            }
        }
    }
}
