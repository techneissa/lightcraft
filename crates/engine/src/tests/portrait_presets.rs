//! Portrait presets (fork, `fork/portrait-presets.md`, issue #106): saved from one face, applied to the same person's
//! face only, on chosen photos or on every photo of the person, through export and import.

use std::sync::{Arc, Mutex};

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::retouch::{FEATURES, Face, KeyFrame, NormRect, Retouch};
use lightcraft_geom::Rect;
use lightcraft_meta::{Region, RegionKind};
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

use crate::Session;
use crate::portrait_presets::KEY;
use crate::retouch_input::FaceProvider;

/// Gives the faces it holds, whatever the picture.
struct Fake(Mutex<Vec<Face>>);

impl FaceProvider for Fake {
    fn name(&self) -> &str {
        "Fake"
    }
    fn faces(&self, _img: &Rgba8) -> Result<Vec<Face>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

const LEFT: (f64, f64, f64, f64) = (0.1, 0.2, 0.3, 0.5);
const RIGHT: (f64, f64, f64, f64) = (0.6, 0.2, 0.8, 0.5);

fn face(b: (f64, f64, f64, f64), person: Option<&str>) -> Face {
    Face { person: person.map(str::to_string), ..Face::boxed(NormRect::new(b.0, b.1, b.2, b.3)) }
}

/// A face as the people parse gives it: its box before lens corrections and transform.
fn parsed(b: (f64, f64, f64, f64)) -> Face {
    Face { key_frame: KeyFrame::Photo, ..face(b, None) }
}

fn rect(b: (f64, f64, f64, f64)) -> Rect {
    Rect { x0: b.0, y0: b.1, x1: b.2, y1: b.3 }
}

fn region(b: (f64, f64, f64, f64), name: Option<&str>) -> Region {
    Region { rect: rect(b), kind: RegionKind::Face, name: name.map(str::to_string), description: None }
}

/// Photo `id`'s face regions, set without an undo step (what the face scan and People leave in the catalog).
fn regions(s: &mut Session, id: PhotoId, regions: Vec<Region>) {
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.regions = regions;
    s.apply_system(Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
}

/// A demo session whose portrait faces come from `faces`, with the first photo active.
fn demo_with(faces: Vec<Face>) -> (Session, Vec<PhotoId>) {
    let mut s = Session::with_demo();
    s.retouch_faces.set_provider(Arc::new(Fake(Mutex::new(faces))));
    let vis = s.visible_cloned();
    s.execute("library.select", &json!({"ids": [vis[0].0]})).unwrap();
    (s, vis)
}

fn retouch(s: &Session, id: PhotoId) -> Retouch {
    s.develop_of(id).unwrap().retouch.clone()
}

const SMOOTH: &str = "retouch.skin.smoothness";

/// Photo 0: Ann on the left, Bob on the right, named; Ann's face smoothed to 70 (every face's stays 0), and a
/// portrait preset saved from her face.
fn ann_preset() -> (Session, Vec<PhotoId>, String) {
    let (mut s, vis) = demo_with(vec![face(LEFT, Some("ann")), face(RIGHT, Some("bob"))]);
    regions(&mut s, vis[0], vec![region(LEFT, Some("Ann")), region(RIGHT, Some("Bob"))]);
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": SMOOTH, "value": 70, "face": 0})).unwrap();
    let r = s.execute("retouch.savePreset", &json!({"name": "Ann Look", "face": 0})).unwrap();
    assert_eq!((r["person"].clone(), r["group"].clone()), (json!("ann"), json!("Portrait")), "{r}");
    (s, vis, r["id"].as_str().unwrap().to_string())
}

/// The settings each of `faces` renders with on photo `id`.
fn smoothness_of(s: &Session, id: PhotoId, faces: &[Face]) -> Vec<f64> {
    let r = retouch(s, id);
    (0..faces.len()).map(|i| r.settings_for(faces, i).get(SMOOTH).unwrap()).collect()
}

#[test]
fn a_preset_from_one_face_changes_only_that_persons_face_on_a_group_photo() {
    let (mut s, vis, pid) = ann_preset();
    // a group photo with the two of them the other way round
    regions(&mut s, vis[1], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("Ann"))]);
    let r = s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0]})).unwrap();
    assert_eq!(r["portrait"]["applied"], json!([vis[1].0]), "{r}");
    let got = retouch(&s, vis[1]);
    assert_eq!(got.global.get(SMOOTH), Some(0.0), "every face's settings are left alone");
    assert_eq!(got.faces.len(), 1);
    assert_eq!(got.faces[0].face.person.as_deref(), Some("ann"));
    // as the photo's faces come back from the parse: Ann's face gets 70, Bob's keeps 0
    let found = [face(LEFT, Some("bob")), face(RIGHT, Some("ann"))];
    assert_eq!(smoothness_of(&s, vis[1], &found), [0.0, 70.0]);
    // a photo with two faces, neither of them Ann: nothing changes there
    regions(&mut s, vis[2], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("Cy"))]);
    let before = retouch(&s, vis[2]);
    let r = s.execute("preset.apply", &json!({"id": pid, "ids": [vis[2].0]})).unwrap();
    assert_eq!(r["portrait"]["skipped"][0]["id"], vis[2].0, "{r}");
    assert_eq!(retouch(&s, vis[2]), before);
}

