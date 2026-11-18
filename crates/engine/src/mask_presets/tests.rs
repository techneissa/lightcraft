//! Mask presets (fork, issue #329): a mask kept as a recipe, found again on each photo, one undo step, kept in the library.

use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_geom::Point;
use serde_json::json;

use crate::Session;

/// Each photo's own subject: one cell of a 4 × 4 grid, a different one per photo.
fn subject_of(id: PhotoId) -> SegMask {
    let cell = (id.0 % 16) as usize;
    let logits: Vec<f32> = (0..16).map(|i| if i == cell { 5.0 } else { -5.0 }).collect();
    SegMask::from_logits(4, &logits)
}

/// A session with the demo photos (the first active) and a finder that gives each photo its own subject and finds no
/// description.
fn session() -> (Session, Vec<PhotoId>) {
    let mut s = Session::with_demo();
    let mut ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).collect();
    ids.sort();
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    s.adaptive.set_finder(Some(Arc::new(|id, shape| match shape {
        MaskShape::Subject { .. } | MaskShape::Background { .. } => Ok(Some(subject_of(id))),
        _ => Ok(None),
    })));
    (s, ids)
}

fn select(s: &mut Session, id: PhotoId) {
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
}

fn part(op: MaskOp, shape: MaskShape) -> MaskComponent {
    MaskComponent { name: None, op, invert: false, shape }
}

/// Give photo `id` the mask and select it; the mask's id.
fn give(s: &mut Session, id: PhotoId, mask: Mask) -> u32 {
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.masks = vec![Mask { id: 1, ..mask }];
    s.set_develop(id, d, "Mask").unwrap();
    s.active_mask = Some(1);
    1
}

fn radial(center: Point) -> MaskShape {
    MaskShape::Radial { center, rx: 0.25, ry: 0.12, angle: 30.0, feather: 60.0, invert: false }
}

/// An AI subject mask saved as a preset and applied to another photo gets that photo's own subject, not the first's.
#[test]
fn a_subject_mask_preset_finds_the_other_photos_subject() {
    let (mut s, ids) = session();
    let (a, b) = (ids[0], ids[1]);
    let mut m = Mask {
        name: "Subject Mask 1".into(),
        components: vec![part(MaskOp::Add, MaskShape::Subject { seg: Some(subject_of(a)), detail: vec![], edge: 10.0 })],
        ..Default::default()
    };
    m.adjust.exposure = 0.7;
    m.refine = 30.0;
    give(&mut s, a, m);
    let saved = s.execute("mask.savePreset", &json!({"name": "  Brighter   Subject "})).unwrap();
    assert_eq!((saved["name"].as_str(), saved["ai"].as_u64()), (Some("Brighter Subject"), Some(1)));
    assert!(s.mask_presets[0].mask.components[0].shape.seg().is_none(), "a recipe, never pixels");
    select(&mut s, b);
    let r = s.execute("mask.applyPreset", &json!({"preset": "brighter subject"})).unwrap();
    assert_eq!(r["applied"], true, "{r}");
    assert!(r["missing"].as_array().unwrap().is_empty() && r["foundNothing"].as_array().unwrap().is_empty(), "{r}");
    let d = s.develop_of(b).unwrap();
    assert_eq!(d.masks.len(), 1);
    let got = &d.masks[0];
    assert_eq!(got.components[0].shape.seg(), Some(&subject_of(b)), "B's own subject");
    assert_ne!(got.components[0].shape.seg(), Some(&subject_of(a)));
    assert!(got.renders(), "found, so it draws");
    assert_eq!((got.adjust.exposure, got.refine), (0.7, 30.0), "the effect sliders came along");
    assert_eq!(s.active_mask, Some(got.id), "and the new mask is selected");
}

/// A radial gradient is kept relative to the photo: on photos of another size and turned another way it sits at the same
/// relative place, the same share of the long edge across.
#[test]
fn a_radial_preset_lands_at_the_same_relative_place_whatever_the_photo() {
    let (mut s, ids) = session();
    let a = ids[0];
    let dims = |s: &Session, id: PhotoId| {
        let p = s.catalog.photo(id).unwrap();
        (p.width, p.height)
    };
    let other = *ids.iter().find(|i| dims(&s, **i) != dims(&s, a)).expect("a demo photo of another size");
    let m = Mask { name: "Vignette".into(), components: vec![part(MaskOp::Add, radial(Point::new(0.3, 0.65)))], ..Default::default() };
    give(&mut s, a, m);
    s.execute("mask.savePreset", &json!({"name": "Low Left Pool"})).unwrap();
    for turn in [false, true] {
        select(&mut s, other);
        if turn {
            s.execute("photo.rotateRight", &json!({})).unwrap();
        }
        s.execute("mask.applyPreset", &json!({"preset": "Low Left Pool"})).unwrap();
        let d = s.develop_of(other).unwrap();
        let got = d.masks.last().unwrap();
        assert!(got.on_photo);
        assert_eq!(got.components[0].shape, radial(Point::new(0.3, 0.65)), "turned: {turn}");
    }
}

