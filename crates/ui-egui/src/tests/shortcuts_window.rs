//! Headless tests of the Keyboard Shortcuts window (fork feature `feat/49-shortcuts-window`): VS
//! Code-style recording in the cell (a chord, Enter accepts, Esc cancels, the clash warning shows
//! before Enter), Remove Shortcut on ⌫ / Delete and the right-click menu, Add Shortcut, reset to
//! default, search by modifier words and symbols, and the size the window opens at. A recorded
//! shortcut is saved exactly as `app.setShortcut` saves it.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::keybinds::model::effective;
use crate::shortcuts::shortcut_of;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo_with(settings: impl FnOnce(&mut LightcraftApp)) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    settings(&mut app);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn demo() -> Headless {
    demo_with(|_| {})
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

fn click_at(h: &mut Headless, id: &str, button: &str, fx: f32) {
    let r = h.request("ui.clickWidget", json!({"id": id, "button": button, "fx": fx}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn click(h: &mut Headless, id: &str) {
    click_at(h, id, "left", 0.5);
}

/// A click on the left of a row, where its name is.
fn pick(h: &mut Headless, id: &str) {
    click_at(h, &format!("row:shortcut-{id}"), "left", 0.03);
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn text(h: &Headless) -> String {
    h.painted_text().join("\n")
}

/// The window open, narrowed to `search`.
fn open(h: &mut Headless, search: &str) {
    h.app.run("app.shortcuts", json!({})).unwrap();
    // the window lays itself out over its first frames
    for _ in 0..6 {
        h.step();
    }
    click(h, "field:shortcutsSearch");
    assert_eq!(h.request("ui.text", json!({"text": search}), T)["ok"], true);
    h.step();
    h.step();
}

fn slot(h: &Headless, id: &str, slot: usize) -> Option<String> {
    effective(&h.app.ui.settings.keymap, id, slot).map(str::to_string)
}

#[test]
fn clicking_a_shortcut_records_in_the_cell_without_a_box() {
    let mut h = demo();
    open(&mut h, "compare");
    click(&mut h, "button:shortcut2-view.compare");
    assert_eq!(h.app.keybinds.capture.as_ref().map(|c| (c.id.as_str(), c.slot, c.inline)), Some(("view.compare", 1, true)));
    assert_eq!(h.app.recording_shortcut.as_deref(), Some("view.compare"));
    assert!(!has(&h, "assignShortcut:keys"), "the recording shows in the window, not in the Assign Shortcut box");
    assert!(text(&h).contains("Press the keys"), "{}", text(&h));
    // each combination pressed shows in the cell, and nothing is saved before Enter
    press(&mut h, "Q", false, true);
    assert!(text(&h).contains("Q"), "{}", text(&h));
    assert_eq!(slot(&h, "view.compare", 1), None);
    key(&mut h, "Enter");
    assert_eq!(slot(&h, "view.compare", 1).as_deref(), Some("Shift+Q"));
    assert!(h.app.keybinds.capture.is_none() && h.app.recording_shortcut.is_none());
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Shortcuts)));
}

#[test]
fn two_combinations_make_a_chord_and_a_third_starts_over() {
    let mut h = demo();
    open(&mut h, "compare");
    click(&mut h, "button:shortcut2-view.compare");
    press(&mut h, "J", true, false);
    key(&mut h, "J");
    assert_eq!(h.app.keybinds.capture.as_ref().and_then(|c| c.text()).as_deref(), Some("Cmd+J J"));
    key(&mut h, "K");
    assert_eq!(h.app.keybinds.capture.as_ref().and_then(|c| c.text()).as_deref(), Some("K"), "a third key starts over");
    key(&mut h, "Escape");
    click(&mut h, "button:shortcut2-view.compare");
    press(&mut h, "J", true, false);
    key(&mut h, "J");
    key(&mut h, "Enter");
    assert_eq!(slot(&h, "view.compare", 1).as_deref(), Some("Cmd+J J"));
}

#[test]
fn esc_cancels_the_recording_and_keeps_the_window_open() {
    let mut h = demo();
    open(&mut h, "compare");
    click(&mut h, "button:shortcut2-view.compare");
    press(&mut h, "Q", false, true);
    key(&mut h, "Escape");
    assert!(h.app.keybinds.capture.is_none() && h.app.recording_shortcut.is_none());
    assert_eq!(slot(&h, "view.compare", 1), None, "nothing was saved");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Shortcuts)), "Esc closed only the recording");
    key(&mut h, "Escape");
    assert!(h.app.ui.dialog.is_none(), "the next Esc closes the window");
}

