//! LT5 / LT6: the local profile choice's colour (`super::local`).
use super::local::{LocalOutcome, fit_local_look_with, local_colour_with};
use super::*;
use crate::camera_profiles::{self, CameraProfile, LocalUnavailable};
use lightcraft_pipeline::ColourNote;

fn raw_of(format: RawFormat, model: &str, color: ColorData) -> RawImage {
    let area = lightcraft_raw::Rect::new(0, 0, 2, 2);
    RawImage {
        format,
        width: 2,
        height: 2,
        cpp: 1,
        data: lightcraft_raw::RawData::U16(vec![0; 4]),
        cfa: None,
        bits: 14,
        black: Default::default(),
        white: vec![16383.0],
        active_area: area,
        crop: area,
        orientation: Default::default(),
        color,
        wb_multipliers: Some([2.0, 1.0, 1.5]),
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: Default::default(),
        metadata: lightcraft_raw::Metadata { make: Some("SONY".into()), model: Some(model.to_owned()), ..Default::default() },
        lens_metadata: Default::default(),
    }
}

fn dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-local-colour-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn profile(model: &str) -> CameraProfile {
    let table = HsvTable { hue_divisions: 4, sat_divisions: 2, val_divisions: 1, data: vec![[4.0, 1.1, 1.0]; 8], srgb_value: false };
    CameraProfile::new(model, 40, 9000, Mat3([[1.5, -0.4, -0.1], [-0.2, 1.3, -0.1], [-0.05, -0.3, 1.35]]), Some(table)).named("A7 V Chart")
}

fn never_matching(_: &mut RawImage) -> (Xy, CameraTransform, Option<CameraLook>) {
    panic!("a usable local profile never asks Camera Matching")
}

/// The profile's colour is the base, relative to the camera transform; the tone is the fitted one; Camera Matching is
/// not consulted; a failed tone fit keeps the colour and marks the default tone.
#[test]
fn a_local_profile_gives_its_colour_and_a_fitted_tone_or_the_default_tone() {
    let d = dir("colour");
    camera_profiles::save_named(&profile("ILCE-7M5"), &d, "A7 V Chart").unwrap();
    let _d = camera_profiles::use_dir_for_tests(&d);
    let key = camera_profiles::local_key("A7 V Chart");
    let mut raw = raw_of(RawFormat::Arw, "ILCE-7M5", Default::default());
    let xy = lightcraft_raw::color::as_shot_white_xy(&raw);
    let transform = lightcraft_raw::color::camera_transform(&raw, xy);
    let want = profile("ILCE-7M5").matrix().mul(&transform.matrix.inverse().unwrap());
    let fitted = CameraLook {
        matrix: want,
        tone: CameraTone::new(std::array::from_fn(|i| {
            let x = 0.006 * 1.13f32.powi(i as i32);
            [x, x.sqrt() * 0.9]
        }))
        .unwrap(),
        hue_sat: profile("ILCE-7M5").hue_sat,
    };
    let (_, _, look, outcome) = local_colour_with(
        &mut raw,
        key,
        |_, _, colour| {
            assert_eq!(colour.0, want, "the fit is given the profile's colour, not the photo's own");
            Some(fitted.clone())
        },
        never_matching,
    )
    .unwrap();
    let look = look.unwrap();
    assert_eq!((look.matrix, look.tone), (want, fitted.tone));
    assert_eq!(outcome, LocalOutcome::default());
    // no tone fit passed: the profile's colour with LightCraft's default tone
    let (_, _, look, outcome) = local_colour_with(&mut raw, key, |_, _, _| None, never_matching).unwrap();
    let look = look.unwrap();
    assert_eq!((look.matrix, look.hue_sat.is_some()), (want, true));
    assert_eq!(outcome, LocalOutcome { default_tone: true, note: None });
    let _ = std::fs::remove_dir_all(&d);
}

