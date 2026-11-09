//! Portrait presets (fork, `fork/portrait-presets.md`, issue #106): saved from one face, applied to the same person's
//! face only, on chosen photos or on every photo of the person, through export and import.

use std::sync::{Arc, Mutex};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
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
fn every_photo_of_a_person_gets_it_as_one_undo_step_and_the_others_are_left_alone() {
    let (mut s, vis, pid) = ann_preset();
    regions(&mut s, vis[1], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("ann "))]);
    regions(&mut s, vis[2], vec![region(RIGHT, Some("Ann"))]);
    // Bob alone, someone unnamed alone (a lone face never counts for Apply to Every Photo), nobody
    regions(&mut s, vis[3], vec![region(LEFT, Some("Bob"))]);
    regions(&mut s, vis[4], vec![region(LEFT, None)]);
    regions(&mut s, vis[5], vec![]);
    let untouched: Vec<(PhotoId, Retouch)> = vis[3..].iter().map(|id| (*id, retouch(&s, *id))).collect();
    let undo_before = s.undo.len();
    let r = s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid})).unwrap();
    assert_eq!(r["state"], "done", "{r}");
    let mut applied: Vec<u64> = r["applied"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    applied.sort();
    let mut want = [vis[0].0, vis[1].0, vis[2].0];
    want.sort();
    assert_eq!(applied, want, "{r}");
    assert_eq!(r["changed"], 2, "photo 0 had them already: {r}");
    for (id, was) in &untouched {
        assert_eq!(retouch(&s, *id), *was, "photo {} has no Ann", id.0);
    }
    assert_eq!(smoothness_of(&s, vis[1], &[face(LEFT, Some("bob")), face(RIGHT, Some("ann"))]), [0.0, 70.0]);
    assert_eq!(smoothness_of(&s, vis[2], &[face(RIGHT, Some("ann"))]), [70.0]);
    assert_eq!(s.undo.len() - undo_before, 1, "one undo step for every photo");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(retouch(&s, vis[1]).faces.is_empty() && retouch(&s, vis[2]).faces.is_empty(), "one undo takes all of it back");
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

/// Unit vectors of a stand-in recognition model.
fn unit(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

#[test]
fn an_unnamed_face_is_matched_by_how_alike_the_faces_are_and_a_stranger_is_not() {
    let (mut s, vis) = demo_with(vec![face(LEFT, None)]);
    // the face index, as the background recognition leaves it
    s.faces.index.reset("stand-in@1", 4, None);
    regions(&mut s, vis[0], vec![region(LEFT, None)]);
    s.faces.index.insert(vis[0].0, &rect(LEFT), unit(&[1.0, 0.0, 0.0, 0.0]));
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": SMOOTH, "value": 60, "face": 0})).unwrap();
    let r = s.execute("retouch.savePreset", &json!({"name": "Look", "face": 0})).unwrap();
    assert_eq!((r["person"].clone(), r["byFace"].clone(), r["anyFace"].clone()), (Value::Null, json!(true), json!(false)), "{r}");
    let pid = r["id"].as_str().unwrap().to_string();
    // two unnamed faces: the right one is the same person
    regions(&mut s, vis[1], vec![region(LEFT, None), region(RIGHT, None)]);
    s.faces.index.insert(vis[1].0, &rect(LEFT), unit(&[0.0, 1.0, 0.0, 0.0]));
    s.faces.index.insert(vis[1].0, &rect(RIGHT), unit(&[0.95, 0.3, 0.0, 0.0]));
    // two unnamed faces, neither alike
    regions(&mut s, vis[2], vec![region(LEFT, None), region(RIGHT, None)]);
    s.faces.index.insert(vis[2].0, &rect(LEFT), unit(&[0.0, 1.0, 0.0, 0.0]));
    s.faces.index.insert(vis[2].0, &rect(RIGHT), unit(&[0.0, 0.0, 1.0, 0.0]));
    // one face nothing is known about: it can't be compared, so the photo's only face gets it
    regions(&mut s, vis[3], vec![region(RIGHT, None)]);
    // one face of a stranger, scoring 0.1: compared and turned down, even though it is the only one
    regions(&mut s, vis[4], vec![region(RIGHT, None)]);
    s.faces.index.insert(vis[4].0, &rect(RIGHT), vec![0.1, 0.995, 0.0, 0.0]);
    let ids = [vis[1].0, vis[2].0, vis[3].0, vis[4].0];
    let r = s.execute("preset.apply", &json!({"id": pid, "ids": ids})).unwrap();
    let mut applied: Vec<u64> = r["portrait"]["applied"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    applied.sort();
    let mut want = [vis[1].0, vis[3].0];
    want.sort();
    assert_eq!(applied, want, "{r}");
    assert_eq!(smoothness_of(&s, vis[1], &[parsed(LEFT), parsed(RIGHT)]), [0.0, 60.0]);
    assert!(retouch(&s, vis[2]).faces.is_empty());
    assert_eq!(smoothness_of(&s, vis[3], &[parsed(RIGHT)]), [60.0]);
    assert!(retouch(&s, vis[4]).faces.is_empty(), "the stranger keeps their face");
}

/// A face 0.50 like Ann and 0.90 like Bob is Bob's: it is compared with every named person, and Ann must lead by
/// the margin name suggestions use.
#[test]
fn a_face_more_like_someone_else_is_never_taken_for_the_person() {
    let (mut s, vis, pid) = ann_preset();
    s.faces.index.reset("stand-in@1", 3, None);
    let (ann, bob) = ([1.0, 0.0, 0.0], [0.5, 0.866, 0.0]);
    s.faces.index.insert(vis[0].0, &rect(LEFT), ann.to_vec());
    s.faces.index.insert(vis[0].0, &rect(RIGHT), unit(&bob));
    // q·ann = 0.50, q·bob = 0.90
    let q = vec![0.5, 0.7506, 0.4320];
    regions(&mut s, vis[1], vec![region(LEFT, None)]);
    s.faces.index.insert(vis[1].0, &rect(LEFT), q.clone());
    // and a face clearly Ann's
    regions(&mut s, vis[2], vec![region(LEFT, None), region(RIGHT, Some("Bob"))]);
    s.faces.index.insert(vis[2].0, &rect(LEFT), unit(&[0.98, 0.0, 0.2]));
    s.faces.index.insert(vis[2].0, &rect(RIGHT), unit(&bob));
    let r = s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid})).unwrap();
    let applied = r["applied"].as_array().unwrap();
    assert!(!applied.contains(&json!(vis[1].0)) && applied.contains(&json!(vis[2].0)), "{r}");
    // an explicit apply turns it down too: it was compared
    let r = s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0]})).unwrap();
    assert_eq!(r["portrait"]["skipped"][0]["id"], vis[1].0, "{r}");
    assert!(retouch(&s, vis[1]).faces.is_empty());
}

