//! Select People picker, second round (PR #123's reviews): a saved face not in the photo shows no chip and says so;
//! a mask asked for before the people were found is still made after visiting another photo; one set of face
//! textures; the search starts when the panel opens on a photo, not from every frame.

use std::sync::{Arc, Condvar, Mutex};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::select_people::PeopleState;
use lightcraft_engine::select_people::parsing::Parsing;
use serde_json::json;

use super::face_and_note;
use super::tests::{T, click, masks, person, until};
use crate::headless::Headless;
use crate::state::RightPanel;

#[test]
fn a_saved_face_not_in_the_photo_picks_no_chip_and_says_so() {
    let dir = std::env::temp_dir().join(format!("lc-ui-select-people-missing-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut s = lightcraft_engine::Session::with_demo();
    s.parsing.set_paths(Some(dir.join("models")), None, None);
    let parse = Parsing { model_set: "segformer".into(), model_version: "1".into(), width: 2048, height: 1365, people: vec![person(0.1, None)] };
    s.parsing.set_runner(Some(Arc::new(move |_, _, _, _, _| Ok(parse.clone()))));
    let photo = s.catalog.photos().map(|p| p.id).min().unwrap();
    let st = PeopleState { ready: Some(s.people_now(photo).unwrap()), busy: false, error: None };
    // the selected part's face is not found: no chip, and the line
    let (face, note) = face_and_note(&st, Some((None, false)), Some(0));
    assert_eq!(face, Some(1), "an index past the one chip: none picked");
    assert_eq!(note.as_deref(), Some("Face not in this photo"));
    // everyone's part: All, no line; a found face: its chip
    assert_eq!(face_and_note(&st, Some((None, true)), Some(0)), (None, None));
    assert_eq!(face_and_note(&st, Some((Some(0), false)), None), (Some(0), None));
    // no part selected: the face picked last
    assert_eq!(face_and_note(&st, None, Some(0)), (Some(0), None));
    let _ = std::fs::remove_dir_all(dir);
}

/// The app on the demo library with a runner that waits at `gate` until it is opened.
fn gated(tag: &str) -> (Headless, Arc<(Mutex<bool>, Condvar)>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-ui-select-people-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let session = lightcraft_engine::Session::with_demo();
    session.parsing.set_paths(Some(dir.join("models")), None, None);
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let parse = Parsing {
        model_set: "segformer".into(),
        model_version: "1".into(),
        width: 2048,
        height: 1365,
        people: vec![person(0.1, None), person(0.6, None)],
    };
    let g = gate.clone();
    session.parsing.set_runner(Some(Arc::new(move |_, _, _, _, _| {
        let (open, cv) = &*g;
        let mut o = open.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*o {
            o = cv.wait(o).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        Ok(parse.clone())
    })));
    let app = crate::LightcraftApp::new(session, crate::Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1100.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = RightPanel::Masking;
    h.step();
    h.step();
    (h, gate, dir)
}

fn open(gate: &(Mutex<bool>, Condvar)) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

fn show(h: &mut Headless, id: PhotoId) {
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [id.0], "active": id.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn others(h: &Headless, photo: PhotoId) -> Vec<PhotoId> {
    let mut ids: Vec<PhotoId> = h.app.session.catalog.photos().map(|p| p.id).filter(|id| *id != photo).collect();
    ids.sort();
    ids
}

#[test]
fn a_mask_asked_for_is_made_after_visiting_another_photo() {
    let (mut h, gate, dir) = gated("pending");
    let first = h.app.session.active().unwrap();
    // Person while the people are still being found: nothing yet
    click(&mut h, "button:maskMode-person", false);
    assert!(h.app.session.people_of(first).busy);
    assert!(masks(&h).is_empty());
    // another photo meanwhile
    let second = others(&h, first)[0];
    show(&mut h, second);
    open(&gate);
    show(&mut h, first);
    until(&mut h, "the asked-for mask", |h| !masks(h).is_empty());
    assert_eq!(masks(&h).len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// The face textures alive, by name.
fn face_textures(h: &Headless) -> usize {
    let m = h.view.ctx.tex_manager();
    let m = m.read();
    m.allocated().filter(|(_, meta)| meta.name.starts_with("select-people-")).count()
}

#[test]
fn one_set_of_face_textures_is_kept() {
    let (mut h, gate, dir) = gated("textures");
    open(&gate);
    let first = h.app.session.active().unwrap();
    click(&mut h, "button:maskMode-person", false);
    until(&mut h, "the first photo's people", |h| h.app.session.people_of(first).ready.is_some());
    assert_eq!(face_textures(&h), 2);
    for other in others(&h, first).into_iter().take(3) {
        show(&mut h, other);
        until(&mut h, "the next photo's people", |h| h.app.session.people_of(other).ready.is_some());
    }
    assert_eq!(face_textures(&h), 2, "the shown photo's two faces only");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_search_starts_when_the_panel_opens_not_from_every_frame() {
    let (mut h, gate, dir) = gated("start");
    open(&gate);
    let first = h.app.session.active().unwrap();
    click(&mut h, "button:maskMode-person", false);
    until(&mut h, "the people", |h| h.app.session.people_of(first).ready.is_some());
    // the people of eight other photos push this photo's out of the memory
    let rest = others(&h, first);
    assert!(rest.len() >= 8, "the demo library has {} other photos", rest.len());
    for id in rest.into_iter().take(8) {
        h.app.session.people_now(id).unwrap();
    }
    assert!(h.app.session.people_of(first).ready.is_none());
    // frames go by: no new search
    for _ in 0..5 {
        h.step();
    }
    let st = h.app.session.people_of(first);
    assert!(!st.busy && st.ready.is_none(), "a frame started a search");
    // the panel opened again on the photo: found again
    h.app.ui.right = RightPanel::Edit;
    h.step();
    h.app.ui.right = RightPanel::Masking;
    until(&mut h, "the people again", |h| h.app.session.people_of(first).ready.is_some());
    let _ = std::fs::remove_dir_all(dir);
}