/// A part a model can't find here (no finder, no model) or finds nothing for is named, the mask is still added, and
/// nothing else is touched.
#[test]
fn a_missing_model_or_a_search_that_finds_nothing_is_said_and_breaks_nothing() {
    let (mut s, ids) = session();
    let a = ids[0];
    let m = Mask {
        name: "Dog".into(),
        components: vec![part(MaskOp::Add, MaskShape::prompt("dog")), part(MaskOp::Subtract, radial(Point::new(0.5, 0.5)))],
        ..Default::default()
    };
    give(&mut s, a, m);
    // (a description is a part the finder finds nothing for)
    s.mask_presets.push(lightcraft_develop::mask_presets::MaskPreset::capture("user-1", "Dog Only", &s.develop_of(a).unwrap().masks[0]));
    select(&mut s, ids[1]);
    let r = s.execute("mask.applyPreset", &json!({"preset": "user-1"})).unwrap();
    assert_eq!(r["foundNothing"], json!(["prompt"]), "{r}");
    let d = s.develop_of(ids[1]).unwrap();
    assert_eq!(d.masks.len(), 1, "the mask is there, selecting nothing for the part");
    assert!(!d.masks[0].is_waiting(), "nothing waits for ever");
    // without any stand-in, the missing model is named up front
    s.adaptive.set_finder(None);
    select(&mut s, ids[2]);
    let r = s.execute("mask.applyPreset", &json!({"preset": "user-1"})).unwrap();
    assert!(!r["missing"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(s.develop_of(ids[2]).unwrap().masks.len(), 1);
    // and a preset that isn't one is an error that changes nothing
    assert!(s.execute("mask.applyPreset", &json!({"preset": "teapot"})).is_err());
    assert!(s.execute("mask.applyPreset", &json!({})).is_err());
    assert_eq!(s.develop_of(ids[2]).unwrap().masks.len(), 1);
}

/// Applying a preset is one undo step, also when the photo's own parts are found in the same command.
#[test]
fn applying_a_preset_is_one_undo_step() {
    let (mut s, ids) = session();
    let (a, b) = (ids[0], ids[1]);
    let m = Mask {
        name: "Subject Mask 1".into(),
        components: vec![part(MaskOp::Add, MaskShape::subject()), part(MaskOp::Intersect, radial(Point::new(0.5, 0.5)))],
        ..Default::default()
    };
    give(&mut s, a, m);
    s.execute("mask.savePreset", &json!({"name": "Centre Subject"})).unwrap();
    select(&mut s, b);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    let before = (*s.develop_of(b).unwrap()).clone();
    s.execute("mask.applyPreset", &json!({"preset": "Centre Subject"})).unwrap();
    assert_eq!(s.develop_of(b).unwrap().masks.len(), 1);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(*s.develop_of(b).unwrap(), before, "one undo puts everything back, the found subject included");
}

#[test]
fn names_are_cleaned_not_repeated_and_the_list_is_managed_by_command() {
    let (mut s, ids) = session();
    let m = Mask { name: "Pool".into(), components: vec![part(MaskOp::Add, radial(Point::new(0.5, 0.5)))], ..Default::default() };
    give(&mut s, ids[0], m);
    let saved = s.execute("mask.savePreset", &json!({"name": "Pool"})).unwrap();
    assert!(s.execute("mask.savePreset", &json!({"name": " pool "})).is_err(), "names are not repeated");
    assert!(s.execute("mask.savePreset", &json!({"name": "  "})).is_err());
    assert!(s.execute("mask.savePreset", &json!({"name": "x".repeat(41)})).is_err());
    // applying twice names the second mask apart
    s.execute("mask.applyPreset", &json!({"preset": saved["id"]})).unwrap();
    s.execute("mask.applyPreset", &json!({"preset": saved["id"]})).unwrap();
    let names: Vec<String> = s.develop_of(ids[0]).unwrap().masks.iter().map(|m| m.name.clone()).collect();
    assert_eq!(names, ["Pool", "Pool 2", "Pool 3"]);
    let list = s.execute("mask.presets", &json!({})).unwrap();
    assert_eq!(list["presets"][0]["kinds"], json!(["radial"]));
    s.execute("mask.renamePreset", &json!({"preset": saved["id"], "name": "Hard Pool"})).unwrap();
    assert_eq!(s.mask_presets[0].name, "Hard Pool");
    s.execute("mask.deletePreset", &json!({"preset": "hard pool"})).unwrap();
    assert!(s.mask_presets.is_empty());
    // a mask that selects nothing yet is not saved
    let empty = Mask { name: "Empty".into(), components: vec![part(MaskOp::Add, MaskShape::Brush { strokes: vec![] })], ..Default::default() };
    give(&mut s, ids[0], empty);
    assert!(s.execute("mask.savePreset", &json!({"name": "Nothing"})).is_err());
}

fn temp_library(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-mask-presets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A presets.json with one mask preset that isn't one beside a good one, a develop preset and a number among the
/// favourites: the good ones load, and a copy of the file is kept before the next save writes it without the bad one.
#[test]
fn one_bad_entry_of_presets_json_costs_only_itself() {
    let dir = temp_library("bad-entry");
    let mut s = Session::new().with_fs();
    s.open_library(&dir, false).unwrap();
    s.close_library().unwrap();
    drop(s);
    let m = Mask { name: "Good".into(), components: vec![part(MaskOp::Add, radial(Point::new(0.4, 0.4)))], ..Default::default() };
    let good = serde_json::to_value(lightcraft_develop::mask_presets::MaskPreset::capture("user-1", "Good", &m)).unwrap();
    let file = json!({
        "user": [{"id": "mine-1", "name": "Mine", "group": "User", "settings": {}}],
        "favorites": ["mine-1", 7],
        "maskPresets": [{"id": "user-9", "name": null, "mask": 3}, {"id": "user-8", "name": "No Parts", "mask": {}}, good],
    });
    std::fs::write(dir.join("presets.json"), file.to_string()).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&dir, false).unwrap();
    assert!(s.presets.iter().any(|p| p.id == "mine-1" && !p.builtin), "the develop preset loads");
    assert_eq!(s.mask_presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Good"]);
    let aside = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().contains("presets.json.corrupt"));
    assert!(aside, "a copy of the file is kept");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A saved preset survives a restart, and another library does not show it.
#[test]
fn presets_stay_in_their_library() {
    let (a, b) = (temp_library("a"), temp_library("b"));
    let mut s = Session::new().with_fs();
    s.open_library(&a, true).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    select(&mut s, id);
    let m = Mask { name: "Pool".into(), components: vec![part(MaskOp::Add, radial(Point::new(0.5, 0.5)))], ..Default::default() };
    give(&mut s, id, m);
    s.execute("mask.savePreset", &json!({"name": "From A"})).unwrap();
    s.close_library().unwrap();
    // (the same library again: it is there)
    s.open_library(&a, false).unwrap();
    assert_eq!(s.mask_presets.len(), 1, "kept across a restart");
    // another library in the same session: none of A's
    s.open_library(&b, false).unwrap();
    let list = s.execute("mask.presets", &json!({})).unwrap();
    assert!(list["presets"].as_array().unwrap().is_empty(), "{list}");
    s.close_library().unwrap();
    drop(s);
    let text = std::fs::read_to_string(b.join("presets.json")).unwrap_or_default();
    assert!(!text.contains("From A"), "{text}");
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn names_from_the_file_are_cut_and_a_repeated_one_is_left_out() {
    use lightcraft_develop::mask_presets::{MAX_NAME, MaskPreset};
    let m = Mask { components: vec![part(MaskOp::Add, radial(Point::new(0.5, 0.5)))], ..Default::default() };
    let named = |id: &str, name: &str| MaskPreset { id: id.into(), name: name.into(), mask: m.clone() };
    let (list, changed) =
        super::read_user(vec![named("user-1", &"x".repeat(90)), named("user-2", "Mine"), named("user-3", "mine"), named("", "No Id")]);
    assert_eq!(list.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["user-1", "user-2"]);
    assert_eq!(changed, 3);
    assert_eq!(list[0].name.chars().count(), MAX_NAME);
}
