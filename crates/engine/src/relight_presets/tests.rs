//! Light presets (fork, issue #301): placed round the photo's subject, one undo step, kept across a restart.

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::relight_presets::built_in;
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_geom::Rect;
use lightcraft_meta::{Region, RegionKind};
use lightcraft_relight::Field;
use serde_json::json;

use super::SubjectSource;
use crate::Session;

/// A depth for photo `id`: the presets place their lights by it, and a preset is only saved once it is found.
fn depth(s: &mut Session, id: PhotoId) {
    s.relight_test_field(id, Field::from_parts(4, 3, vec![0.0; 12], vec![0.0; 12]).unwrap());
}

fn session() -> (Session, Vec<PhotoId>) {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).take(2).collect();
    ids.iter().for_each(|id| depth(&mut s, *id));
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    (s, ids)
}

/// Photo `id` with a face region at `(x0, y0, x1, y1)`, set the way the face scan leaves it.
fn face(s: &mut Session, id: PhotoId, b: (f64, f64, f64, f64)) {
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.regions = vec![Region { rect: Rect { x0: b.0, y0: b.1, x1: b.2, y1: b.3 }, kind: RegionKind::Face, name: None, description: None }];
    s.apply_system(Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
}

fn key_light(s: &Session, id: PhotoId) -> (f64, f64) {
    let l = &s.develop_of(id).unwrap().relight.lights[0];
    (l.x, l.y)
}

#[test]
fn the_same_preset_lands_round_each_photos_own_subject() {
    let (mut s, ids) = session();
    face(&mut s, ids[0], (0.20, 0.20, 0.30, 0.35));
    face(&mut s, ids[1], (0.60, 0.40, 0.70, 0.55));
    s.execute("relight.applyPreset", &json!({"preset": "rembrandt"})).unwrap();
    let a = key_light(&s, ids[0]);
    s.execute("library.select", &json!({"ids": [ids[1].0]})).unwrap();
    let r = s.execute("relight.applyPreset", &json!({"preset": "Rembrandt"})).unwrap();
    let b = key_light(&s, ids[1]);
    assert_eq!(r["subject"]["source"], "face");
    // the faces are 0.4 across and 0.2 down from each other, the same size: so are the key lights
    assert!((b.0 - a.0 - 0.4).abs() < 1e-6 && (b.1 - a.1 - 0.2).abs() < 1e-6, "{a:?} {b:?}");
    // with no face and no mask the subject is the photo's centre
    let (mut s, ids) = session();
    let r = s.execute("relight.applyPreset", &json!({"preset": "rembrandt"})).unwrap();
    assert_eq!(r["subject"]["source"], "centre");
    let (x, y) = key_light(&s, ids[0]);
    assert!(x < 0.5 && y < 0.5, "a Rembrandt key is up and to the left of the centre: {x} {y}");
}

#[test]
fn a_face_beats_a_mask_and_a_mask_gives_a_face_in_its_upper_part() {
    let (mut s, ids) = session();
    face(&mut s, ids[0], (0.20, 0.20, 0.30, 0.35));
    let side = 32usize;
    let mut logits = vec![-5.0f32; side * side];
    for y in 8..30 {
        for x in 16..28 {
            logits[y * side + x] = 5.0;
        }
    }
    let mask = Mask {
        visible: true,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Subject { seg: Some(SegMask::from_logits(side, &logits)), detail: Vec::new(), edge: 0.0 },
        }],
        ..Mask::default()
    };
    let mut d = (*s.develop_of(ids[0]).unwrap()).clone();
    d.masks.push(mask);
    s.set_develop(ids[0], d, "Mask").unwrap();
    // the face's own box, whatever the mask selects
    let (subject, source) = s.relight_subject(ids[0]);
    assert_eq!(source, SubjectSource::Face);
    assert!((subject.x - 0.25).abs() < 1e-6 && (subject.w - 0.10).abs() < 1e-6, "{subject:?}");
    // without a face the mask's box (x 0.5 .. 0.875, y 0.25 .. 0.9375) gives a face at its top
    let mut meta = s.catalog.photo(ids[0]).unwrap().meta.clone();
    meta.regions.clear();
    s.apply_system(Op::SetMeta { id: ids[0], meta: Box::new(meta) }).unwrap();
    let (subject, source) = s.relight_subject(ids[0]);
    assert_eq!(source, SubjectSource::Mask);
    assert!((subject.x - 0.6875).abs() < 1e-6 && subject.y < 0.5 && subject.w < 0.2, "{subject:?}");
}

