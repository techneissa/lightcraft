use std::time::Duration;

use serde_json::{Value, json};

use super::EditSection::{Calibration, Color, Detail, Effects, LensBlur, Light, Optics};
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
    assert_eq!(order(&layout), [Light, Color, Effects, Detail, Optics, LensBlur, Calibration]);
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
    assert_eq!(order(&layout), [Detail, Light, Color, Effects, Optics, LensBlur, Calibration]);
    assert!(layout.is_hidden(Detail), "the first entry for a section counts");
    for saved in [json!(null), json!("light"), json!(3), json!({"light": true}), json!([])] {
        assert!(read(saved).is_default());
    }
    // every section hidden is not an arrangement the panel can be left in
    let all_hidden =
        read(json!(["light", "color", "effects", "detail", "optics", "lensBlur", "calibration"].map(|id| json!({"id": id, "hidden": true}))));
    assert_eq!(all_hidden.shown(), [Light]);
}

#[test]
fn moving_steps_over_hidden_sections_and_stops_at_the_ends() {
    let mut layout = EditSectionLayout::default();
    layout.set_hidden(Effects, true);
    assert!(layout.move_by(Detail, true), "Detail goes above Color, past the hidden Effects");
    assert_eq!(layout.shown(), [Light, Detail, Color, Optics, LensBlur, Calibration]);
    assert!(!layout.can_move(Light, true) && !layout.can_move(Calibration, false));
    assert!(!layout.move_by(Light, true) && !layout.move_by(Effects, true), "the first and a hidden section stay");
    assert!(layout.move_before(Light, None));
    assert_eq!(layout.shown().last(), Some(&Light));
}

#[test]
fn the_last_shown_section_cannot_be_hidden() {
    let mut layout = EditSectionLayout::default();
    for s in [Color, Effects, Detail, Optics, LensBlur, Calibration] {
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
    assert_eq!(order(&next), [Optics, Light, Color, Effects, Detail, LensBlur, Calibration]);
    assert_eq!(next.shown(), [Optics, Light, Color, Detail, LensBlur, Calibration]);
    // a key left out keeps what is there
    assert_eq!(next.with_request(None, Some(&json!([]))).unwrap().shown().len(), 7);
    assert_eq!(order(&next.with_request(None, Some(&json!(["color"]))).unwrap()), order(&next));
    for (o, h, says) in [
        (json!("light"), json!(null), "must be a list"),
        (json!(["light", "light"]), json!(null), "listed twice"),
        (json!(["lite"]), json!(null), "unknown section `lite`"),
        (json!(null), json!([3]), "names no section"),
        (json!(null), json!(["light", "color", "effects", "detail", "optics", "lensBlur", "calibration"]), "at least one"),
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
    assert_eq!(headers(&mut h), ["light", "color", "effects", "detail", "optics", "lensBlur", "calibration"]);
    exec(&mut h, "view.editSections", json!({"order": ["optics", "light"], "hidden": ["effects", "calibration"]}));
    assert_eq!(headers(&mut h), ["optics", "light", "color", "detail", "lensBlur"]);
    exec(&mut h, "view.editSectionsReset", json!({}));
    assert_eq!(headers(&mut h), ["light", "color", "effects", "detail", "optics", "lensBlur", "calibration"]);
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
fn asking_for_a_hidden_open_section_brings_it_back_open_and_a_shown_one_still_toggles() {
    let mut h = edit_open();
    assert!(h.app.ui.section_open("effects"));
    exec(&mut h, "view.editSectionShow", json!({"section": "effects", "show": false}));
    exec(&mut h, "section.effects", json!({}));
    assert!(!h.app.ui.edit_sections.is_hidden(Effects));
    assert!(h.app.ui.section_open("effects"), "came back folded");
    // a visible section keeps the toggle: asking for an open one folds it
    exec(&mut h, "section.effects", json!({}));
    assert!(!h.app.ui.section_open("effects"));
    // hidden while folded: it comes back and opens
    exec(&mut h, "view.editSectionShow", json!({"section": "effects", "show": false}));
    exec(&mut h, "section.effects", json!({}));
    assert!(h.app.ui.section_open("effects"));
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

fn header_rect(h: &Headless, section: EditSection) -> egui::Rect {
    let id = format!("section:{}", section.id());
    h.app.widgets.iter().find(|(w, _)| *w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

fn press_and_move(h: &mut Headless, from: egui::Pos2, to: egui::Pos2) {
    h.app.synthetic.push(egui::Event::PointerMoved(from));
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    for i in 1..=8 {
        h.app.synthetic.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.step();
}

fn release(h: &mut Headless, at: egui::Pos2) {
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    for _ in 0..3 {
        h.step();
    }
}

/// Given the Edit panel, when a section's heading is dragged above another and released, then the
/// section is there, a line showed where it would go, and the new order is in the saved UI state;
/// a click on a heading still folds its section, and a drag given up moves nothing.
#[test]
fn dragging_a_heading_moves_its_section() {
    let mut h = edit_open();
    // every section folded, so every heading is on screen
    h.app.ui.open_sections.clear();
    h.step();
    h.step();
    // Optics, dragged to just above Color
    let color_top = header_rect(&h, Color).top();
    let (from, to) = (header_rect(&h, Optics).center(), egui::pos2(header_rect(&h, Optics).center().x - 40.0, color_top - 4.0));
    // (the header's centre is left of its Solo and eye buttons, which keep their own presses)
    press_and_move(&mut h, from, to);
    assert_eq!(h.app.ui.dragging_edit_section, Some(Optics));
    let line = h.app.widgets.iter().find(|(w, _)| w == "editSectionDrop").map(|(_, r)| *r).expect("the drop line is drawn");
    assert!((line.center().y - color_top).abs() < 3.0, "the line is at the top of Color: {line:?} / {color_top}");
    release(&mut h, to);
    assert_eq!(h.app.ui.dragging_edit_section, None);
    assert_eq!(headers(&mut h), ["light", "optics", "color", "effects", "detail", "lensBlur", "calibration"]);
    assert_eq!(order(&h.app.ui.edit_sections), [Light, Optics, Color, Effects, Detail, LensBlur, Calibration]);
    assert!(!h.app.ui.section_open("optics"), "a drag does not open the section");
    // saved
    let back = serde_json::from_value::<crate::state::UiState>(serde_json::to_value(&h.app.ui).unwrap()).unwrap();
    assert_eq!(back.edit_sections, h.app.ui.edit_sections);
    // Esc leaves the order
    let (from, to) = (header_rect(&h, Calibration).center(), egui::pos2(from.x, header_rect(&h, Light).top() - 4.0));
    press_and_move(&mut h, from, to);
    assert_eq!(h.app.ui.dragging_edit_section, Some(Calibration));
    h.app.synthetic.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    release(&mut h, to);
    assert_eq!(headers(&mut h), ["light", "optics", "color", "effects", "detail", "lensBlur", "calibration"]);
    // a click still opens
    assert!(!h.app.ui.section_open("detail"));
    let r = h.request("ui.clickWidget", json!({"id": "section:detail", "fx": 0.3}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.ui.section_open("detail"), "a click opens the section");
    assert_eq!(order(&h.app.ui.edit_sections), [Light, Optics, Color, Effects, Detail, LensBlur, Calibration]);
}