/// LT6: a missing, unusable or other-camera file renders as Camera Matching (the fit is not even tried), with the note
/// that says why; a file with colour matrices of its own is not offered the choice at all.
#[test]
fn an_unusable_local_profile_renders_as_camera_matching_with_a_note() {
    let d = dir("fallback");
    camera_profiles::save_named(&profile("ILCE-7M5"), &d, "A7 V Chart").unwrap();
    std::fs::write(d.join("broken.json"), b"{}").unwrap();
    let _d = camera_profiles::use_dir_for_tests(&d);
    let matching = CameraLook { matrix: Mat3::IDENTITY, ..profile_look() };
    let run = |model: &str, key: u128| {
        let mut raw = raw_of(RawFormat::Arw, model, Default::default());
        local_colour_with(
            &mut raw,
            key,
            |_, _, _| panic!("no tone fit without a usable profile"),
            |_| {
                let xy = Xy { x: 0.3127, y: 0.329 };
                (xy, lightcraft_raw::color::camera_transform(&raw_of(RawFormat::Arw, model, Default::default()), xy), Some(matching.clone()))
            },
        )
        .unwrap()
    };
    for (what, model, id, expect) in [
        ("another camera", "ILCE-7M4", "A7 V Chart", LocalUnavailable::OtherCamera),
        ("a missing file", "ILCE-7M5", "Gone", LocalUnavailable::Missing),
        ("a damaged file", "ILCE-7M5", "broken", LocalUnavailable::Invalid),
        ("a hostile id", "ILCE-7M5", "../A7 V Chart", LocalUnavailable::Missing),
    ] {
        let (_, _, look, outcome) = run(model, camera_profiles::local_key(id));
        assert_eq!(look.map(|l| l.matrix), Some(Mat3::IDENTITY), "{what}: Camera Matching's look");
        assert_eq!(outcome.note, Some(ColourNote { why: expect, profile: id.to_owned() }), "{what}");
        assert!(!outcome.default_tone, "{what}");
    }
    // the file comes back: the profile is used again
    assert!(camera_profiles::local_profiles_for("ILCE-7M5").iter().any(|p| p.id == "A7 V Chart"));
    let (_, _, _, outcome) = {
        let mut raw = raw_of(RawFormat::Arw, "ILCE-7M5", Default::default());
        local_colour_with(&mut raw, camera_profiles::local_key("A7 V Chart"), |_, _, _| None, never_matching).unwrap()
    };
    assert_eq!(outcome.note, None);
    // a file with colour matrices of its own keeps them: not offered
    let own = lightcraft_raw::ColorData {
        color_matrix: lightcraft_raw::spectral::find(Some("NIKON CORPORATION"), "NIKON Z f").unwrap().color_matrix.map(Some),
        illuminant: [17, 21],
        ..Default::default()
    };
    let mut raw = raw_of(RawFormat::Dng, "ILCE-7M5", own);
    assert!(local_colour_with(&mut raw, camera_profiles::local_key("A7 V Chart"), |_, _, _| None, never_matching).is_none());
    let _ = std::fs::remove_dir_all(&d);
}

fn profile_look() -> CameraLook {
    CameraLook {
        matrix: Mat3::IDENTITY,
        tone: CameraTone::new(std::array::from_fn(|i| {
            let x = 0.006 * 1.13f32.powi(i as i32);
            [x, x.sqrt() * 0.9]
        }))
        .unwrap(),
        hue_sat: None,
    }
}

