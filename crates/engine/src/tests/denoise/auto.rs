//! Auto AI Denoise (`denoise.autoOpen`): a high-ISO raw gets AI Denoise switched on when it is opened, once.

use lightcraft_catalog::{Op, PhotoId};
use serde_json::{Value, json};

use super::{Setup, model_files, photo, pump_until_ready, setup};
use crate::Session;

fn set_iso(s: &mut Session, id: PhotoId, iso: Option<u32>) {
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.iso = iso;
    s.commit("ISO", Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
}

fn turn_on(s: &mut Session) {
    s.execute("denoise.settings", &json!({"autoOpen": true})).unwrap();
}

fn open(s: &mut Session, id: PhotoId) -> Value {
    s.execute("denoise.autoOpen", &json!({"id": id.0})).unwrap()
}

fn on(s: &Session, id: PhotoId) -> bool {
    s.develop_of(id).unwrap().enhance.denoise_enabled()
}

fn with_iso(tag: &str, with_model: bool, iso: u32) -> (Setup, PhotoId) {
    let mut x = setup(tag, with_model);
    let id = photo(&x.s);
    set_iso(&mut x.s, id, Some(iso));
    (x, id)
}

fn finish(x: Setup) {
    drop(x.s);
    let _ = std::fs::remove_dir_all(x.dir);
}

#[test]
fn it_is_off_until_the_setting_is_turned_on() {
    let (mut x, id) = with_iso("auto-off", true, 12_800);
    let s = x.s.execute("denoise.settings", &json!({})).unwrap();
    assert_eq!((s["autoOpen"].clone(), s["autoIso"].clone()), (json!(false), json!(6400)));
    let r = open(&mut x.s, id);
    assert_eq!((r["applied"].clone(), r["reason"].clone()), (json!(false), json!("off")), "{r}");
    assert!(!on(&x.s, id));
    finish(x);
}

#[test]
fn a_photo_at_or_above_the_iso_gets_ai_denoise_as_one_undo_step() {
    let (mut x, id) = with_iso("auto-apply", true, 6400);
    turn_on(&mut x.s);
    let r = open(&mut x.s, id);
    assert_eq!((r["applied"].clone(), r["reason"].clone(), r["amount"].clone()), (json!(true), json!("applied"), json!(50.0)), "{r}");
    assert!(on(&x.s, id));
    assert_eq!(x.s.undo.last().map(|u| u.label.as_str()), Some("Auto AI Denoise"));
    // its picture is made by the usual queue
    pump_until_ready(&mut x.s, 1);
    assert_eq!(x.s.denoise_photo_state(id), crate::denoise::PhotoState::Ready);
    // one undo step takes it all back
    x.s.execute("edit.undo", &json!({})).unwrap();
    assert!(!on(&x.s, id));
    finish(x);
}

#[test]
fn below_the_threshold_without_an_iso_or_with_another_threshold() {
    let (mut x, id) = with_iso("auto-below", true, 3200);
    turn_on(&mut x.s);
    assert_eq!(open(&mut x.s, id)["reason"], "belowThreshold");
    // the user's threshold counts
    x.s.execute("denoise.settings", &json!({"autoIso": 3200})).unwrap();
    assert_eq!(open(&mut x.s, id)["reason"], "applied");
    assert!(x.s.execute("denoise.settings", &json!({"autoIso": 0})).is_err(), "an ISO of 0 is refused");
    // a file without an ISO is left alone
    x.s.execute("edit.undo", &json!({})).unwrap();
    set_iso(&mut x.s, id, None);
    x.s.execute("denoise.settings", &json!({"autoIso": 100})).unwrap();
    assert_eq!(open(&mut x.s, id)["reason"], "noIso");
    assert!(!on(&x.s, id));
    finish(x);
}

#[test]
fn it_is_never_applied_again_after_an_undo_or_a_switch_off() {
    let (mut x, id) = with_iso("auto-once", true, 12_800);
    turn_on(&mut x.s);
    assert_eq!(open(&mut x.s, id)["reason"], "applied");
    // undone: opening it again does not bring it back
    x.s.execute("edit.undo", &json!({})).unwrap();
    assert!(!on(&x.s, id));
    assert_eq!(open(&mut x.s, id)["reason"], "alreadyDone");
    assert!(!on(&x.s, id));
    // and neither does a switch the user turned off
    x.s.execute("denoise.toggle", &json!({"id": id.0, "enabled": true})).unwrap();
    x.s.execute("denoise.toggle", &json!({"id": id.0, "enabled": false})).unwrap();
    assert_eq!(open(&mut x.s, id)["reason"], "alreadyDone");
    assert!(!on(&x.s, id));
    finish(x);
}

/// The library of `x` opened again in a new session, as after a restart.
fn restart(x: Setup) -> (Session, PhotoId, std::path::PathBuf) {
    let mut x = x;
    x.s.persist().unwrap();
    let dir = x.dir.clone();
    drop(x.s);
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("library"), false).unwrap();
    s.set_denoise_models_dir(Some(dir.join("denoise-models")));
    s.denoise.loader = super::dim_loader(&x.runs);
    let id = photo(&s);
    (s, id, dir)
}

