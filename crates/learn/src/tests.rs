//! Learn My Edits on procedural photos whose edits follow known rules.

use lightcraft_develop::DevelopSettings;

use crate::sliders::{self, Group, SLIDERS};
use crate::train::{self, Example};
use crate::{Error, Features, Profile, features, layout};

/// A procedural photo: a sky-to-ground gradient with a few blobs, at brightness `light` (0..1) and a colour cast
/// `cast` (−1 blue … 1 yellow), 96 × 64.
pub(crate) fn scene(seed: u32, light: f32, cast: f32) -> (usize, usize, Vec<u8>) {
    let (w, h) = (96usize, 64usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    let r = |k: u32| ((seed.wrapping_mul(2_654_435_761).wrapping_add(k.wrapping_mul(40_503)) >> 8) % 1000) as f32 / 1000.0;
    let blobs: Vec<(f32, f32, f32, [f32; 3])> = (0..4).map(|k| (r(k * 4), r(k * 4 + 1), 0.08 + 0.15 * r(k * 4 + 2), [r(k * 7 + 3), r(k * 7 + 4), r(k * 7 + 5)])).collect();
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut c = if fy < 0.45 { [0.45, 0.6, 0.85] } else { [0.35 + 0.2 * r(9), 0.45, 0.25] };
            for (bx, by, br, col) in &blobs {
                if (fx - bx).hypot(fy - by) < *br {
                    c = *col;
                }
            }
            let texture = 0.9 + 0.2 * (((x * 7 + y * 13 + seed as usize) % 17) as f32 / 17.0);
            let tint = [1.0 + 0.25 * cast, 1.0 + 0.05 * cast, 1.0 - 0.3 * cast];
            for (v, t) in c.iter().zip(tint) {
                let lin = (v * texture * t * light * 1.6).clamp(0.0, 1.0);
                rgb.push((lin.powf(1.0 / 2.2) * 255.0).round() as u8);
            }
        }
    }
    (w, h, rgb)
}

fn idx(id: &str) -> usize {
    SLIDERS.iter().position(|s| s.id == id).unwrap_or(usize::MAX)
}

fn stats_profile(groups: &[Group]) -> Profile {
    Profile::new("Test", "Landscapes", Features { encoder: None, layout: layout(false) }, groups)
}

/// An example whose edit follows `rule(light, cast) -> settings`.
fn example(seed: u32, light: f32, cast: f32, rule: &dyn Fn(f32, f32) -> DevelopSettings) -> Example {
    let (w, h, rgb) = scene(seed, light, cast);
    let defaults = DevelopSettings::default();
    Example { photo: format!("p{seed}"), features: features::photo_stats(w, h, &rgb).unwrap(), targets: sliders::targets(&rule(light, cast), &defaults) }
}

/// The rule: darker photos got more Exposure; bluer ones a warmer Temp.
fn rule(light: f32, cast: f32) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.light.exposure = (-(light / 0.5).log2() * 1.0) as f64;
    s.wb.temp = 1e6 / (1e6 / 5500.0 + 25.0 * cast as f64);
    s.wb.temp = (s.wb.temp).round();
    s.light.shadows = 20.0;
    s
}

fn grid(n: u32, offset: u32) -> Vec<(u32, f32, f32)> {
    (0..n).map(|i| (i + offset, 0.2 + 0.7 * ((i * 37 + offset) % 101) as f32 / 100.0, -1.0 + 2.0 * ((i * 53 + offset * 3) % 97) as f32 / 96.0)).collect()
}

