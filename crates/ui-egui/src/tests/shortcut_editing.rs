//! Headless tests of shortcut editing (fork feature `feat/shortcut-editing`): a second shortcut
//! fires, a chord fires after its leader and is cancelled by Esc, a wrong key or the timeout, the
//! Assign Shortcut box saves a key and a chord, conflicts are shown, a right-click opens the box,
//! and menus show the first shortcut.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::shortcuts::shortcut_of;
use crate::state::{Dialog, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn press(h: &mut Headless, k: &str, cmd: bool, shift: bool) {
    let r = h.request("ui.key", json!({"key": k, "cmd": cmd, "shift": shift}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn key(h: &mut Headless, k: &str) {
    press(h, k, false, false);
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn set(h: &mut Headless, id: &str, slot: u32, shortcut: &str) {
    let r = h.app.run("app.setShortcut", json!({"id": id, "slot": slot, "shortcut": shortcut})).unwrap();
    assert!(r.is_object(), "{r}");
}

fn text(h: &Headless) -> String {
    h.painted_text().join("\n")
}

#[test]
fn the_second_shortcut_fires_and_the_first_still_does() {
    let mut h = demo();
    set(&mut h, "view.survey", 2, "Shift+K");
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"), "the first shortcut is unchanged");
    press(&mut h, "K", false, true);
    assert_eq!(h.app.ui.view, ViewMode::Survey);
    h.app.run("view.detail", json!({})).unwrap();
    h.step();
    key(&mut h, "N");
    assert_eq!(h.app.ui.view, ViewMode::Survey, "the first one works too");
}

#[test]
fn menus_and_the_palette_show_the_first_shortcut() {
    let mut h = demo();
    set(&mut h, "view.survey", 2, "Shift+K");
    let entry = |h: &Headless| crate::menus::menu_entries(&h.app).into_iter().find(|e| e.id == "view.survey").unwrap();
    assert_eq!(entry(&h).shortcut.as_deref(), Some("N"));
    set(&mut h, "view.survey", 1, "Cmd+K S");
    assert_eq!(entry(&h).shortcut.as_deref(), Some("Cmd+K S"));
    // the palette and the menus draw it through shortcut_text: both steps, a space between
    assert_eq!(crate::menubar::shortcut_text("Cmd+K S", true), "⌘K S");
    let rows = crate::panels::command_palette::entries(&h.app, true);
    let row = rows.iter().find(|e| e.id == "view.survey").unwrap();
    assert_eq!(row.shortcut.as_deref(), Some("⌘K S"));
    // the control channel lists both
    let listed = crate::control::all_commands(&h.app);
    let survey = listed.as_array().unwrap().iter().find(|c| c["id"] == "view.survey").unwrap();
    assert_eq!((survey["shortcut"].as_str(), survey["shortcut2"].as_str()), (Some("Cmd+K S"), Some("Shift+K")));
}

#[test]
fn a_chord_fires_its_command_after_the_leader_and_shows_a_hint() {
    let mut h = demo();
    set(&mut h, "view.survey", 1, "Cmd+K S");
    // the leader alone does nothing but wait (and the old Cmd+K command no longer has the key)
    press(&mut h, "K", true, false);
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert!(h.app.keybinds.pending.is_some());
    assert!(has(&h, "chordHint"), "the hint shows while the chord waits");
    let shown = text(&h);
    assert!(!shown.contains("was pressed"), "the leader shows as a key, not a sentence: {shown}");
    assert!(shown.contains("Esc"), "Esc is shown as the way out: {shown}");
    assert!(shown.contains("Survey"), "the hint lists what the next key does: {shown}");
    key(&mut h, "S");
    assert_eq!(h.app.ui.view, ViewMode::Survey);
    assert!(h.app.keybinds.pending.is_none());
    h.step();
    assert!(!has(&h, "chordHint"), "the hint goes away");
}

#[test]
fn esc_a_wrong_key_or_the_timeout_cancels_a_chord() {
    let mut h = demo();
    set(&mut h, "view.survey", 2, "Cmd+K S");
    // Esc
    press(&mut h, "K", true, false);
    key(&mut h, "Escape");
    assert!(h.app.keybinds.pending.is_none());
    key(&mut h, "S");
    assert_ne!(h.app.ui.view, ViewMode::Survey, "S alone is not the chord");
    // a key that finishes no chord: nothing runs, the key is used up
    h.app.run("view.detail", json!({})).unwrap();
    h.step();
    press(&mut h, "K", true, false);
    key(&mut h, "N");
    assert!(h.app.keybinds.pending.is_none());
    assert_eq!(h.app.ui.view, ViewMode::Detail, "N (Survey's own key) did not run inside the chord");
    // the timeout: a leader pressed long ago is forgotten, and the next key is an ordinary one
    press(&mut h, "K", true, false);
    assert!(h.app.keybinds.pending.is_some());
    if let Some(p) = h.app.keybinds.pending.as_mut() {
        p.since -= 100.0;
    }
    key(&mut h, "N");
    assert!(h.app.keybinds.pending.is_none());
    assert_eq!(h.app.ui.view, ViewMode::Survey, "after the timeout N is just Survey's key");
}

#[test]
fn the_assign_box_saves_a_key_with_enter_and_a_chord_too() {
    let mut h = demo();
    h.app.run("app.assignShortcut", json!({"id": "view.survey", "slot": 2})).unwrap();
    h.step();
    h.step();
    assert!(has(&h, "assignShortcut:keys"));
    assert_eq!(h.app.recording_shortcut.as_deref(), Some("view.survey"));
    assert!(text(&h).contains("Press the keys"), "{}", text(&h));
    // a key is taken, not run, and waits for Enter
    press(&mut h, "K", false, true);
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"));
    assert!(h.app.keybinds.capture.is_some());
    key(&mut h, "Enter");
    assert!(h.app.keybinds.capture.is_none() && h.app.recording_shortcut.is_none());
    assert_eq!(crate::keybinds::model::effective(&h.app.ui.settings.keymap, "view.survey", 1), Some("Shift+K"));
    // two keys in a row are a chord
    h.app.run("app.assignShortcut", json!({"id": "view.compare"})).unwrap();
    h.step();
    press(&mut h, "J", true, false);
    key(&mut h, "J");
    assert_eq!(h.app.keybinds.capture.as_ref().and_then(|c| c.text()).as_deref(), Some("Cmd+J J"));
    key(&mut h, "Enter");
    // Compare's first slot has its declared key, so the box opened on the second one
    assert_eq!(crate::keybinds::model::effective(&h.app.ui.settings.keymap, "view.compare", 1), Some("Cmd+J J"));
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.compare"), Some("Shift+C"));
    // and the chord works
    press(&mut h, "J", true, false);
    key(&mut h, "J");
    assert_eq!(h.app.ui.view, ViewMode::Compare);
}

#[test]
fn esc_closes_the_box_and_the_dialog_under_it_stays() {
    let mut h = demo();
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    h.app.run("app.assignShortcut", json!({"id": "view.survey", "slot": 2})).unwrap();
    h.step();
    h.step();
    press(&mut h, "K", false, true);
    key(&mut h, "Escape");
    assert!(h.app.keybinds.capture.is_none());
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Shortcuts)), "Esc closed only the box");
    assert_eq!(crate::keybinds::model::effective(&h.app.ui.settings.keymap, "view.survey", 1), None);
}

#[test]
fn the_box_names_the_command_that_has_the_key_and_saving_takes_it_over() {
    let mut h = demo();
    h.app.run("app.assignShortcut", json!({"id": "view.compare", "slot": 2})).unwrap();
    h.step();
    press(&mut h, "N", false, false);
    h.step();
    let shown = text(&h);
    assert!(shown.contains("Also used by Survey"), "{shown}");
    click(&mut h, "button:assignShortcut.save");
    assert!(h.app.keybinds.capture.is_none());
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), None, "Survey lost it");
    assert_eq!(crate::keybinds::model::effective(&h.app.ui.settings.keymap, "view.compare", 1), Some("N"));
    // Remove takes the shortcut of the chosen slot away
    h.app.run("app.assignShortcut", json!({"id": "view.compare", "slot": 2})).unwrap();
    h.step();
    h.step();
    click(&mut h, "button:assignShortcut.remove");
    assert!(h.app.keybinds.capture.is_none());
    assert_eq!(crate::keybinds::model::effective(&h.app.ui.settings.keymap, "view.compare", 1), None);
}