#[test]
fn every_photo_of_a_person_gets_it_and_the_others_are_left_alone_one_undo_step_each() {
    let (mut s, vis, pid) = ann_preset();
    regions(&mut s, vis[1], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("ann "))]);
    regions(&mut s, vis[2], vec![region(RIGHT, Some("Ann"))]);
    // Bob alone, someone unnamed alone (not recognised as Ann: a lone face counts only for preset.apply), nobody
    regions(&mut s, vis[3], vec![region(LEFT, Some("Bob"))]);
    regions(&mut s, vis[4], vec![region(LEFT, None)]);
    regions(&mut s, vis[5], vec![]);
    let untouched: Vec<(PhotoId, Retouch)> = vis[3..].iter().map(|id| (*id, retouch(&s, *id))).collect();
    let undo_before = s.undo.len();
    let r = s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid})).unwrap();
    let mut applied: Vec<u64> = r["applied"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    applied.sort();
    let mut want = [vis[0].0, vis[1].0, vis[2].0];
    want.sort();
    assert_eq!(applied, want, "{r}");
    for (id, was) in &untouched {
        assert_eq!(retouch(&s, *id), *was, "photo {} has no Ann", id.0);
    }
    assert_eq!(smoothness_of(&s, vis[1], &[face(LEFT, Some("bob")), face(RIGHT, Some("ann"))]), [0.0, 70.0]);
    assert_eq!(smoothness_of(&s, vis[2], &[face(RIGHT, Some("ann"))]), [70.0]);
    // photo 0 had it already: no step for it; one for each of the two others
    assert_eq!(s.undo.len() - undo_before, 2);
    s.execute("edit.undo", &json!({})).unwrap();
    let left: Vec<bool> = [vis[1], vis[2]].iter().map(|id| retouch(&s, *id).faces.is_empty()).collect();
    assert_eq!(left.iter().filter(|undone| **undone).count(), 1, "one undo takes back one photo: {left:?}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(retouch(&s, vis[1]).faces.is_empty() && retouch(&s, vis[2]).faces.is_empty());
}

#[test]
fn from_the_face_picker_a_faces_own_settings_go_to_every_other_photo_of_them() {
    let (mut s, vis, _) = ann_preset();
    regions(&mut s, vis[1], vec![region(RIGHT, Some("Ann"))]);
    regions(&mut s, vis[2], vec![region(RIGHT, Some("Bob"))]);
    let r = s.execute("retouch.applyToPerson", &json!({"face": 0})).unwrap();
    assert_eq!((r["person"].clone(), r["applied"].clone()), (json!("ann"), json!([vis[1].0])), "{r}");
    assert_eq!(smoothness_of(&s, vis[1], &[face(RIGHT, Some("ann"))]), [70.0]);
    // an unknown preset, a preset that is not a portrait one, no person: errors
    assert!(s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": "nope"})).is_err());
    assert!(s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": "lc.bright-airy"})).is_err());
    assert!(s.execute("retouch.applyToPerson", &json!({})).is_err());
    assert!(s.execute("retouch.applyToPerson", &json!({"name": "Ann"})).is_err(), "by name it needs a preset");
}

#[test]
fn an_unnamed_face_is_matched_by_how_alike_the_faces_are() {
    let (mut s, vis) = demo_with(vec![face(LEFT, None)]);
    // the face index, as the background recognition leaves it (4-number embeddings of a stand-in model)
    s.faces.index.reset("stand-in@1", 4, None);
    let unit = |v: [f32; 4]| {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.map(|x| x / n).to_vec()
    };
    regions(&mut s, vis[0], vec![region(LEFT, None)]);
    s.faces.index.insert(vis[0].0, &rect(LEFT), unit([1.0, 0.0, 0.0, 0.0]));
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": SMOOTH, "value": 60, "face": 0})).unwrap();
    let r = s.execute("retouch.savePreset", &json!({"name": "Look", "face": 0})).unwrap();
    assert_eq!((r["person"].clone(), r["byFace"].clone()), (Value::Null, json!(true)), "{r}");
    let pid = r["id"].as_str().unwrap().to_string();
    // two unnamed faces: the right one is the same person
    regions(&mut s, vis[1], vec![region(LEFT, None), region(RIGHT, None)]);
    s.faces.index.insert(vis[1].0, &rect(LEFT), unit([0.0, 1.0, 0.0, 0.0]));
    s.faces.index.insert(vis[1].0, &rect(RIGHT), unit([0.95, 0.3, 0.0, 0.0]));
    // two unnamed faces, neither alike
    regions(&mut s, vis[2], vec![region(LEFT, None), region(RIGHT, None)]);
    s.faces.index.insert(vis[2].0, &rect(LEFT), unit([0.0, 1.0, 0.0, 0.0]));
    s.faces.index.insert(vis[2].0, &rect(RIGHT), unit([0.0, 0.0, 1.0, 0.0]));
    // one face, nothing known about it: a photo's only face gets it
    regions(&mut s, vis[3], vec![region(RIGHT, None)]);
    let r = s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0, vis[2].0, vis[3].0]})).unwrap();
    assert_eq!(r["portrait"]["applied"], json!([vis[1].0, vis[3].0]), "{r}");
    assert_eq!(r["portrait"]["skipped"][0]["id"], vis[2].0, "{r}");
    assert_eq!(smoothness_of(&s, vis[1], &[parsed(LEFT), parsed(RIGHT)]), [0.0, 60.0]);
    assert!(retouch(&s, vis[2]).faces.is_empty());
    assert_eq!(smoothness_of(&s, vis[3], &[parsed(RIGHT)]), [60.0]);
}