#[test]
fn a_profile_learns_that_darker_photos_got_more_exposure() {
    let mut p = stats_profile(&[Group::Tone, Group::WhiteBalance]);
    for (seed, light, cast) in grid(40, 0) {
        p.add_example(example(seed, light, cast, &rule)).unwrap();
    }
    let report = p.train(&|| false, &mut |_, _| {}).unwrap().clone();
    assert!(report.groups.iter().all(|g| g.kept), "{report:?}");
    assert!(report.beats_defaults >= 36, "{report:?}");
    assert!(report.ratio < 0.4, "{report:?}");
    // photos it never saw
    let mut errors = Vec::new();
    for (seed, light, cast) in grid(20, 500) {
        let (w, h, rgb) = scene(seed, light, cast);
        let f = features::photo_stats(w, h, &rgb).unwrap();
        let out = crate::apply(&p, &f, &DevelopSettings::default(), &DevelopSettings::default()).unwrap();
        let want = rule(light, cast);
        errors.push(((out.light.exposure - want.light.exposure).abs(), (1e6 / out.wb.temp - 1e6 / want.wb.temp).abs(), (out.light.shadows - 20.0).abs()));
    }
    let n = errors.len() as f64;
    let exposure = errors.iter().map(|e| e.0).sum::<f64>() / n;
    let mired = errors.iter().map(|e| e.1).sum::<f64>() / n;
    let shadows = errors.iter().map(|e| e.2).sum::<f64>() / n;
    // the rule spans about 2 EV and 50 mired
    assert!(exposure < 0.2, "exposure error {exposure} EV");
    assert!(mired < 6.0, "temp error {mired} mired");
    assert!(shadows < 1.0, "shadows error {shadows}");
    let (dark, bright) = (scene(900, 0.25, 0.0), scene(901, 0.85, 0.0));
    let e = |s: &(usize, usize, Vec<u8>)| p.predict(&features::photo_stats(s.0, s.1, &s.2).unwrap()).unwrap().iter().find(|(i, _)| *i == idx("light.exposure")).map(|(_, v)| *v).unwrap();
    assert!(e(&dark) > e(&bright) + 1.0, "dark {} bright {}", e(&dark), e(&bright));
}

#[test]
fn applying_writes_only_the_learned_groups() {
    let mut p = stats_profile(&[Group::Tone]);
    for (seed, light, cast) in grid(20, 0) {
        // the examples also changed the mixer and grading: not learned, so never written
        p.add_example(example(seed, light, cast, &|l, c| {
            let mut s = rule(l, c);
            s.mixer.blue.sat = -30.0;
            s.grading.shadows.sat = 40.0;
            s
        }))
        .unwrap();
    }
    p.train(&|| false, &mut |_, _| {}).unwrap();
    assert_eq!(p.learned_groups(), vec![Group::Tone]);
    let mut current = DevelopSettings::default();
    current.mixer.red.hue = 12.0;
    current.color.vibrance = 15.0;
    current.wb.temp = 4100.0;
    current.vignette.amount = -20.0;
    current.crop.geometry.angle = 3.0;
    let (w, h, rgb) = scene(77, 0.3, 0.2);
    let out = crate::apply(&p, &features::photo_stats(w, h, &rgb).unwrap(), &current, &DevelopSettings::default()).unwrap();
    assert!(out.light.exposure > 0.5, "{}", out.light.exposure);
    let mut back = out.clone();
    back.light = current.light;
    assert_eq!(back, current, "only the Tone sliders change");
    // Tone sliders no example changed stay as they were
    assert_eq!(out.light.contrast, current.light.contrast);
    assert_eq!(out.light.whites, current.light.whites);
}

#[test]
fn too_few_or_disagreeing_examples_are_refused() {
    let mut p = stats_profile(&[Group::Tone]);
    for (seed, light, cast) in grid(9, 0) {
        p.add_example(example(seed, light, cast, &rule)).unwrap();
    }
    let e = p.train(&|| false, &mut |_, _| {}).unwrap_err();
    assert!(matches!(e, Error::Train(_)) && e.to_string().contains("at least 10"), "{e}");
    assert!(p.model.is_none() && p.problem.is_some());

    // exposure that has nothing to do with the photo, as often up as down
    let mut p = stats_profile(&[Group::Tone]);
    for (seed, light, cast) in grid(40, 0) {
        let up = (seed * 7919) % 2 == 0;
        let amount = 0.5 + ((seed * 31) % 10) as f64 / 10.0;
        p.add_example(example(seed, light, cast, &|_, _| {
            let mut s = DevelopSettings::default();
            s.light.exposure = if up { amount } else { -amount };
            s
        }))
        .unwrap();
    }
    let e = p.train(&|| false, &mut |_, _| {}).unwrap_err();
    assert!(e.to_string().contains("don't share a pattern"), "{e}");
    assert!(p.predict(&[0.0; crate::PHOTO_DIMS]).unwrap_err().to_string().contains("no model"));

    // nothing edited in the chosen group
    let mut p = stats_profile(&[Group::ColorGrading]);
    for (seed, light, cast) in grid(12, 0) {
        p.add_example(example(seed, light, cast, &rule)).unwrap();
    }
    assert!(p.train(&|| false, &mut |_, _| {}).unwrap_err().to_string().contains("nothing to learn"));
}