#[test]
fn an_exported_file_has_no_face_embedding_or_person_and_imports_as_an_any_face_preset() {
    let (mut s, vis, _) = ann_preset();
    s.faces.index.reset("stand-in@1", 2, None);
    s.faces.index.insert(vis[0].0, &rect(LEFT), vec![1.0, 0.0]);
    let r = s.execute("retouch.savePreset", &json!({"name": "Ann Look", "face": 0})).unwrap();
    let pid = r["id"].as_str().unwrap().to_string();
    let local = s.presets.iter().find(|p| p.id == pid).unwrap().clone();
    assert!(local.settings[KEY].get("embedding").is_some() && local.settings[KEY].get("personId").is_some(), "kept on this Mac");
    let dir = std::env::temp_dir().join(format!("lc-portrait-presets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Ann Look.lcpreset");
    s.execute("preset.export", &json!({"path": file.to_string_lossy(), "ids": [pid]})).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let part = &serde_json::from_str::<Value>(&text).unwrap()["presets"][0]["settings"][KEY];
    for k in ["embedding", "person", "personId"] {
        assert!(part.get(k).is_none(), "{k} is not in an exported file: {part}");
    }
    assert!(!text.contains("\"ann\"") && !text.contains("vector"), "{text}");
    // Settings Sync to the user's other Macs keeps the person, never the embedding
    let synced: Value = serde_json::from_str(&crate::presets::to_lcpreset_synced(std::slice::from_ref(&local))).unwrap();
    let part = &synced["presets"][0]["settings"][KEY];
    assert_eq!((part["person"].clone(), part.get("embedding")), (json!("ann"), None));
    assert_eq!(part["personId"], local.settings[KEY]["personId"]);

    let (mut t, vis) = demo_with(vec![]);
    let r = t.execute("preset.import", &json!({"paths": [file.to_string_lossy()]})).unwrap();
    let id = r["imported"][0]["id"].as_str().unwrap().to_string();
    let back = t.presets.iter().find(|p| p.id == id).unwrap().clone();
    assert_eq!((back.name.as_str(), back.group.as_str()), ("Ann Look", "Portrait"));
    assert_eq!(back.settings[KEY]["features"], local.settings[KEY]["features"], "the face settings round-trip");
    // nobody in it: it goes to every face of the photo
    regions(&mut t, vis[4], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("Ann"))]);
    let r = t.execute("preset.apply", &json!({"id": id, "ids": [vis[4].0]})).unwrap();
    assert_eq!(r["portrait"]["anyFace"], true, "{r}");
    assert_eq!(smoothness_of(&t, vis[4], &[face(LEFT, Some("bob")), face(RIGHT, Some("ann"))]), [70.0, 70.0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_preset_from_all_faces_goes_to_every_face_and_follows_nobody() {
    let (mut s, vis) = demo_with(vec![face(LEFT, Some("ann"))]);
    regions(&mut s, vis[0], vec![region(LEFT, Some("Ann"))]);
    s.execute("retouch.set", &json!({"control": SMOOTH, "value": 40})).unwrap();
    let r = s.execute("retouch.savePreset", &json!({"name": "Soft"})).unwrap();
    assert_eq!((r["anyFace"].clone(), r["person"].clone()), (json!(true), Value::Null), "{r}");
    let pid = r["id"].as_str().unwrap().to_string();
    // a photo where one face already has its own settings: both faces get it
    regions(&mut s, vis[1], vec![region(LEFT, Some("Bob")), region(RIGHT, None)]);
    let mut d = (*s.develop_of(vis[1]).unwrap()).clone();
    let faces = [face(LEFT, Some("bob")), face(RIGHT, None)];
    d.retouch.override_mut(&faces, 0).unwrap().set("retouch.eyes.whites", 20.0);
    s.set_develop(vis[1], d, "setup").unwrap();
    s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0]})).unwrap();
    assert_eq!(smoothness_of(&s, vis[1], &faces), [40.0, 40.0]);
    assert_eq!(retouch(&s, vis[1]).faces.len(), 1, "Bob keeps his own settings, now with the preset's");
    assert!(s.execute("retouch.applyToPerson", &json!({"preset": pid})).is_err(), "nobody to follow");
}