#[test]
fn a_preset_round_trips_through_export_and_import() {
    let (mut s, _, pid) = ann_preset();
    let dir = std::env::temp_dir().join(format!("lc-portrait-presets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Ann Look.lcpreset");
    s.execute("preset.export", &json!({"path": file.to_string_lossy(), "ids": [pid]})).unwrap();
    let saved = s.presets.iter().find(|p| p.id == pid).unwrap().clone();

    let (mut t, vis) = demo_with(vec![]);
    let r = t.execute("preset.import", &json!({"paths": [file.to_string_lossy()]})).unwrap();
    let id = r["imported"][0]["id"].as_str().unwrap().to_string();
    let back = t.presets.iter().find(|p| p.id == id).unwrap().clone();
    assert_eq!((back.name.as_str(), back.group.as_str(), &back.settings), (saved.name.as_str(), saved.group.as_str(), &saved.settings));
    regions(&mut t, vis[4], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("Ann"))]);
    t.execute("preset.apply", &json!({"id": id, "ids": [vis[4].0]})).unwrap();
    assert_eq!(smoothness_of(&t, vis[4], &[face(LEFT, Some("bob")), face(RIGHT, Some("ann"))]), [0.0, 70.0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_preset_carries_every_feature_and_other_settings_only_when_asked_never_spots() {
    let (mut s, vis, pid) = ann_preset();
    let p = s.presets.iter().find(|p| p.id == pid).unwrap().clone();
    let keys: Vec<&String> = p.settings.as_object().unwrap().keys().collect();
    assert_eq!(keys, [KEY], "only the face part without `others`");
    let features = p.settings[KEY]["features"].as_object().unwrap();
    assert_eq!(features.len(), FEATURES.len(), "every feature is carried: {features:?}");
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
    let with = s.execute("retouch.savePreset", &json!({"name": "Ann and Light", "face": 0, "others": true})).unwrap();
    let w = s.presets.iter().find(|p| p.id == with["id"]).unwrap().clone();
    assert_eq!(w.settings["light"]["exposure"], 0.7);
    let spots = s.execute("retouch.savePreset", &json!({"name": "Asked", "face": 0, "groups": ["spots", "redEye", "portrait", "light"]})).unwrap();
    let sp = s.presets.iter().find(|p| p.id == spots["id"]).unwrap().clone();
    for k in ["spots", "red_eye", "fork_retouch"] {
        assert!(sp.settings.get(k).is_none(), "{k} is never carried: {}", sp.settings);
    }
    // the other settings apply as any preset's, and the photo's own spots stay
    s.execute("preset.apply", &json!({"id": w.id, "ids": [vis[1].0]})).unwrap();
    assert_eq!(s.develop_of(vis[1]).unwrap().light.exposure, 0.7);
    // a malformed face part is no panic: it applies nothing to faces
    let mut bad = w.clone();
    bad.id = "user.bad".into();
    bad.settings[KEY] = json!({"features": {"skin": "x"}, "person": 7, "embedding": {"model": 1}});
    s.presets.push(bad);
    s.execute("preset.apply", &json!({"id": "user.bad", "ids": [vis[2].0]})).unwrap();
}

#[test]
fn the_hover_preview_shows_what_a_click_commits() {
    let (mut s, vis, pid) = ann_preset();
    regions(&mut s, vis[1], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("Ann"))]);
    let preset = s.presets.iter().find(|p| p.id == pid).unwrap().clone();
    let preview = s.preset_applied(vis[1], &preset, 1.0).unwrap();
    s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0]})).unwrap();
    assert_eq!(preview.retouch, retouch(&s, vis[1]));
}