#[test]
fn a_cancelled_training_keeps_the_profile() {
    let mut p = stats_profile(&[Group::Tone]);
    for (seed, light, cast) in grid(12, 0) {
        p.add_example(example(seed, light, cast, &rule)).unwrap();
    }
    let before = p.clone();
    assert!(matches!(p.train(&|| true, &mut |_, _| {}), Err(Error::Cancelled)));
    assert_eq!(p, before);
}

#[test]
fn targets_and_settings_round_trip() {
    let defaults = DevelopSettings::for_raw(5200.0, 4.0);
    let mut s = defaults.clone();
    s.wb.temp = 6100.0;
    s.wb.tint = 12.0;
    s.light.exposure = 0.7;
    s.grading.shadows = lightcraft_develop::Wheel { hue: 220.0, sat: 30.0, lum: -10.0 };
    s.grading.highlights = lightcraft_develop::Wheel { hue: 350.0, sat: 12.0, lum: 0.0 };
    s.curve.master = vec![lightcraft_geom::Point::new(0.0, 0.05), lightcraft_geom::Point::new(0.5, 0.55), lightcraft_geom::Point::new(1.0, 0.95)];
    let t = sliders::targets(&s, &defaults);
    let all: Vec<(usize, f32)> = t.iter().copied().enumerate().collect();
    let mut out = defaults.clone();
    sliders::write(&mut out, &defaults, &all);
    assert!((out.wb.temp - 6100.0).abs() < 1.0, "{}", out.wb.temp);
    assert!((out.wb.tint - 12.0).abs() < 1e-3);
    assert!((out.light.exposure - 0.7).abs() < 1e-5);
    assert_eq!((out.grading.shadows.hue, out.grading.shadows.sat, out.grading.shadows.lum), (220.0, 30.0, -10.0));
    assert_eq!((out.grading.highlights.hue, out.grading.highlights.sat), (350.0, 12.0));
    let t2 = sliders::targets(&out, &defaults);
    for (a, b) in t.iter().zip(&t2) {
        assert!((a - b).abs() < 0.6, "{t:?} vs {t2:?}");
    }
    // an unedited photo has no targets, a warmer Temp a positive one
    assert!(sliders::targets(&defaults, &defaults).iter().all(|v| *v == 0.0));
    assert!(t[idx("wb.temp")] > 0.0);
}

#[test]
fn statistics_are_finite_for_any_photo_and_bad_buffers_are_errors() {
    for (w, h) in [(1, 1), (3, 500), (500, 2), (17, 13)] {
        for fill in [0u8, 255, 128] {
            let f = features::photo_stats(w, h, &vec![fill; w * h * 3]).unwrap();
            assert_eq!(f.len(), crate::PHOTO_DIMS);
            assert!(f.iter().all(|v| v.is_finite()));
        }
    }
    let (w, h, rgb) = scene(1, 0.5, 0.0);
    assert_eq!(features::photo_stats(w, h, &rgb).unwrap().len(), crate::PHOTO_DIMS);
    assert!(features::photo_stats(0, 10, &[]).is_err());
    assert!(features::photo_stats(10, 10, &[0; 299]).is_err());
    assert!(features::photo_stats(usize::MAX, 2, &[0; 6]).is_err());
    assert!(features::resample_rgb(4, 4, &[0; 47], 2, 2).is_err());
    let small = features::resample_rgb(w, h, &rgb, 7, 5).unwrap();
    assert_eq!(small.len(), 7 * 5 * 3);
}

