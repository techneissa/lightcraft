//! Headless tests of shortcut conditions (fork feature `feat/shortcut-when`): the When column
//! edits a condition by typing (autocomplete, negations) and with the builder, nothing invalid is
//! saved, a binding fires only where its condition holds, chords and the keys of dialogs follow
//! it, and clashes are shown with the reason. With `LIGHTCRAFT_TEST_SHOTS` set, `when_pictures`
//! also writes the screenshots.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::keybinds::model::conditions;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1500.0, 950.0], 1.0);
    let r = h.request("ui.set", json!({"view": "photoGrid"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn key(h: &mut Headless, k: &str) {
    let r = h.request("ui.key", json!({"key": k}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn type_text(h: &mut Headless, text: &str) {
    assert_eq!(h.request("ui.text", json!({"text": text}), T)["ok"], true);
    h.step();
    h.step();
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

fn shown(h: &Headless) -> String {
    h.painted_text().join("\n")
}

fn when_of(h: &Headless, id: &str) -> Option<String> {
    conditions::text(&h.app.ui.settings.keymap, id)
}

fn set(h: &mut Headless, p: serde_json::Value) -> serde_json::Value {
    h.app.run("app.setShortcut", p).unwrap()
}

/// Open the Keyboard Shortcuts window on Survey's row.
fn open_window(h: &mut Headless) {
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    click(h, "field:shortcutsSearch");
    type_text(h, "survey");
}

fn close_window(h: &mut Headless) {
    h.app.ui.dialog = None;
    h.step();
    h.step();
}

fn shot(h: &mut Headless, name: &str) {
    if let Some(dir) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        let path = std::path::Path::new(&dir).join(name);
        let r = h.request("ui.screenshot", json!({"path": path.to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

fn edit_when(h: &mut Headless, id: &str, text: &str) {
    click(h, &format!("button:shortcutWhen-{id}"));
    assert!(h.app.keybinds.when_edit.is_some(), "the click starts the edit");
    type_text(h, text);
    key(h, "Enter");
}

#[test]
fn typing_a_condition_limits_the_shortcut() {
    let mut h = demo();
    set(&mut h, json!({"id": "view.survey", "shortcut": "K"}));
    open_window(&mut h);
    assert!(shown(&h).contains("Always"), "{}", shown(&h));
    edit_when(&mut h, "view.survey", "view == detail");
    assert!(h.app.keybinds.when_edit.is_none(), "Enter saves and ends the edit");
    assert_eq!(when_of(&h, "view.survey").as_deref(), Some("view == detail"));
    assert!(shown(&h).contains("Loupe View"), "the chip: {}", shown(&h));
    close_window(&mut h);
    // in the grid the key does nothing; in the loupe it opens Survey
    key(&mut h, "K");
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
    h.app.run("view.detail", json!({})).unwrap();
    h.step();
    key(&mut h, "K");
    assert_eq!(h.app.ui.view, ViewMode::Survey);
}

#[test]
fn a_negated_condition_takes_effect() {
    let mut h = demo();
    set(&mut h, json!({"id": "view.survey", "shortcut": "K"}));
    open_window(&mut h);
    edit_when(&mut h, "view.survey", "!(view == detail)");
    assert_eq!(when_of(&h, "view.survey").as_deref(), Some("!(view == detail)"));
    assert!(shown(&h).contains("Not Loupe View"), "{}", shown(&h));
    close_window(&mut h);
    h.app.run("view.detail", json!({})).unwrap();
    h.step();
    key(&mut h, "K");
    assert_eq!(h.app.ui.view, ViewMode::Detail, "not in the loupe");
    h.app.ui.view = ViewMode::PhotoGrid;
    h.step();
    key(&mut h, "K");
    assert_eq!(h.app.ui.view, ViewMode::Survey, "everywhere else");
}

#[test]
fn an_invalid_condition_is_not_saved_and_shows_why() {
    let mut h = demo();
    open_window(&mut h);
    click(&mut h, "button:shortcutWhen-view.survey");
    type_text(&mut h, "viw == detail");
    assert!(shown(&h).contains("Unknown key “viw”, did you mean “view”?"), "{}", shown(&h));
    key(&mut h, "Enter");
    assert_eq!(when_of(&h, "view.survey"), None, "nothing saved");
    assert!(h.app.keybinds.when_edit.is_some(), "the box stays open for the fix");
    // Esc gives up: the window stays, the condition is untouched
    key(&mut h, "Escape");
    assert!(h.app.keybinds.when_edit.is_none());
    assert!(h.app.ui.dialog.is_some(), "Esc ended the edit, not the window");
    assert_eq!(when_of(&h, "view.survey"), None);
}

#[test]
fn an_unfinished_condition_is_red_only_once_you_try_to_save_it() {
    let mut h = demo();
    open_window(&mut h);
    click(&mut h, "button:shortcutWhen-view.survey");
    type_text(&mut h, "view == detail &&");
    assert!(!shown(&h).contains("stops too early"), "still typing: {}", shown(&h));
    key(&mut h, "Escape");
    click(&mut h, "button:shortcutWhen-view.survey");
    type_text(&mut h, "view ==");
    key(&mut h, "Enter");
    assert!(shown(&h).contains("A value of view should follow"), "{}", shown(&h));
    assert_eq!(when_of(&h, "view.survey"), None);
}

#[test]
fn autocomplete_lists_keys_with_descriptions_and_tab_takes_one() {
    let mut h = demo();
    open_window(&mut h);
    click(&mut h, "button:shortcutWhen-view.survey");
    type_text(&mut h, "te");
    let text = shown(&h);
    assert!(text.contains("textFocus") && text.contains("!textFocus"), "keys and their negated forms: {text}");
    assert!(text.contains("A text box has the keyboard"), "with a description: {text}");
    assert!(has(&h, "menu:whenSuggestion-0"));
    // clear the word and walk the grammar with Tab: key, operator, value, and
    key(&mut h, "Escape");
    click(&mut h, "button:shortcutWhen-view.survey");
    type_text(&mut h, "vi");
    key(&mut h, "Tab");
    assert_eq!(h.app.keybinds.when_edit.as_ref().map(|e| e.text.as_str()), Some("view "));
    key(&mut h, "Tab");
    assert_eq!(h.app.keybinds.when_edit.as_ref().map(|e| e.text.as_str()), Some("view == "));
    // the list now offers the values (grid, square, detail…): two arrows down, Tab takes the third
    key(&mut h, "ArrowDown");
    key(&mut h, "ArrowDown");
    key(&mut h, "Tab");
    assert_eq!(h.app.keybinds.when_edit.as_ref().map(|e| e.text.as_str()), Some("view == detail "));
    key(&mut h, "Tab");
    assert_eq!(h.app.keybinds.when_edit.as_ref().map(|e| e.text.as_str()), Some("view == detail && "));
}

#[test]
fn arrows_and_enter_take_the_highlighted_suggestion() {
    let mut h = demo();
    open_window(&mut h);
    click(&mut h, "button:shortcutWhen-view.survey");
    type_text(&mut h, "tool != ");
    key(&mut h, "ArrowDown");
    key(&mut h, "ArrowDown");
    key(&mut h, "Enter");
    assert_eq!(
        h.app.keybinds.when_edit.as_ref().map(|e| e.text.as_str()),
        Some("tool != crop "),
        "Enter took the highlighted value, it did not save"
    );
    assert_eq!(when_of(&h, "view.survey"), None);
    key(&mut h, "Enter");
    assert_eq!(when_of(&h, "view.survey").as_deref(), Some("tool != crop"), "a second Enter saves");
}

#[test]
fn the_builder_adds_conditions_with_is_and_is_not_for_every_key() {
    let mut h = demo();
    open_window(&mut h);
    click(&mut h, "button:shortcutWhenAdd-view.survey");
    for k in ["view", "textFocus", "tool"] {
        assert!(has(&h, &format!("menu:whenKey-{k}")), "{k}");
    }
    // an on / off key: Is Not Typing
    click(&mut h, "menu:whenKey-textFocus");
    assert!(has(&h, "menu:whenIs") && has(&h, "menu:whenIsNot"), "Is and Is Not for an on / off key too");
    click(&mut h, "menu:whenIsNot");
    click(&mut h, "menu:whenValue-textFocus");
    assert_eq!(when_of(&h, "view.survey").as_deref(), Some("!textFocus"));
    // an enumeration key: Is Not Loupe View, added with &&
    click(&mut h, "button:shortcutWhenAdd-view.survey");
    click(&mut h, "menu:whenKey-view");
    click(&mut h, "menu:whenIsNot");
    click(&mut h, "menu:whenValue-detail");
    assert_eq!(when_of(&h, "view.survey").as_deref(), Some("!textFocus && view != detail"));
    let text = shown(&h);
    assert!(text.contains("Not Typing") && text.contains("Not Loupe View"), "{text}");
    // and a plain one
    click(&mut h, "button:shortcutWhenAdd-view.survey");
    click(&mut h, "menu:whenKey-tool");
    click(&mut h, "menu:whenValue-crop");
    assert_eq!(when_of(&h, "view.survey").as_deref(), Some("!textFocus && view != detail && tool == crop"));
}

#[test]
fn a_clash_is_shown_with_the_reason_until_conditions_tell_the_commands_apart() {
    let mut h = demo();
    // a hand-edited file: Survey's second shortcut is D, which is Detail's declared key
    h.app.ui.settings.keymap.insert("view.survey#2".into(), "D".into());
    open_window(&mut h);
    let text = shown(&h);
    assert!(text.contains("share D") && text.contains("both always work"), "{text}");
    set(&mut h, json!({"id": "view.survey", "when": "!(view == detail)"}));
    let r = set(&mut h, json!({"id": "view.detail", "when": "view == detail"}));
    assert_eq!(r["conflictsWith"], json!([]), "{r}");
    h.step();
    h.step();
    assert!(!shown(&h).contains("share D"), "{}", shown(&h));
    // widening one brings the clash back, with the shared context named
    let r = set(&mut h, json!({"id": "view.detail", "when": "view != compare"}));
    assert_eq!(r["conflictsWith"], json!(["view.survey"]), "{r}");
    h.step();
    h.step();
    let text = shown(&h);
    assert!(text.contains("share D") && text.contains("both work at the same time"), "{text}");
}

#[test]
fn a_chord_follows_its_condition() {
    let mut h = demo();
    set(&mut h, json!({"id": "view.survey", "shortcut": "Cmd+K S", "when": "view == detail"}));
    let r = h.request("ui.key", json!({"key": "K", "cmd": true}), T);
    assert_eq!(r["ok"], true);
    h.step();
    h.step();
    assert!(h.app.keybinds.pending.is_none(), "in the grid the leader does nothing");
    h.app.run("view.detail", json!({})).unwrap();
    h.step();
    let r = h.request("ui.key", json!({"key": "K", "cmd": true}), T);
    assert_eq!(r["ok"], true);
    h.step();
    h.step();
    assert!(h.app.keybinds.pending.is_some(), "in the loupe it waits for the next key");
    key(&mut h, "S");
    assert_eq!(h.app.ui.view, ViewMode::Survey);
}

#[test]
fn a_condition_that_asks_for_a_dialog_works_inside_one() {
    let mut h = demo();
    set(&mut h, json!({"id": "view.survey", "shortcut": "K", "when": "dialogOpen"}));
    key(&mut h, "K");
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid, "no dialog: the condition does not hold");
    h.app.run("dialog.about", json!({})).ok();
    h.app.ui.dialog = Some(crate::state::Dialog::Shortcuts);
    h.step();
    h.step();
    key(&mut h, "K");
    assert_eq!(h.app.ui.view, ViewMode::Survey, "dialogs normally own the keys; this one asked for them");
    // a condition that merely allows dialogs does not open the gate
    h.app.ui.view = ViewMode::PhotoGrid;
    set(&mut h, json!({"id": "view.survey", "when": "!dialogOpen || view == grid"}));
    key(&mut h, "K");
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
}

#[test]
fn set_shortcut_when_validates_and_reports() {
    let mut h = demo();
    let r = set(&mut h, json!({"id": "view.survey", "when": "view == detail  &&  !textFocus"}));
    assert_eq!(r["when"], "view == detail && !textFocus", "saved in its canonical spelling: {r}");
    let e = h.app.run("app.setShortcut", json!({"id": "view.survey", "when": "view == "})).unwrap_err();
    assert!(e.contains("not a condition") && e.contains("value of view"), "{e}");
    assert_eq!(when_of(&h, "view.survey").as_deref(), Some("view == detail && !textFocus"), "unchanged");
    assert!(h.app.run("app.setShortcut", json!({"id": "view.survey", "when": 3})).is_err());
    let r = set(&mut h, json!({"id": "view.survey", "when": null}));
    assert_eq!(r["when"], serde_json::Value::Null);
    // the contexts and the command list carry the conditions
    let c = h.app.run("app.shortcutContexts", json!({})).unwrap();
    assert!(c["keys"].as_array().unwrap().iter().any(|k| k["key"] == "textFocus" && k["type"] == "bool"), "{c}");
    let all = crate::control::all_commands(&h.app);
    let reject = all.as_array().unwrap().iter().find(|c| c["id"] == "photo.reject").unwrap();
    assert_eq!(reject["when"], "tool != crop");
}

#[test]
fn a_damaged_condition_is_ignored_with_a_notice() {
    let mut h = demo();
    h.app.ui.settings.keymap.insert("view.survey?when".into(), "view ==".into());
    open_window(&mut h);
    let text = shown(&h);
    assert!(text.contains("could not be read and is ignored"), "{text}");
    assert_eq!(when_of(&h, "view.survey"), None);
    // the shortcut still works
    close_window(&mut h);
    key(&mut h, "N");
    assert_eq!(h.app.ui.view, ViewMode::Survey);
}

/// The pictures of the feature (`LIGHTCRAFT_TEST_SHOTS=<folder>`).
#[test]
#[ignore = "writes pictures; run with LIGHTCRAFT_TEST_SHOTS"]
fn when_pictures() {
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let services = Services {
        png: Some(Box::new(png)),
        write_new: Some(Box::new(lightcraft_engine::export::write_file_new)),
        save_shortcuts_file: Some(Box::new(|_| None)),
        pick_shortcuts_file: Some(Box::new(Vec::new)),
        ..Default::default()
    };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1500.0, 950.0], 1.0);
    h.settle(SETTLE);
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    click(&mut h, "field:shortcutsSearch");
    type_text(&mut h, "view");
    for (id, when) in [
        ("view.beforeAfter", Some("view == detail && hasPhoto")),
        ("view.cropOverlay", Some("tool == crop")),
        ("view.compare", Some("!textFocus && view != grid")),
        ("view.detail", Some("!(view == compare || view == survey)")),
    ] {
        set(&mut h, json!({"id": id, "when": when}));
    }
    h.step();
    h.step();
    shot(&mut h, "when-chips.png");
    click(&mut h, "button:shortcutWhen-view.compare");
    assert_eq!(h.request("ui.key", json!({"key": "A", "cmd": true}), T)["ok"], true);
    type_text(&mut h, "!textFocus && ");
    shot(&mut h, "when-autocomplete.png");
    key(&mut h, "Escape");
    click(&mut h, "button:shortcutWhenAdd-view.compare");
    click(&mut h, "menu:whenKey-textFocus");
    click(&mut h, "menu:whenIsNot");
    shot(&mut h, "when-builder.png");
    key(&mut h, "Escape");
    h.app.ui.settings.keymap.insert("view.compare#2".into(), "D".into());
    set(&mut h, json!({"id": "view.compare", "when": null}));
    h.step();
    h.step();
    shot(&mut h, "when-conflict.png");
}