#[test]
fn the_clash_warning_shows_before_enter_and_enter_takes_the_key() {
    let mut h = demo();
    open(&mut h, "compare");
    click(&mut h, "button:shortcut2-view.compare");
    // N is Survey's key
    key(&mut h, "N");
    let shown = text(&h);
    assert!(shown.contains("Also used by Survey"), "{shown}");
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"), "nothing changed before Enter");
    key(&mut h, "Enter");
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), None, "Survey lost it");
    assert_eq!(slot(&h, "view.compare", 1).as_deref(), Some("N"));
    assert!(!text(&h).contains("Also used by Survey"), "the warning goes with the recording");
}

#[test]
fn a_recorded_shortcut_is_saved_exactly_as_the_command_saves_it() {
    // through the window
    let mut h = demo();
    open(&mut h, "compare");
    click(&mut h, "button:shortcut2-view.compare");
    press(&mut h, "K", true, true);
    key(&mut h, "Enter");
    click(&mut h, "button:shortcut-view.compare");
    press(&mut h, "J", true, false);
    key(&mut h, "J");
    key(&mut h, "Enter");
    // through the command
    let mut other = demo();
    other.app.run("app.setShortcut", json!({"id": "view.compare", "slot": 2, "shortcut": "Cmd+Shift+K"})).unwrap();
    other.app.run("app.setShortcut", json!({"id": "view.compare", "slot": 1, "shortcut": "Cmd+J J"})).unwrap();
    assert_eq!(h.app.ui.settings.keymap, other.app.ui.settings.keymap, "{:?}", h.app.ui.settings.keymap);
    assert_eq!(slot(&h, "view.compare", 1).as_deref(), Some("Cmd+Shift+K"));
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.compare"), Some("Cmd+J J"));
    // and it survives a save and load, and works
    let back = serde_json::from_str::<crate::UiState>(&serde_json::to_string(&h.app.ui).unwrap()).unwrap();
    assert_eq!(back.settings.keymap, h.app.ui.settings.keymap);
}

#[test]
fn backspace_and_delete_remove_the_picked_shortcut() {
    let mut h = demo();
    open(&mut h, "survey");
    // typing in the search must not remove anything
    click(&mut h, "field:shortcutsSearch");
    key(&mut h, "Backspace");
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"));
    assert_eq!(h.request("ui.text", json!({"text": "survey"}), T)["ok"], true);
    h.step();
    h.step();
    pick(&mut h, "view.survey");
    assert_eq!(h.app.keybinds.selected, Some(("view.survey", 0)));
    key(&mut h, "Backspace");
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), None, "⌫ removed Shortcut 1");
    // Delete does it too, for the slot clicked
    h.app.run("app.setShortcut", json!({"id": "view.survey", "slot": 2, "shortcut": "Shift+Q"})).unwrap();
    h.step();
    click(&mut h, "button:shortcut2-view.survey");
    key(&mut h, "Escape");
    assert_eq!(h.app.keybinds.selected, Some(("view.survey", 1)));
    key(&mut h, "Delete");
    assert_eq!(slot(&h, "view.survey", 1), None, "Delete removed Shortcut 2");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Shortcuts)));
}

#[test]
fn while_recording_backspace_is_a_key_to_record_not_a_removal() {
    let mut h = demo();
    open(&mut h, "survey");
    pick(&mut h, "view.survey");
    click(&mut h, "button:shortcut2-view.survey");
    key(&mut h, "Backspace");
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"), "Shortcut 1 is still there");
    assert_eq!(h.app.keybinds.capture.as_ref().and_then(|c| c.text()).as_deref(), Some("Delete"));
}

#[test]
fn the_right_click_menu_removes_a_shortcut_and_reaches_the_condition() {
    let mut h = demo();
    open(&mut h, "survey");
    click_at(&mut h, "button:shortcut-view.survey", "right", 0.5);
    assert!(has(&h, "menu:shortcutRemove-0"), "the shortcut cell's menu offers Remove Shortcut");
    click(&mut h, "menu:shortcutRemove-0");
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), None);
    // a right-click on the row's name: Add Shortcut, Edit Condition…, Remove Shortcut, Reset
    click_at(&mut h, "row:shortcut-view.survey", "right", 0.03);
    for id in ["menu:shortcutAdd", "menu:shortcutCondition", "menu:shortcutRemove", "menu:shortcutReset"] {
        assert!(has(&h, id), "{id}");
    }
    click(&mut h, "menu:shortcutCondition");
    assert_eq!(h.app.keybinds.when_edit.as_ref().map(|e| e.id), Some("view.survey"), "the condition opens for typing");
}

#[test]
fn add_shortcut_records_into_the_first_free_slot_of_the_picked_row() {
    let mut h = demo();
    open(&mut h, "survey");
    assert!(has(&h, "button:shortcutsAdd"));
    pick(&mut h, "view.survey");
    click(&mut h, "button:shortcutsAdd");
    assert_eq!(h.app.keybinds.capture.as_ref().map(|c| (c.id.as_str(), c.slot)), Some(("view.survey", 1)), "Shortcut 1 is taken, so Shortcut 2");
    press(&mut h, "Q", false, true);
    key(&mut h, "Enter");
    assert_eq!(slot(&h, "view.survey", 1).as_deref(), Some("Shift+Q"));
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"), "the first one stays");
}