#[test]
fn applying_a_preset_is_one_undo_step_and_touches_only_the_lights() {
    let (mut s, ids) = session();
    let id = ids[0];
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    let before = (*s.develop_of(id).unwrap()).clone();
    s.execute("relight.applyPreset", &json!({"preset": "neon-two-tone"})).unwrap();
    let after = s.develop_of(id).unwrap();
    assert_eq!(after.relight.lights.len(), 2);
    assert_eq!((after.light.exposure, after.relight.is_on()), (before.light.exposure, true));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(*s.develop_of(id).unwrap(), before, "one undo puts everything back");
    // a preset that isn't one is an error and changes nothing
    assert!(s.execute("relight.applyPreset", &json!({"preset": "teapot"})).is_err());
    assert!(s.execute("relight.applyPreset", &json!({})).is_err());
    assert_eq!(*s.develop_of(id).unwrap(), before);
}

#[test]
fn a_saved_preset_survives_a_restart_and_fits_another_photo() {
    let dir = std::env::temp_dir().join(format!("lc-light-presets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&dir, true).unwrap();
    let ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).take(2).collect();
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    face(&mut s, ids[0], (0.30, 0.25, 0.40, 0.40));
    depth(&mut s, ids[0]);
    // nothing to save while no light is on, and a built-in name is taken
    assert!(s.execute("relight.savePreset", &json!({"name": "Mine"})).is_err());
    s.execute("relight.applyPreset", &json!({"preset": "split"})).unwrap();
    assert!(s.execute("relight.savePreset", &json!({"name": " rembrandt "})).is_err());
    let saved = s.execute("relight.savePreset", &json!({"name": "  My   Side Light "})).unwrap();
    assert_eq!(saved["name"], "My Side Light");
    assert!(s.execute("relight.savePreset", &json!({"name": "my side light"})).is_err(), "names are not repeated");
    let want = s.develop_of(ids[0]).unwrap().relight.clone();
    s.close_library().unwrap();
    drop(s);

    let mut s = Session::new().with_fs();
    s.open_library(&dir, false).unwrap();
    assert_eq!(s.relight_presets.len(), 1);
    let list = s.execute("relight.presets", &json!({})).unwrap();
    assert_eq!(list["presets"].as_array().unwrap().len(), built_in().len() + 1);
    // applied to the photo it was saved on, it gives the lights it was saved from
    let id = s.catalog.photos().map(|p| p.id).next().unwrap();
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("relight.applyPreset", &json!({"preset": saved["id"]})).unwrap();
    let got = s.develop_of(id).unwrap().relight.clone();
    assert_eq!(got.lights.len(), want.lights.len());
    assert!((got.lights[0].x - want.lights[0].x).abs() < 1e-6 && (got.lights[0].amount - want.lights[0].amount).abs() < 1e-9);
    // rename and delete reach it, and not the built-in ones
    s.execute("relight.renamePreset", &json!({"preset": saved["id"], "name": "Hard Side"})).unwrap();
    assert!(s.execute("relight.renamePreset", &json!({"preset": "candle", "name": "X"})).is_err());
    assert!(s.execute("relight.deletePreset", &json!({"preset": "candle"})).is_err());
    s.execute("relight.deletePreset", &json!({"preset": "Hard Side"})).unwrap();
    assert!(s.relight_presets.is_empty());
    s.close_library().unwrap();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&dir, false).unwrap();
    assert!(s.relight_presets.is_empty(), "a deleted preset stays deleted");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_list_of_saved_presets_reads_what_it_can() {
    use lightcraft_develop::relight_presets::LightPreset;
    let good = LightPreset { id: "user-1".into(), name: "Good".into(), ..LightPreset::default() };
    let list = vec![
        good.clone(),
        LightPreset { id: "user-1".into(), name: "Repeated Id".into(), ..LightPreset::default() },
        LightPreset { id: String::new(), name: "No Id".into(), ..LightPreset::default() },
        LightPreset { id: "user-2".into(), name: "  ".into(), ..LightPreset::default() },
        LightPreset { id: "rembrandt".into(), name: "Takes A Built-In Id".into(), ..LightPreset::default() },
    ];
    assert_eq!(super::read_user(list), vec![good]);
}

#[test]
fn a_preset_is_not_saved_until_the_photos_depth_is_found() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).take(2).collect();
    depth(&mut s, ids[0]);
    // a photo whose depth failed: the lights are on, and nothing is saved
    s.relight_test_failure(ids[1], "no depth model");
    s.execute("library.select", &json!({"ids": [ids[1].0]})).unwrap();
    s.execute("develop.set", &json!({"control": "relight.amount", "value": 60})).unwrap();
    let e = s.execute("relight.savePreset", &json!({"name": "Mine"})).unwrap_err().to_string();
    assert!(e.contains("depth") && e.contains("no depth model"), "{e}");
    assert!(s.relight_presets.is_empty());
    // a photo with its depth saves
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    s.execute("develop.set", &json!({"control": "relight.amount", "value": 60})).unwrap();
    s.execute("relight.savePreset", &json!({"name": "Mine"})).unwrap();
}