#[test]
fn damaged_profile_files_are_errors() {
    let mut p = stats_profile(&[Group::Tone]);
    for (seed, light, cast) in grid(12, 0) {
        p.add_example(example(seed, light, cast, &rule)).unwrap();
    }
    p.train(&|| false, &mut |_, _| {}).unwrap();
    let good = p.to_json().unwrap();
    assert_eq!(Profile::from_json(&good).unwrap(), p);
    let mut v: serde_json::Value = serde_json::from_str(&good).unwrap();
    let cases: Vec<(&str, Box<dyn Fn(&mut serde_json::Value)>)> = vec![
        ("format", Box::new(|v| v["format"] = "x".into())),
        ("newer", Box::new(|v| v["version"] = 99.into())),
        ("no version", Box::new(|v| v["version"] = serde_json::Value::Null)),
        ("short features", Box::new(|v| v["examples"][0]["features"] = serde_json::json!([1.0]))),
        ("short targets", Box::new(|v| v["examples"][0]["targets"] = serde_json::json!([]))),
        ("huge number", Box::new(|v| v["examples"][0]["targets"][0] = 1e30.into())),
        ("alpha", Box::new(|v| v["model"]["groups"][0]["alpha"] = serde_json::json!([1.0, 2.0]))),
        ("model examples", Box::new(|v| v["model"]["examples"] = 5000.into())),
        ("zero scale", Box::new(|v| v["model"]["standardizer"]["scale"][0] = 0.0.into())),
        ("foreign slider", Box::new(|v| v["model"]["groups"][0]["sliders"][0] = "mixer.red.hue".into())),
        ("sliders", Box::new(|v| v["sliders"][0] = "nope".into())),
        ("empty name", Box::new(|v| v["name"] = " ".into())),
        ("layout", Box::new(|v| v["features"]["layout"]["photo"] = 3.into())),
        ("encoder", Box::new(|v| v["features"]["encoder"] = "dinov2-small".into())),
        ("gamma", Box::new(|v| v["model"]["groups"][0]["kernel"]["rbf"] = (-1.0).into())),
    ];
    for (what, change) in &cases {
        let mut bad = v.clone();
        change(&mut bad);
        assert!(Profile::from_json(&bad.to_string()).is_err(), "{what} was accepted");
    }
    for text in ["", "{", "[]", "null", "{\"format\": 1}", &"[".repeat(100_000), &good[..good.len() / 2]] {
        assert!(Profile::from_json(text).is_err());
    }
    v["name"] = "Renamed".into();
    assert_eq!(Profile::from_json(&v.to_string()).unwrap().name, "Renamed");
    // a file
    let dir = std::env::temp_dir().join(format!("lc-learn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.lclearn");
    p.write(&path).unwrap();
    assert_eq!(Profile::read(&path).unwrap(), p);
    std::fs::write(&path, b"\xff\xfe garbage").unwrap();
    assert!(Profile::read(&path).is_err());
    assert!(Profile::read(&dir.join("missing.lclearn")).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_or_damaged_encoder_is_an_error() {
    let dir = std::env::temp_dir().join(format!("lc-learn-enc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(matches!(crate::encoder::Encoder::load_cpu(&dir.join("model.safetensors")), Err(Error::Model(_))));
    let junk = dir.join("junk.safetensors");
    std::fs::write(&junk, [0xffu8; 64]).unwrap();
    assert!(matches!(crate::encoder::Encoder::load_cpu(&junk), Err(Error::Model(_))));
    // a real safetensors file with the wrong tensors
    let header = r#"{"x":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
    let mut b = (header.len() as u64).to_le_bytes().to_vec();
    b.extend_from_slice(header.as_bytes());
    b.extend_from_slice(&[0u8; 4]);
    std::fs::write(&junk, b).unwrap();
    assert!(matches!(crate::encoder::Encoder::load_cpu(&junk), Err(Error::Model(_))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_encoder_gives_finite_features_when_installed() {
    let Some(path) = std::env::var_os("LIGHTCRAFT_LEARN_MODEL").map(std::path::PathBuf::from).filter(|p| p.is_file()) else {
        lightcraft_testkit::skip!(Model, "set LIGHTCRAFT_LEARN_MODEL to DINOv2-small's model.safetensors");
        return;
    };
    let e = crate::encoder::Encoder::load_cpu(&path).unwrap();
    let (w, h, rgb) = scene(3, 0.5, 0.0);
    let a = e.features(w, h, &rgb).unwrap();
    assert_eq!(a.len(), crate::encoder::DIMS);
    assert!(a.iter().all(|v| v.is_finite()));
    // DINOv2 sees what a photo shows, not its brightness or colour: a brighter, warmer copy stays closer to the
    // photo than a different one (made the same way, so already close) does
    let (_, _, warm) = scene(3, 0.7, 0.4);
    let (w2, h2, rgb2) = scene(4, 0.5, 0.0);
    let cos = |p: &[f32], q: &[f32]| {
        let (p, q) = (&p[..384], &q[..384]);
        p.iter().zip(q).map(|(x, y)| x * y).sum::<f32>() / (p.iter().map(|x| x * x).sum::<f32>().sqrt() * q.iter().map(|x| x * x).sum::<f32>().sqrt())
    };
    let same = cos(&a, &e.features(w, h, &warm).unwrap());
    let other = cos(&a, &e.features(w2, h2, &rgb2).unwrap());
    assert!(same > other && same > 0.98, "same photo {same}, another {other}");
    let _ = train::MIN_EXAMPLES;
}