#[test]
fn the_reset_icon_shows_on_a_changed_row_and_restores_the_declared_shortcut() {
    let mut h = demo();
    open(&mut h, "survey");
    assert!(!has(&h, "button:shortcutReset-view.survey"), "an unchanged row has no reset icon");
    h.app.run("app.setShortcut", json!({"id": "view.survey", "shortcut": "Shift+J"})).unwrap();
    h.step();
    h.step();
    assert!(has(&h, "button:shortcutReset-view.survey"));
    click(&mut h, "button:shortcutReset-view.survey");
    assert!(h.app.ui.settings.keymap.is_empty(), "{:?}", h.app.ui.settings.keymap);
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"));
}

#[test]
fn the_buttons_that_went_are_gone() {
    let mut h = demo();
    open(&mut h, "survey");
    h.app.run("app.setShortcut", json!({"id": "view.survey", "slot": 2, "shortcut": "Shift+Q"})).unwrap();
    h.step();
    h.step();
    for quiet in ["button:shortcutClear-view.survey", "button:shortcutClear2-view.survey"] {
        assert!(!has(&h, quiet), "{quiet}: the trash icon shows only on the cell the pointer is over");
    }
    let shown = text(&h);
    for gone in ["Delete Shortcut", "Use Default", "Add Chord", "modified", "Click a shortcut to change it"] {
        assert!(!shown.contains(gone), "{gone}: {shown}");
    }
    for want in ["Add Shortcut", "Reset All to Default", "Show commands without a shortcut"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
}

#[test]
fn the_window_opens_at_about_1000_by_640_and_keeps_its_size() {
    let mut h = demo();
    h.app.run("app.shortcuts", json!({})).unwrap();
    for _ in 0..6 {
        h.step();
    }
    let rect = |h: &Headless| h.app.widgets.iter().find(|(w, _)| w == "dialog:window").map(|(_, r)| *r).expect("the window");
    let first = rect(&h);
    assert!((first.width() - 1000.0).abs() < 3.0 && (first.height() - 640.0).abs() < 3.0, "{first:?}");
    for _ in 0..12 {
        h.step();
    }
    assert_eq!(rect(&h).size(), first.size(), "the window does not creep from frame to frame");
    // the Close button and the footer stay inside it
    assert!(has(&h, "button:shortcutsResetAll"));
    // a remembered size is where it opens next time
    let mut h = demo_with(|app| app.ui.settings.shortcuts_window_size = Some([1200.0, 700.0]));
    h.app.run("app.shortcuts", json!({})).unwrap();
    for _ in 0..8 {
        h.step();
    }
    let second = rect(&h);
    assert!((second.width() - 1200.0).abs() < 3.0 && (second.height() - 700.0).abs() < 3.0, "{second:?}");
    // closing and opening again keeps it, and nothing was written for a size nobody dragged
    key(&mut h, "Escape");
    h.app.run("app.shortcuts", json!({})).unwrap();
    for _ in 0..8 {
        h.step();
    }
    assert_eq!(rect(&h).size(), second.size());
    assert_eq!(h.app.ui.settings.shortcuts_window_size, Some([1200.0, 700.0]));
    // it survives a save and load of the settings
    let back = serde_json::from_str::<crate::UiState>(&serde_json::to_string(&h.app.ui).unwrap()).unwrap();
    assert_eq!(back.settings.shortcuts_window_size, Some([1200.0, 700.0]));
    // an old file without the key opens at the default
    let old: crate::state::AppSettings = serde_json::from_str(r#"{"keymap": {}}"#).unwrap();
    assert_eq!(old.shortcuts_window_size, None);
    let damaged: crate::state::AppSettings = serde_json::from_str(r#"{"shortcutsWindowSize": "big"}"#).unwrap();
    assert_eq!(damaged.shortcuts_window_size, None);
}

#[test]
fn a_bare_trash_icon_shows_on_the_hovered_cell_and_removes_its_shortcut() {
    let mut h = demo();
    open(&mut h, "survey");
    h.app.run("app.setShortcut", json!({"id": "view.survey", "slot": 2, "shortcut": "Shift+Q"})).unwrap();
    h.step();
    h.step();
    let r = h.request("ui.hoverWidget", json!({"id": "button:shortcut2-view.survey"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(has(&h, "button:shortcutClear2-view.survey"));
    assert!(!has(&h, "button:shortcutClear-view.survey"), "only the hovered cell shows it");
    click(&mut h, "button:shortcutClear2-view.survey");
    assert_eq!(slot(&h, "view.survey", 1), None);
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"), "the other shortcut stays");
    assert!(h.app.keybinds.capture.is_none(), "the click on the icon did not start a recording");
}