#[test]
fn a_renamed_person_keeps_the_preset_and_a_new_person_of_the_old_name_does_not_get_it() {
    let (mut s, vis, pid) = ann_preset();
    let id_before = s.presets.iter().find(|p| p.id == pid).unwrap().settings[KEY]["personId"].clone();
    s.execute("faces.renamePerson", &json!({"from": "Ann", "to": "Anna"})).unwrap();
    let part = s.presets.iter().find(|p| p.id == pid).unwrap().settings[KEY].clone();
    assert_eq!((part["person"].clone(), part["personId"].clone()), (json!("anna"), id_before));
    regions(&mut s, vis[1], vec![region(LEFT, Some("Bob")), region(RIGHT, Some("Anna"))]);
    regions(&mut s, vis[2], vec![region(LEFT, Some("Ann")), region(RIGHT, Some("Bob"))]);
    let r = s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0, vis[2].0]})).unwrap();
    assert_eq!(r["portrait"]["applied"], json!([vis[1].0]), "{r}");
    assert!(retouch(&s, vis[2]).faces.is_empty(), "the new Ann is someone else");
}

#[test]
fn a_preset_carries_every_feature_and_other_settings_only_when_asked_never_masks_or_spots() {
    let (mut s, vis, pid) = ann_preset();
    let p = s.presets.iter().find(|p| p.id == pid).unwrap().clone();
    let keys: Vec<&String> = p.settings.as_object().unwrap().keys().collect();
    assert_eq!(keys, [KEY], "face settings only by default");
    let features = p.settings[KEY]["features"].as_object().unwrap();
    assert_eq!(features.len(), FEATURES.len(), "every feature is carried: {features:?}");
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
    let with = s.execute("retouch.savePreset", &json!({"name": "Ann and Light", "face": 0, "others": true})).unwrap();
    assert_eq!(with["wholeLook"], true);
    let w = s.presets.iter().find(|p| p.id == with["id"]).unwrap().clone();
    assert_eq!(w.settings["light"]["exposure"], 0.7);
    let asked =
        s.execute("retouch.savePreset", &json!({"name": "Asked", "face": 0, "groups": ["spots", "redEye", "portrait", "masks", "light"]})).unwrap();
    let sp = s.presets.iter().find(|p| p.id == asked["id"]).unwrap().clone();
    for k in ["spots", "red_eye", "fork_retouch", "masks"] {
        assert!(sp.settings.get(k).is_none(), "{k} is never carried: {}", sp.settings);
    }
    // the other settings apply as any preset's
    s.execute("preset.apply", &json!({"id": w.id, "ids": [vis[1].0]})).unwrap();
    assert_eq!(s.develop_of(vis[1]).unwrap().light.exposure, 0.7);
    // a malformed face part is no panic
    let mut bad = w.clone();
    bad.id = "user.bad".into();
    bad.settings[KEY] = json!({"features": {"skin": "x"}, "person": 7, "embedding": {"model": 1}});
    s.presets.push(bad);
    s.execute("preset.apply", &json!({"id": "user.bad", "ids": [vis[2].0]})).unwrap();
}