#[test]
fn the_window_edits_both_slots() {
    let mut h = demo();
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    click(&mut h, "field:shortcutsSearch");
    assert_eq!(h.request("ui.text", json!({"text": "survey"}), T)["ok"], true);
    h.step();
    h.step();
    for id in [
        "button:shortcut-view.survey",
        "button:shortcut2-view.survey",
        "button:shortcutClear-view.survey",
        "button:shortcutClear2-view.survey",
        "button:shortcutReset-view.survey",
    ] {
        assert!(has(&h, id), "{id}");
    }
    let shown = text(&h);
    for title in ["Action", "Shortcut 1", "Shortcut 2", "Where"] {
        assert!(shown.contains(title), "{title}: {shown}");
    }
    click(&mut h, "button:shortcut2-view.survey");
    assert_eq!(h.app.keybinds.capture.as_ref().map(|c| (c.id.as_str(), c.slot)), Some(("view.survey", 1)));
    press(&mut h, "K", false, true);
    key(&mut h, "Enter");
    assert_eq!(crate::keybinds::model::effective(&h.app.ui.settings.keymap, "view.survey", 1), Some("Shift+K"));
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Shortcuts)));
    // × removes the second one, ↺ restores everything
    click(&mut h, "button:shortcutClear2-view.survey");
    assert_eq!(crate::keybinds::model::effective(&h.app.ui.settings.keymap, "view.survey", 1), None);
    set(&mut h, "view.survey", 2, "Shift+K");
    set(&mut h, "view.survey", 1, "Shift+J");
    click(&mut h, "button:shortcutReset-view.survey");
    assert!(h.app.ui.settings.keymap.is_empty(), "{:?}", h.app.ui.settings.keymap);
}

