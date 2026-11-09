//! Select People in the Masking panel (`fork/select-people.md`): Person, Background and Custom; the picker on the
//! photo makes and changes masks; ⌘-click takes several parts; it folds and hides with the tool overlay; no people
//! is a message. A fake runner stands in for the people parser.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lightcraft_develop::MaskShape;
use lightcraft_engine::select_people::parsing::{NormRect, Parsing, Part, Person, Plane};
use serde_json::json;

use super::{FOLDED, Mode, mode};
use crate::headless::Headless;
use crate::state::{RightPanel, ToolOverlay};
use crate::{LightcraftApp, Services};

pub(super) const T: Duration = Duration::from_secs(20);

pub(super) fn person(x: f32, name: Option<&str>) -> Person {
    let mut q = Person {
        face: Some(NormRect::new(x + 0.05, 0.1, x + 0.2, 0.35)),
        face_score: 0.9,
        person_id: name.map(str::to_string),
        bounds: Some(NormRect::new(x, 0.1, x + 0.25, 0.95)),
        ..Person::default()
    };
    for (i, part) in Part::ALL.into_iter().enumerate() {
        let y = 0.1 + i as f32 * 0.05;
        let r = if part == Part::EntirePerson { NormRect::new(x, 0.1, x + 0.25, 0.95) } else { NormRect::new(x + 0.02, y, x + 0.2, y + 0.04) };
        q.parts.insert(part, vec![Plane { rect: r, width: 8, height: 8, data: vec![1.0; 64], split: false }]);
    }
    q
}

pub(super) fn app(people: Vec<Person>) -> (Headless, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-ui-select-people-{}-{}", people.len(), std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let session = lightcraft_engine::Session::with_demo();
    session.parsing.set_paths(Some(dir.join("models")), None, None);
    let parse = Parsing { model_set: "segformer".into(), model_version: "1".into(), width: 2048, height: 1365, people };
    session.parsing.set_runner(Some(Arc::new(move |_, _, _, _, _| Ok(parse.clone()))));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1100.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = RightPanel::Masking;
    h.step();
    h.step();
    (h, dir)
}

pub(super) fn rect_of(h: &mut Headless, id: &str) -> Option<[f64; 4]> {
    let r = h.request("ui.widgets", json!({"filter": id}), T);
    r["result"].as_array().into_iter().flatten().find(|w| w["id"] == id).and_then(|w| serde_json::from_value(w["rect"].clone()).ok())
}

pub(super) fn click(h: &mut Headless, id: &str, cmd: bool) {
    let r = h.request("ui.clickWidget", json!({"id": id, "cmd": cmd}), T);
    assert_eq!(r["ok"], true, "click {id}: {r}");
    h.step();
}

pub(super) fn masks(h: &Headless) -> Vec<lightcraft_develop::Mask> {
    let id = h.app.session.active().unwrap();
    h.app.session.develop_of(id).map(|d| d.masks.clone()).unwrap_or_default()
}

/// The selected mask's Select People part: (its part ids, whether it has a face).
pub(super) fn part(h: &Headless) -> (Vec<String>, bool) {
    let mid = h.app.session.active_mask.unwrap();
    let m = masks(h).into_iter().find(|m| m.id == mid).unwrap();
    match &m.components[0].shape {
        MaskShape::People { parts, face, seg: Some(_), .. } => (parts.clone(), face.is_some()),
        other => panic!("not a Select People part: {other:?}"),
    }
}

/// Step until `done` (a guard against a hang only).
pub(super) fn until(h: &mut Headless, what: &str, done: impl Fn(&Headless) -> bool) {
    let end = Instant::now() + Duration::from_secs(60);
    while !done(h) {
        assert!(Instant::now() < end, "never: {what}");
        h.step();
    }
    h.step();
    h.step();
}

/// The parts are in a dropdown, closed: the picker is one short row over the photo.
fn one_short_row(h: &mut Headless) {
    assert!(rect_of(h, "selectPeople:parts").is_some());
    assert!(rect_of(h, "selectPeople:part:hair").is_none(), "the parts menu starts closed");
    let card = rect_of(h, "selectPeople:card").unwrap();
    assert!(card[3] < 60.0, "one row: {card:?}");
}