#[test]
fn the_hover_preview_shows_what_a_click_commits() {
    let (mut s, vis, pid) = ann_preset();
    s.faces.index.reset("stand-in@1", 2, None);
    s.faces.index.insert(vis[0].0, &rect(LEFT), vec![1.0, 0.0]);
    // an unnamed face only the library's named faces recognise
    regions(&mut s, vis[1], vec![region(LEFT, None), region(RIGHT, Some("Bob"))]);
    s.faces.index.insert(vis[1].0, &rect(LEFT), unit(&[0.99, 0.1]));
    let preset = s.presets.iter().find(|p| p.id == pid).unwrap().clone();
    let preview = s.preset_applied(vis[1], &preset, 1.0).unwrap();
    s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0]})).unwrap();
    assert_eq!(preview.retouch, retouch(&s, vis[1]));
    assert_eq!(retouch(&s, vis[1]).faces.len(), 1, "both found her");
}

/// A face that can't have its own settings is reported, and the rest still change as one step.
#[test]
fn a_photo_that_cant_take_it_is_reported_and_the_others_still_change() {
    let (mut s, vis, pid) = ann_preset();
    regions(&mut s, vis[1], vec![region(RIGHT, Some("Ann"))]);
    regions(&mut s, vis[2], vec![region(RIGHT, Some("Ann"))]);
    // photo 2: every one of its face slots taken by other faces
    let mut d = (*s.develop_of(vis[2]).unwrap()).clone();
    for i in 0..lightcraft_develop::retouch::MAX_OVERRIDES {
        let x = (i % 20) as f64 * 0.001;
        d.retouch.faces.push(lightcraft_develop::retouch::FaceOverride {
            face: lightcraft_develop::retouch::FaceKey {
                person: Some(format!("p{i}")),
                bounds: NormRect::new(x, 0.9, x + 0.01, 0.95),
                frame: KeyFrame::Photo,
            },
            ..Default::default()
        });
    }
    s.set_develop(vis[2], d, "setup").unwrap();
    let r = s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid})).unwrap();
    assert_eq!(r["failed"][0]["id"], vis[2].0, "{r}");
    assert!(r["applied"].as_array().unwrap().contains(&json!(vis[1].0)), "{r}");
    assert_eq!(smoothness_of(&s, vis[1], &[face(RIGHT, Some("ann"))]), [70.0]);
}

/// A library of `n` photos without files, Ann on every one.
fn library_of_ann(n: usize) -> (Session, Vec<PhotoId>) {
    let mut s = Session::new();
    let mut ids = Vec::new();
    for k in 0..n {
        let id = s.catalog.alloc_photo_id();
        let mut p = Photo::new(id, Source::File { path: format!("/photos/{k}.jpg") }, &format!("{k}.jpg"), "JPEG", 64, 64, "2026-01-01T00:00:00");
        p.meta.regions = vec![region(LEFT, Some("Bob")), region(RIGHT, Some("Ann"))];
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        ids.push(id);
    }
    (s, ids)
}

fn ann_look(s: &mut Session) -> String {
    let mut f = lightcraft_develop::retouch::Features::default();
    f.set(SMOOTH, 55.0);
    let preset = crate::portrait_presets::make(
        "Ann",
        "Portrait",
        &crate::portrait_presets::Portrait::of(&f, crate::portrait_presets::Who::named("Ann")),
        json!({}),
    );
    s.add_presets(vec![preset]).remove(0)
}

