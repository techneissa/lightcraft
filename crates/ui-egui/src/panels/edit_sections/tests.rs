use std::time::Duration;

use serde_json::{Value, json};

use super::EditSection::{Calibration, Color, Detail, Effects, Light, Optics};
use super::*;
use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn order(layout: &EditSectionLayout) -> Vec<EditSection> {
    layout.sections().iter().map(|s| s.id).collect()
}

fn read(saved: Value) -> EditSectionLayout {
    serde_json::from_value(saved).unwrap()
}

#[test]
fn a_new_panel_lists_every_section_shown_in_the_usual_order() {
    let layout = EditSectionLayout::default();
    assert_eq!(order(&layout), [Light, Color, Effects, Detail, Optics, Calibration]);
    assert!(!layout.any_hidden() && layout.is_default());
}

#[test]
fn an_arrangement_survives_saving() {
    let mut layout = EditSectionLayout::default();
    layout.move_before(Optics, Some(Light));
    layout.set_hidden(Calibration, true);
    let saved = serde_json::to_value(&layout).unwrap();
    assert_eq!(saved[0], json!({"id": "optics", "hidden": false}));
    assert_eq!(read(saved), layout);
}

#[test]
fn a_damaged_or_missing_arrangement_is_made_whole() {
    let layout = read(json!([{"id": "detail", "hidden": true}, {"id": "map"}, "light", {"id": "detail"}, {"id": 7}, null]));
    assert_eq!(order(&layout), [Detail, Light, Color, Effects, Optics, Calibration]);
    assert!(layout.is_hidden(Detail), "the first entry for a section counts");
    for saved in [json!(null), json!("light"), json!(3), json!({"light": true}), json!([])] {
        assert!(read(saved).is_default());
    }
    // every section hidden is not an arrangement the panel can be left in
    let all_hidden = read(json!(["light", "color", "effects", "detail", "optics", "calibration"].map(|id| json!({"id": id, "hidden": true}))));
    assert_eq!(all_hidden.shown(), [Light]);
}

#[test]
fn moving_steps_over_hidden_sections_and_stops_at_the_ends() {
    let mut layout = EditSectionLayout::default();
    layout.set_hidden(Effects, true);
    assert!(layout.move_by(Detail, true), "Detail goes above Color, past the hidden Effects");
    assert_eq!(layout.shown(), [Light, Detail, Color, Optics, Calibration]);
    assert!(!layout.can_move(Light, true) && !layout.can_move(Calibration, false));
    assert!(!layout.move_by(Light, true) && !layout.move_by(Effects, true), "the first and a hidden section stay");
    assert!(layout.move_before(Light, None));
    assert_eq!(layout.shown().last(), Some(&Light));
}

#[test]
fn the_last_shown_section_cannot_be_hidden() {
    let mut layout = EditSectionLayout::default();
    for s in [Color, Effects, Detail, Optics, Calibration] {
        assert!(layout.set_hidden(s, true));
    }
    assert!(!layout.set_hidden(Light, true));
    assert_eq!(layout.shown(), [Light]);
    layout.show_all();
    assert!(!layout.any_hidden());
}

#[test]
fn requests_are_read_strictly() {
    let layout = EditSectionLayout::default();
    let next = layout.with_request(Some(&json!(["optics", "light"])), Some(&json!(["effects"]))).unwrap();
    assert_eq!(order(&next), [Optics, Light, Color, Effects, Detail, Calibration]);
    assert_eq!(next.shown(), [Optics, Light, Color, Detail, Calibration]);
    // a key left out keeps what is there
    assert_eq!(next.with_request(None, Some(&json!([]))).unwrap().shown().len(), 6);
    assert_eq!(order(&next.with_request(None, Some(&json!(["color"]))).unwrap()), order(&next));
    for (o, h, says) in [
        (json!("light"), json!(null), "must be a list"),
        (json!(["light", "light"]), json!(null), "listed twice"),
        (json!(["lite"]), json!(null), "unknown section `lite`"),
        (json!(null), json!([3]), "names no section"),
        (json!(null), json!(["light", "color", "effects", "detail", "optics", "calibration"]), "at least one"),
    ] {
        let e = layout.with_request(Some(&o), Some(&h)).unwrap_err();
        assert!(e.contains(says), "{e}");
    }
}