#[test]
fn a_new_ids_number_that_would_overflow_is_an_error_not_a_panic() {
    use lightcraft_develop::relight_presets::LightPreset;
    let (mut s, ids) = session();
    s.relight_presets.push(LightPreset { id: format!("user-{}", u64::MAX), name: "Hand Written".into(), ..LightPreset::default() });
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    s.execute("develop.set", &json!({"control": "relight.amount", "value": 60})).unwrap();
    assert!(s.execute("relight.savePreset", &json!({"name": "Mine"})).is_err());
}

#[test]
fn names_from_the_file_are_cut_and_a_built_in_or_repeated_name_is_left_out() {
    use lightcraft_develop::relight_presets::{LightPreset, MAX_NAME};
    let named = |id: &str, name: &str| LightPreset { id: id.into(), name: name.into(), ..LightPreset::default() };
    let list =
        super::read_user(vec![named("user-1", &"x".repeat(90)), named("user-2", " RemBrandt "), named("user-3", "Mine"), named("user-4", "mine")]);
    assert_eq!(list.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["user-1", "user-3"]);
    assert_eq!(list[0].name.chars().count(), MAX_NAME);
}

fn temp_library(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-light-presets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A presets.json with one light preset that isn't one beside a good one, a develop preset and a number among the
/// favourites: the good ones load, and nothing is set aside.
#[test]
fn one_bad_entry_of_presets_json_costs_only_itself() {
    let dir = temp_library("bad-entry");
    let mut s = Session::new().with_fs();
    s.open_library(&dir, false).unwrap();
    s.close_library().unwrap();
    drop(s);
    let good =
        serde_json::to_value(lightcraft_develop::relight_presets::LightPreset { id: "user-1".into(), name: "Good".into(), ..Default::default() })
            .unwrap();
    let file = json!({
        "user": [
            {"id": "mine-1", "name": "Mine", "group": "User", "settings": {}},
            {"id": "mine-2", "name": null, "group": "User", "settings": {}},
        ],
        "favorites": ["mine-1", 7],
        "relightPresets": [{"id": "user-9", "name": null, "lights": 3}, good],
    });
    std::fs::write(dir.join("presets.json"), file.to_string()).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&dir, false).unwrap();
    assert!(s.presets.iter().any(|p| p.id == "mine-1" && !p.builtin), "the develop preset loads");
    assert!(s.presets.iter().all(|p| p.id != "mine-2"));
    assert_eq!(s.relight_presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Good"]);
    let aside = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().contains("presets.json.corrupt"));
    assert!(!aside, "nothing is set aside");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Presets saved in one library are not the next one's: opening another library in the same session clears them.
#[test]
fn another_library_does_not_show_the_last_ones_presets() {
    let (a, b) = (temp_library("a"), temp_library("b"));
    let mut s = Session::new().with_fs();
    s.open_library(&a, true).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    depth(&mut s, id);
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.set", &json!({"control": "relight.amount", "value": 60})).unwrap();
    s.execute("relight.savePreset", &json!({"name": "From A"})).unwrap();
    s.presets.push(lightcraft_develop::Preset {
        id: "a-1".into(),
        name: "A's".into(),
        group: "User".into(),
        settings: json!({}),
        favorite: false,
        builtin: false,
    });
    s.open_library(&b, false).unwrap();
    let list = s.execute("relight.presets", &json!({})).unwrap();
    assert!(list["presets"].as_array().unwrap().iter().all(|p| p["builtin"] == true), "{list}");
    assert!(s.relight_presets.is_empty() && s.presets.iter().all(|p| p.builtin));
    // and a save in B writes nothing of A's into B's file
    s.close_library().unwrap();
    drop(s);
    let text = std::fs::read_to_string(b.join("presets.json")).unwrap_or_default();
    assert!(!text.contains("From A") && !text.contains("A's"), "{text}");
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}