/// Past a handful of photos it runs on a worker with an activity row, and is committed (one step) when done.
#[test]
fn many_photos_are_worked_out_in_the_background_with_an_activity_row() {
    let n = crate::portrait_presets::person::HANDFUL + 6;
    let (mut s, ids) = library_of_ann(n);
    let pid = ann_look(&mut s);
    let r = s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid})).unwrap();
    assert_eq!(r["state"], "running", "{r}");
    assert!(s.activity.list().iter().any(|t| t.kind == "portrait"), "an activity row while it runs");
    assert!(s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid})).is_err(), "one at a time");
    let undo_before = s.undo.len();
    let r = s.execute("retouch.applyStatus", &json!({"wait": true})).unwrap();
    assert_eq!((r["state"].clone(), r["count"].clone()), (json!("done"), json!(n)), "{r}");
    assert_eq!(s.undo.len() - undo_before, 1);
    assert!(!s.activity.list().iter().any(|t| t.kind == "portrait"), "the row ends with it");
    assert_eq!(smoothness_of(&s, ids[n - 1], &[face(LEFT, Some("bob")), face(RIGHT, Some("ann"))]), [0.0, 55.0]);
    assert_eq!(s.execute("retouch.applyStatus", &json!({})).unwrap()["state"], "idle");
    // cancelled: nothing changes
    let pid2 = {
        let mut f = lightcraft_develop::retouch::Features::default();
        f.set(SMOOTH, 10.0);
        let p = crate::portrait_presets::make(
            "Ann 2",
            "Portrait",
            &crate::portrait_presets::Portrait::of(&f, crate::portrait_presets::Who::named("Ann")),
            json!({}),
        );
        s.add_presets(vec![p]).remove(0)
    };
    s.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": pid2})).unwrap();
    let r = s.execute("retouch.applyStatus", &json!({"cancel": true, "wait": true})).unwrap();
    // (the worker may have finished before the cancel)
    let want = if r["state"] == "cancelled" { 55.0 } else { 10.0 };
    assert!(r["state"] == "cancelled" || r["state"] == "done", "{r}");
    assert_eq!(smoothness_of(&s, ids[0], &[face(LEFT, Some("bob")), face(RIGHT, Some("ann"))]), [0.0, want]);
}

/// A portrait preset carries a face's sliders, never its Refine strokes (#99): saved from a face with strokes (its
/// own and every face's), applied to the same person on another photo, the strokes stay on the first photo only.
#[test]
fn a_portrait_preset_never_carries_refine_strokes() {
    let (mut s, vis) = demo_with(vec![face(LEFT, Some("ann")), face(RIGHT, Some("bob"))]);
    regions(&mut s, vis[0], vec![region(LEFT, Some("Ann")), region(RIGHT, Some("Bob"))]);
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": SMOOTH, "value": 70, "face": 0})).unwrap();
    s.execute("retouch.refine.stroke", &json!({"feature": "skin", "points": [[0.2, 0.3]], "face": 0})).unwrap();
    s.execute("retouch.refine.stroke", &json!({"feature": "skin", "mode": "add", "points": [[0.25, 0.35]]})).unwrap();
    let r = s.execute("retouch.savePreset", &json!({"name": "Ann Look", "face": 0, "others": true})).unwrap();
    let pid = r["id"].as_str().unwrap().to_string();
    // (the stored preset itself: the list shows names only)
    let stored = s.presets.iter().find(|p| p.id == pid).unwrap().settings[KEY].to_string();
    let first = retouch(&s, vis[0]);
    assert_eq!((first.refine.count(None), first.faces[0].refine.count(None)), (1, 1));
    // (the JSON that would carry strokes shows the word `strokes`, so this check can fail)
    assert!(serde_json::to_string(&first).unwrap().contains("\"strokes\""));
    assert!(stored.contains("smoothness") && stored.contains("70"), "the stored preset has the sliders: {stored}");
    assert!(!stored.contains("strokes") && !stored.contains("refine"), "no strokes in the saved preset: {stored}");
    regions(&mut s, vis[1], vec![region(LEFT, Some("Ann"))]);
    s.execute("preset.apply", &json!({"id": pid, "ids": [vis[1].0]})).unwrap();
    let got = retouch(&s, vis[1]);
    assert_eq!(got.faces.len(), 1, "Ann's face got the sliders");
    assert_eq!(smoothness_of(&s, vis[1], &[face(LEFT, Some("ann"))]), [70.0], "(the preset was applied)");
    assert!(got.refine.is_empty() && got.faces[0].refine.is_empty(), "and no strokes");
    assert_eq!(retouch(&s, vis[0]).faces[0].refine.count(None), 1, "the first photo keeps its own");
}