/// LT5 (fit half): the search takes the profile's colour as given and fits tone around it. Where Camera Matching would
/// retry with the photo's own colour after a profile that does not fit (a wrong matrix), the local choice has no look
/// at all: it keeps the profile's colour with the default tone instead.
#[test]
fn the_local_fit_never_retries_with_the_photos_own_colour() {
    let matrix = Mat3([[1.8, -0.4, -0.1], [-0.2, 1.5, -0.1], [-0.05, -0.3, 1.7]]);
    let wrong = Mat3([[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
    let mut sensor = Rgb32f::new(64, 64);
    let mut reference = sensor.clone();
    let mut seed = 0x2545_f491_u32;
    for (src, dst) in sensor.data.iter_mut().zip(&mut reference.data) {
        *src = std::array::from_fn(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            0.05 + (seed % 1000) as f32 * 0.0003
        });
        let p = matrix.apply_f32(*src);
        let y = luminance_2020(p);
        *dst = p.map(|v| (v * (1.0 - (-2.5 * y).exp()) / y).clamp(0.0, 0.999));
    }
    let clipped = vec![false; sensor.data.len()];
    // the right colour: its look, with the profile's matrix untouched
    let good = fit_local_look_with(&sensor, &reference, &clipped, &(matrix, None), false).expect("a look");
    assert_eq!(good.matrix, matrix);
    // a wrong colour: Camera Matching falls back to the photo's own fit, the local choice finds nothing
    assert!(fit_look_with(&sensor, &reference, &clipped, Some((wrong, None)), &|| None, false).is_some_and(|l| l.matrix != wrong));
    assert!(fit_local_look_with(&sensor, &reference, &clipped, &(wrong, None), false).is_none());
    // a clipped sky: still the profile's colour
    let mut clipped = clipped;
    for c in clipped.iter_mut().take(300) {
        *c = true;
    }
    let with_clipped = fit_local_look_with(&sensor, &reference, &clipped, &(matrix, None), false).expect("a look");
    assert_eq!(with_clipped.matrix, matrix);
}

/// LT12: the colour tags of a DNG written from a photo on a local profile are the profile's: its matrix (as the one
/// forward matrix, at unit white), its table, and the fitted tone (none when the tone fit failed and the photo keeps
/// the default tone). Camera Matching's tags differ.
#[test]
fn a_dng_of_a_local_profile_photo_carries_the_profiles_colour() {
    use super::colour::starting_colour_tags_with;
    let d = dir("dng");
    camera_profiles::save_named(&profile("ILCE-7M5"), &d, "A7 V Chart").unwrap();
    let _d = camera_profiles::use_dir_for_tests(&d);
    let key = camera_profiles::local_key("A7 V Chart");
    let want = profile("ILCE-7M5");
    let tone = profile_look().tone;
    let tags = |fits: bool| {
        let mut raw = raw_of(RawFormat::Arw, "ILCE-7M5", Default::default());
        starting_colour_tags_with(&mut raw, |raw| {
            local_colour_with(
                raw,
                key,
                move |_, _, colour| fits.then(|| CameraLook { matrix: colour.0, tone, hue_sat: colour.1.clone() }),
                never_matching,
            )
            .unwrap()
        })
        .expect("tags")
    };
    let fitted = tags(true);
    assert_eq!(fitted.profile.hue_sat_map[0], want.hue_sat, "the profile's table");
    assert!(fitted.profile.tone_curve.is_some(), "the fitted tone");
    // the forward matrix is the profile's matrix with white at unit brightness, in XYZ D50
    let m = want.matrix();
    let white = m.apply([1.0; 3]);
    let k = (white[0] + white[1] + white[2]) / 3.0;
    let scaled = Mat3(m.0.map(|r| r.map(|v| v / k)));
    let expected = bradford(D65, D50).mul(&REC2020.to_xyz()).mul(&scaled);
    let got = fitted.forward_matrix[0].expect("a forward matrix");
    for (a, b) in got.0.iter().flatten().zip(expected.0.iter().flatten()) {
        assert!((a - b).abs() < 1e-9, "forward matrix {got:?} vs {expected:?}");
    }
    assert!(fitted.color_matrix[0].is_some(), "the DNG declares its own colour matrix");
    // no tone fit passed: the same colour, no curve of its own
    let plain = tags(false);
    assert_eq!((plain.profile.hue_sat_map[0].clone(), plain.profile.tone_curve.is_none()), (want.hue_sat.clone(), true));
    assert_eq!(plain.forward_matrix, fitted.forward_matrix);
    // Camera Matching's tags (no look at all here) are not the profile's
    let mut raw = raw_of(RawFormat::Arw, "ILCE-7M5", Default::default());
    let xy = lightcraft_raw::color::as_shot_white_xy(&raw);
    let t = lightcraft_raw::color::camera_transform(&raw, xy);
    let matching = starting_colour_tags_with(&mut raw, |_| (xy, t, None, LocalOutcome::default())).expect("tags");
    assert_ne!(matching.forward_matrix, fitted.forward_matrix);
    assert!(matching.profile.hue_sat_map[0].is_none());
    let _ = std::fs::remove_dir_all(&d);
}

/// LT14: a preset that names a local profile, applied to a photo of another camera and to the same camera once the
/// file is gone, applies the rest of the preset and keeps the stored id; the photo renders as Camera Matching with the
/// note; with the file back it renders the profile again.
#[test]
fn a_preset_with_a_local_profile_falls_back_at_render_time_and_returns_with_the_file() {
    use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
    use lightcraft_develop::CameraColour;
    use serde_json::json;
    let d = dir("preset");
    camera_profiles::save_named(&profile("ILCE-7M5"), &d, "A7 V Chart").unwrap();
    let _d = camera_profiles::use_dir_for_tests(&d);
    let mut s = crate::Session::new();
    for (id, camera) in [(1, "SONY ILCE-7M5"), (2, "SONY ILCE-7M4"), (3, "SONY ILCE-7M5")] {
        let mut p = Photo::new(PhotoId(id), Source::File { path: format!("/p/{id}.arw") }, &format!("{id}.arw"), "ARW", 6000, 4000, "2026-10-28");
        p.kind = MediaKind::Raw;
        p.meta.camera = camera.into();
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let pick = |s: &mut crate::Session, id: u64| s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    pick(&mut s, 1);
    s.execute("develop.profile", &json!({"camera_colour": "local", "camera_profile": "A7 V Chart"})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.6})).unwrap();
    let preset = s.execute("preset.create", &json!({"name": "Chart look"})).unwrap()["id"].as_str().unwrap().to_owned();
    for id in [2, 3] {
        pick(&mut s, id);
        s.execute("preset.apply", &json!({"id": preset})).unwrap();
    }
    // (the render of a photo as its decoder makes it: the stored settings pick the file by its key)
    let matching = CameraLook { matrix: Mat3::IDENTITY, ..profile_look() };
    let render = |s: &crate::Session, id: u64, model: &str| {
        let key = crate::media::LoadOptions::of(&s.develop_of(PhotoId(id)).unwrap()).profile_key.unwrap();
        let mut raw = raw_of(RawFormat::Arw, model, Default::default());
        local_colour_with(
            &mut raw,
            key,
            |_, _, _| None,
            |_| {
                let xy = Xy { x: 0.3127, y: 0.329 };
                (xy, lightcraft_raw::color::camera_transform(&raw_of(RawFormat::Arw, model, Default::default()), xy), Some(matching.clone()))
            },
        )
        .unwrap()
    };
    let stored = |s: &crate::Session, id: u64| {
        let d = s.develop_of(PhotoId(id)).unwrap();
        (d.profile.camera_colour, d.profile.camera_profile.clone(), d.light.exposure)
    };
    let chart = (CameraColour::Local, Some("A7 V Chart".to_string()), 0.6);
    // the other camera: the rest of the preset is applied, the id stays, and the picture is Camera Matching with the note
    assert_eq!(stored(&s, 2), chart);
    let (_, _, look, outcome) = render(&s, 2, "ILCE-7M4");
    assert_eq!(look.map(|l| l.matrix), Some(Mat3::IDENTITY));
    assert_eq!(outcome.note, Some(ColourNote { why: LocalUnavailable::OtherCamera, profile: "A7 V Chart".into() }));
    // the same camera with the file there: the profile
    assert_eq!(stored(&s, 3), chart);
    assert_eq!(render(&s, 3, "ILCE-7M5").3.note, None);
    // the file removed: Camera Matching with the note; nothing stored changes
    std::fs::remove_file(d.join("A7 V Chart.json")).unwrap();
    let (_, _, look, outcome) = render(&s, 3, "ILCE-7M5");
    assert_eq!(look.map(|l| l.matrix), Some(Mat3::IDENTITY));
    assert_eq!(outcome.note, Some(ColourNote { why: LocalUnavailable::Missing, profile: "A7 V Chart".into() }));
    assert_eq!(stored(&s, 3), chart, "the stored id stays");
    // the file back: the profile again
    camera_profiles::save_named(&profile("ILCE-7M5"), &d, "A7 V Chart").unwrap();
    let (_, _, look, outcome) = render(&s, 3, "ILCE-7M5");
    assert_eq!(outcome.note, None);
    assert_ne!(look.map(|l| l.matrix), Some(Mat3::IDENTITY), "the profile's own colour");
    let _ = std::fs::remove_dir_all(&d);
}