#[test]
fn every_photo_of_a_person_includes_the_unnamed_faces_recognised_as_them() {
    let (mut s, vis, pid) = ann_preset();
    s.faces.index.reset("stand-in@1", 2, None);
    // Ann's named face on photo 0 is what she is known by
    s.faces.index.insert(vis[0].0, &rect(LEFT), vec![1.0, 0.0]);
    s.faces.index.insert(vis[0].0, &rect(RIGHT), vec![0.0, 1.0]);
    // photo 1: an unnamed face like hers, next to someone else; photo 2: two unnamed faces like neither
    regions(&mut s, vis[1], vec![region(LEFT, None), region(RIGHT, Some("Bob"))]);
    s.faces.index.insert(vis[1].0, &rect(LEFT), vec![0.98, 0.2]);
    s.faces.index.insert(vis[1].0, &rect(RIGHT), vec![0.0, 1.0]);
    regions(&mut s, vis[2], vec![region(LEFT, None), region(RIGHT, None)]);
    s.faces.index.insert(vis[2].0, &rect(LEFT), vec![0.0, 1.0]);
    s.faces.index.insert(vis[2].0, &rect(RIGHT), vec![-1.0, 0.0]);
    let r = s.execute("retouch.applyToPerson", &json!({"name": "ann", "preset": pid})).unwrap();
    let applied: Vec<u64> = r["applied"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert!(applied.contains(&vis[1].0) && !applied.contains(&vis[2].0), "{r}");
    assert_eq!(smoothness_of(&s, vis[1], &[parsed(LEFT), face(RIGHT, Some("bob"))]), [70.0, 0.0]);
    assert!(retouch(&s, vis[2]).faces.is_empty());
}