#[test]
fn person_makes_a_mask_and_the_picker_on_the_photo_changes_it() {
    let (mut h, dir) = app(vec![person(0.1, Some("ann")), person(0.6, None)]);
    // Custom: the usual tiles, no picker
    assert_eq!(mode(&h.view.ctx), Mode::Custom);
    assert!(rect_of(&mut h, "maskNew:subject").is_some());
    assert!(rect_of(&mut h, "selectPeople:card").is_none());
    // Person: an Entire Person mask of everyone once the people are found; the picker shows, the tiles go
    click(&mut h, "button:maskMode-person", false);
    until(&mut h, "a Person mask", |h| !masks(h).is_empty());
    assert_eq!(part(&h), (vec!["entirePerson".to_string()], false));
    assert!(rect_of(&mut h, "maskNew:subject").is_none(), "Person hides the Custom tiles");
    assert!(rect_of(&mut h, "selectPeople:card").is_some());
    assert!(rect_of(&mut h, "selectPeople:face:1").is_some(), "a chip per person");
    one_short_row(&mut h);
    // a plain click picks one part (opening the menu first) and closes it; ⌘-click adds one; all in the same mask
    click(&mut h, "selectPeople:part:hair", false);
    assert!(rect_of(&mut h, "selectPeople:partsMenu").is_none(), "a plain click closes the menu");
    assert_eq!(part(&h).0, ["hair"]);
    click(&mut h, "selectPeople:part:lips", true);
    assert_eq!(part(&h).0, ["hair", "lips"]);
    assert!(rect_of(&mut h, "selectPeople:partsMenu").is_some(), "⌘-click keeps the menu open");
    click(&mut h, "selectPeople:part:hair", true);
    assert_eq!(part(&h).0, ["lips"]);
    // the last part stays: a mask of nothing is not made
    click(&mut h, "selectPeople:part:lips", true);
    assert_eq!(part(&h).0, ["lips"]);
    // a face chip: that person's parts only; All again
    click(&mut h, "selectPeople:face:1", false);
    assert_eq!(part(&h), (vec!["lips".to_string()], true));
    click(&mut h, "selectPeople:face:all", false);
    assert!(!part(&h).1);
    assert_eq!(masks(&h).len(), 1);
    // Background: a second mask
    click(&mut h, "button:maskMode-background", false);
    assert_eq!(masks(&h).len(), 2);
    assert_eq!(part(&h).0, ["background"]);
    // folded into its button, and open again
    click(&mut h, "selectPeople:more", false);
    assert!(h.app.ui.flyout_open(FOLDED));
    assert!(rect_of(&mut h, "selectPeople:parts").is_none());
    click(&mut h, "selectPeople:more", false);
    assert!(rect_of(&mut h, "selectPeople:parts").is_some());
    // hidden with the tool overlay
    h.app.ui.tool_overlay = ToolOverlay::Never;
    h.step();
    h.step();
    assert!(rect_of(&mut h, "selectPeople:card").is_none());
    h.app.ui.tool_overlay = ToolOverlay::Auto;
    // Custom: the tiles again; the picker stays while a Select People mask is selected, and goes with another
    click(&mut h, "button:maskMode-custom", false);
    assert!(rect_of(&mut h, "maskNew:subject").is_some());
    assert!(rect_of(&mut h, "selectPeople:card").is_some());
    let r = h.request("engine.execute", json!({"command": "mask.add", "params": {"kind": "radial"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(rect_of(&mut h, "selectPeople:card").is_none());
    // in another panel, nothing
    h.app.ui.right = RightPanel::Edit;
    h.step();
    assert!(rect_of(&mut h, "selectPeople:card").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_photo_without_people_says_so_and_makes_no_person_mask() {
    let (mut h, dir) = app(Vec::new());
    click(&mut h, "button:maskMode-person", false);
    let photo = h.app.session.active().unwrap();
    until(&mut h, "the people found", |h| h.app.session.people_of(photo).ready.is_some());
    assert!(masks(&h).is_empty(), "no Entire Person without people");
    assert!(rect_of(&mut h, "selectPeople:note").is_some(), "the picker says there are none");
    // the background is all of it
    click(&mut h, "button:maskMode-background", false);
    assert_eq!(masks(&h).len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}