#[test]
fn the_switch_is_stored_with_the_photo() {
    let (mut x, id) = with_iso("auto-stored", true, 12_800);
    turn_on(&mut x.s);
    assert_eq!(open(&mut x.s, id)["reason"], "applied");
    let (mut s, id, dir) = restart(x);
    assert!(on(&s, id), "AI Denoise is still on after a restart");
    s.execute("denoise.settings", &json!({"autoOpen": true})).unwrap();
    assert_eq!(open(&mut s, id)["reason"], "alreadyDone");
    drop(s);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_was_handled_survives_a_restart() {
    let (mut x, id) = with_iso("auto-restart", true, 12_800);
    turn_on(&mut x.s);
    assert_eq!(open(&mut x.s, id)["reason"], "applied");
    x.s.execute("edit.undo", &json!({})).unwrap();
    let (mut s, id, dir) = restart(x);
    assert!(!on(&s, id));
    s.execute("denoise.settings", &json!({"autoOpen": true})).unwrap();
    assert_eq!(open(&mut s, id)["reason"], "alreadyDone", "the undone edit does not come back");
    assert!(!on(&s, id));
    drop(s);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_photo_the_user_already_set_is_left_alone() {
    let (mut x, id) = with_iso("auto-set", true, 12_800);
    turn_on(&mut x.s);
    x.s.execute("develop.set", &json!({"control": "enhance.denoise", "value": 30.0})).unwrap();
    assert_eq!(open(&mut x.s, id)["reason"], "alreadySet");
    assert_eq!(x.s.develop_of(id).unwrap().enhance.denoise, 30.0);
    finish(x);
}

#[test]
fn without_a_model_nothing_happens_until_one_is_installed() {
    let (mut x, id) = with_iso("auto-nomodel", false, 12_800);
    turn_on(&mut x.s);
    let undo = x.s.undo.len();
    let r = open(&mut x.s, id);
    assert_eq!((r["applied"].clone(), r["reason"].clone()), (json!(false), json!("noModel")), "{r}");
    assert!(!on(&x.s, id));
    assert_eq!(x.s.undo.len(), undo);
    // the photo was not used up: once a model is installed, opening it applies
    let onnx = model_files(&x.dir, "stand-in");
    x.s.execute("denoise.models.install", &json!({"path": onnx.to_string_lossy(), "acknowledged": true})).unwrap();
    assert_eq!(open(&mut x.s, id)["reason"], "applied");
    assert!(on(&x.s, id));
    finish(x);
}

#[test]
fn an_unknown_photo_is_an_error_not_a_panic() {
    let (mut x, _) = with_iso("auto-unknown", true, 12_800);
    turn_on(&mut x.s);
    assert!(x.s.execute("denoise.autoOpen", &json!({"id": 9_999_999})).is_err());
    assert!(x.s.execute("denoise.autoOpen", &json!({"id": "x"})).is_err());
    finish(x);
}