/// An Edit panel on a demo photo, Detail view, with every section open.
fn edit_open() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1600.0], 1.0);
    h.request("ui.set", json!({"view": "detail"}), T);
    h.app.ui.right = crate::state::RightPanel::Edit;
    h.app.ui.open_sections = EditSection::ALL.map(|s| s.id().to_string()).to_vec();
    h.step();
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.step();
    r["result"].clone()
}

/// Section headers on screen, top to bottom.
fn headers(h: &mut Headless) -> Vec<String> {
    let r = h.request("ui.widgets", json!({"filter": "section:"}), T);
    let mut found: Vec<(f64, String)> = r["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| Some((w["rect"]["y"].as_f64().or_else(|| w["rect"][1].as_f64())?, w["id"].as_str()?.strip_prefix("section:")?.to_string())))
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found.into_iter().map(|(_, id)| id).collect()
}

#[test]
fn the_panel_draws_the_sections_in_the_chosen_order_and_skips_hidden_ones() {
    let mut h = edit_open();
    assert_eq!(headers(&mut h), ["light", "color", "effects", "detail", "optics", "calibration"]);
    exec(&mut h, "view.editSections", json!({"order": ["optics", "light"], "hidden": ["effects", "calibration"]}));
    assert_eq!(headers(&mut h), ["optics", "light", "color", "detail"]);
    exec(&mut h, "view.editSectionsReset", json!({}));
    assert_eq!(headers(&mut h), ["light", "color", "effects", "detail", "optics", "calibration"]);
}

#[test]
fn a_hidden_section_keeps_its_adjustments() {
    let mut h = edit_open();
    exec(&mut h, "develop.set", json!({"control": "effects.clarity", "value": 40.0}));
    let before = h.app.session.develop_of(h.app.session.active().unwrap()).unwrap();
    exec(&mut h, "view.editSectionShow", json!({"section": "effects", "show": false}));
    assert!(!headers(&mut h).contains(&"effects".to_string()));
    let after = h.app.session.develop_of(h.app.session.active().unwrap()).unwrap();
    assert_eq!(after.effects.clarity, 40.0);
    assert_eq!(before, after, "hiding a section changes nothing about the photo");
    // asking for the section (Window ▸ Edit Sections, ⌘2…) brings it back
    exec(&mut h, "section.effects", json!({}));
    assert!(headers(&mut h).contains(&"effects".to_string()));
}

#[test]
fn the_header_menu_moves_hides_shows_all_and_resets() {
    let mut h = edit_open();
    let r = h.request("ui.clickWidget", json!({"id": "section:color", "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let click = |h: &mut Headless, id: &str| {
        let r = h.request("ui.clickWidget", json!({"id": id}), T);
        assert_eq!(r["ok"], true, "{id}: {r}");
        h.step();
    };
    click(&mut h, "editSectionMenu:moveUp");
    assert_eq!(headers(&mut h)[..2], ["color", "light"]);
    h.request("ui.clickWidget", json!({"id": "section:light", "button": "right"}), T);
    h.step();
    click(&mut h, "editSectionMenu:hide");
    assert!(!headers(&mut h).contains(&"light".to_string()));
    assert!(h.app.ui.edit_sections.is_hidden(Light));
    h.request("ui.clickWidget", json!({"id": "section:color", "button": "right"}), T);
    h.step();
    click(&mut h, "editSectionMenu:showAll");
    assert!(!h.app.ui.edit_sections.any_hidden());
    assert_eq!(headers(&mut h)[..2], ["color", "light"], "show all keeps the order");
    h.request("ui.clickWidget", json!({"id": "section:color", "button": "right"}), T);
    h.step();
    click(&mut h, "editSectionMenu:reset");
    assert!(h.app.ui.edit_sections.is_default());
}

#[test]
fn the_arrangement_is_saved_with_the_ui_state_and_old_state_loads_unchanged() {
    let mut state = crate::state::UiState::default();
    state.edit_sections.move_before(Calibration, Some(Light));
    state.edit_sections.set_hidden(Detail, true);
    let saved = serde_json::to_value(&state).unwrap();
    assert_eq!(saved["editSections"][0], json!({"id": "calibration", "hidden": false}));
    let back: crate::state::UiState = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(back.edit_sections, state.edit_sections);
    // a state saved before sections could be arranged
    let mut old = saved;
    old.as_object_mut().unwrap().remove("editSections");
    let loaded: crate::state::UiState = serde_json::from_value(old).unwrap();
    assert!(loaded.edit_sections.is_default());
}