#[test]
fn a_right_click_on_an_icon_offers_assign_shortcut() {
    let mut h = demo();
    let r = h.app.widgets.iter().find(|(w, _)| w == "icon:filmstrip").map(|(_, r)| *r).expect("the filmstrip icon");
    let right_click = |h: &mut Headless| {
        let r = h.request("ui.click", json!({"x": r.center().x, "y": r.center().y, "button": "right"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
    };
    right_click(&mut h);
    assert!(has(&h, "menu:assignShortcut"));
    click(&mut h, "menu:assignShortcut");
    h.step();
    assert_eq!(h.app.keybinds.capture.as_ref().map(|c| c.id.as_str()), Some("view.filmstrip"));
    assert!(has(&h, "assignShortcut:keys"));
}

#[test]
fn a_right_click_on_a_menu_item_opens_the_box() {
    let mut h = demo();
    click(&mut h, "menu:View");
    let item = h.app.widgets.iter().find(|(w, _)| w == "menuitem:view.survey").map(|(_, r)| *r).expect("the Survey menu item");
    let r = h.request("ui.click", json!({"x": item.center().x, "y": item.center().y, "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.keybinds.capture.as_ref().map(|c| c.id.as_str()), Some("view.survey"));
    assert_eq!(h.app.ui.view, ViewMode::Detail, "a right-click does not run the item");
}

#[test]
fn old_settings_load_and_the_second_slot_is_saved() {
    // a ui.json from before: single shortcuts only
    let old: crate::state::AppSettings = serde_json::from_str(r#"{"keymap": {"view.survey": "Shift+K", "view.detail": ""}}"#).unwrap();
    assert_eq!(shortcut_of(&old.keymap, "view.survey"), Some("Shift+K"));
    assert_eq!(shortcut_of(&old.keymap, "view.detail"), None);
    // a damaged one loads too: nothing in it can break startup
    let damaged: crate::state::AppSettings = serde_json::from_str(r#"{"keymap": {"view.survey#2": "Hyper+K", "x#2": "K"}}"#).unwrap();
    assert_eq!(crate::keybinds::model::effective(&damaged.keymap, "view.survey", 1), None);
    // both slots and a chord survive a save and load
    let mut h = demo();
    set(&mut h, "view.survey", 2, "Cmd+K S");
    set(&mut h, "view.survey", 1, "Shift+K");
    let saved = serde_json::to_string(&h.app.ui).unwrap();
    let back = serde_json::from_str::<crate::UiState>(&saved).unwrap();
    assert_eq!(crate::keybinds::model::effective_slots(&back.settings.keymap, "view.survey"), [Some("Shift+K"), Some("Cmd+K S")]);
}

#[test]
fn a_right_click_on_a_palette_row_opens_the_box() {
    let mut h = demo();
    h.app.run("view.commandPalette", json!({})).unwrap();
    h.step();
    h.step();
    assert_eq!(h.request("ui.text", json!({"text": "survey"}), T)["ok"], true);
    h.step();
    h.step();
    let row = h.app.widgets.iter().find(|(w, _)| w.starts_with("palette:row:view.survey")).map(|(_, r)| *r).expect("the Survey row");
    let r = h.request("ui.click", json!({"x": row.center().x, "y": row.center().y, "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    h.step();
    assert!(h.app.ui.palette.is_none(), "the palette made way");
    assert_eq!(h.app.keybinds.capture.as_ref().map(|c| c.id.as_str()), Some("view.survey"));
}

fn key_event(key: egui::Key, repeat: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat, modifiers }
}

#[test]
fn holding_the_leader_does_not_cancel_the_chord() {
    let mut h = demo();
    set(&mut h, "view.survey", 2, "Cmd+K S");
    // ⌘K goes down and stays down (egui turns a key's later events into repeats while it is held)
    h.app.synthetic.push(key_event(egui::Key::K, false, egui::Modifiers::COMMAND));
    h.step();
    h.step();
    assert!(h.app.keybinds.pending.is_some());
    for _ in 0..3 {
        h.app.synthetic.push(key_event(egui::Key::K, true, egui::Modifiers::COMMAND));
        h.step();
        h.step();
    }
    assert!(h.app.keybinds.pending.is_some(), "the repeats of the leader kept the chord waiting");
    assert!(h.app.ui.toast.is_none(), "no 'no shortcut continues' toast: {:?}", h.app.ui.toast);
    // let go, then the second key
    h.app.synthetic.push(egui::Event::Key {
        key: egui::Key::K,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    });
    h.step();
    h.step();
    key(&mut h, "S");
    assert_eq!(h.app.ui.view, ViewMode::Survey);
}

#[test]
fn cmd_may_stay_down_for_the_second_key() {
    let mut h = demo();
    set(&mut h, "view.survey", 2, "Cmd+K S");
    // ⌘K, then ⌘ still held for S
    press(&mut h, "K", true, false);
    press(&mut h, "S", true, false);
    assert_eq!(h.app.ui.view, ViewMode::Survey, "⌘K then ⌘S works for the chord ⌘K S");
    assert!(h.app.keybinds.pending.is_none());
    // a chord that names ⌘ on its second key takes it held, and not released
    h.app.run("view.detail", json!({})).unwrap();
    h.step();
    set(&mut h, "view.compare", 2, "Cmd+K Cmd+J");
    press(&mut h, "K", true, false);
    key(&mut h, "J");
    assert_eq!(h.app.ui.view, ViewMode::Detail, "J without ⌘ is not ⌘K ⌘J");
    press(&mut h, "K", true, false);
    press(&mut h, "J", true, false);
    assert_eq!(h.app.ui.view, ViewMode::Compare);
}

#[test]
fn a_field_taking_the_keyboard_or_a_dialog_forgets_the_chord() {
    let mut h = demo();
    set(&mut h, "view.survey", 2, "Cmd+K S");
    press(&mut h, "K", true, false);
    assert!(h.app.keybinds.pending.is_some());
    click(&mut h, "field:search");
    assert!(h.app.keybinds.pending.is_none(), "the search field took the keyboard");
    key(&mut h, "S");
    assert_ne!(h.app.ui.view, ViewMode::Survey, "S went into the field");
    // a dialog opening does the same
    key(&mut h, "Escape");
    press(&mut h, "K", true, false);
    assert!(h.app.keybinds.pending.is_some());
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    assert!(h.app.keybinds.pending.is_none());
}

#[test]
fn a_chord_may_not_use_an_app_menu_key() {
    let mut h = demo();
    let e = h.app.run("app.setShortcut", json!({"id": "view.survey", "slot": 2, "shortcut": "F9 Cmd+Q"})).unwrap_err();
    assert!(e.contains("reserved"), "{e}");
}

#[test]
fn a_chord_shows_as_text_in_the_native_menu_item() {
    let (label, key) = crate::menubar::native_item_presentation(
        &crate::shortcuts::Keymap::new(),
        "view.survey",
        &serde_json::Value::Null,
        "Survey",
        Some("Cmd+K S"),
    );
    assert!(label.ends_with("(⌘K S)"), "{label}");
    assert_eq!(key, None, "a chord is no key equivalent");
    let (label, key) =
        crate::menubar::native_item_presentation(&crate::shortcuts::Keymap::new(), "view.survey", &serde_json::Value::Null, "Survey", Some("N"));
    assert!(!label.contains('('), "{label}");
    assert_eq!(key, Some("N"));
}

#[test]
fn the_old_fixed_second_keys_are_editable_shortcuts() {
    let mut h = demo();
    let slots = |h: &Headless, id: &str| crate::keybinds::model::effective_slots(&h.app.ui.settings.keymap, id).map(|s| s.map(str::to_string));
    assert_eq!(slots(&h, "view.filmstrip")[1].as_deref(), Some("F6"));
    // F6 still works, as it did
    let before = h.app.ui.filmstrip;
    key(&mut h, "F6");
    assert_ne!(h.app.ui.filmstrip, before);
    // moving it to another key: F6 stops, the new key works
    set(&mut h, "view.filmstrip", 2, "Shift+F9");
    let before = h.app.ui.filmstrip;
    key(&mut h, "F6");
    assert_eq!(h.app.ui.filmstrip, before, "F6 no longer does it");
    press(&mut h, "F9", false, true);
    assert_ne!(h.app.ui.filmstrip, before);
    // the window lists it under Shortcut 2 and no longer under Fixed Keys
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    assert!(has(&h, "button:shortcut2-view.thumbSizeUp"));
    assert!(!crate::shortcuts::ALIASES.iter().any(|(_, id, _)| *id == "view.filmstrip"));
}

#[test]
fn the_box_ignores_enter_before_a_key() {
    let mut h = demo();
    h.app.run("app.assignShortcut", json!({"id": "view.survey", "slot": 2})).unwrap();
    h.step();
    key(&mut h, "Enter");
    assert!(h.app.keybinds.capture.is_some(), "Enter with no keys does nothing");
    assert!(h.app.keybinds.capture.as_ref().is_some_and(|c| c.steps.is_empty()));
}
