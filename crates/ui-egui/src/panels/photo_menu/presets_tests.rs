use serde_json::{Value, json};

use super::backup::{self, KIND, VERSION};
use super::presets::{self, Presets};
use crate::state::AppSettings;
use crate::{LightcraftApp, Services};

fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default())
}

fn set(app: &mut LightcraftApp, items: &[&str]) {
    app.run("photoMenu.set", json!({"items": items})).unwrap();
}

fn menu(app: &LightcraftApp) -> Vec<String> {
    presets::current(app)
}

/// A scratch folder for one test.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("lcraw").join("pmenu-tests").join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn path_in(dir: &std::path::Path, file: &str) -> String {
    dir.join(file).to_string_lossy().to_string()
}

#[test]
fn slots_save_switch_and_say_when_empty() {
    let mut app = app();
    set(&mut app, &["rename", "---", "delete"]);
    app.run("photoMenu.savePreset", json!({"slot": 1})).unwrap();
    set(&mut app, &["reload"]);
    app.run("photoMenu.savePreset", json!({"slot": 2})).unwrap();
    // an empty slot says so and changes nothing
    let e = app.run("photoMenu.switchPreset", json!({"slot": 3})).unwrap_err();
    assert!(e.contains("Preset 3 is empty"), "{e}");
    assert_eq!(menu(&app), ["reload"]);
    let r = app.run("photoMenu.switchPreset", json!({"slot": 1})).unwrap();
    assert_eq!(menu(&app), ["rename", "---", "delete"]);
    assert_eq!(r["presets"][0]["active"], true);
    assert_eq!(r["presets"][1]["active"], false);
    assert_eq!(r["presets"][2]["saved"], false);
    assert_eq!(r["items"], json!(["rename", "---", "delete"]));
    // slot numbers are 1 to 3
    for bad in [json!({}), json!({"slot": 0}), json!({"slot": 4}), json!({"slot": "1"})] {
        assert!(app.run("photoMenu.switchPreset", bad.clone()).is_err(), "{bad}");
        assert!(app.run("photoMenu.savePreset", bad).is_err());
    }
}

#[test]
fn a_switch_can_be_taken_back_and_an_edit_shows_on_the_active_slot() {
    let mut app = app();
    assert!(app.run("photoMenu.undoPreset", json!({})).is_err(), "nothing to undo yet");
    set(&mut app, &["rename"]);
    app.run("photoMenu.savePreset", json!({"slot": 1})).unwrap();
    // an arrangement that was never saved, then a switch
    set(&mut app, &["delete", "reload"]);
    assert!(!presets::states(&app)[0].edited || presets::states(&app)[0].active);
    app.run("photoMenu.switchPreset", json!({"slot": 1})).unwrap();
    assert_eq!(menu(&app), ["rename"]);
    assert!(presets::can_undo(&app));
    // an edit after the switch: Preset 1 is still the active one, shown as edited, and the switch
    // is no longer the last thing done to the menu
    set(&mut app, &["rename", "reload"]);
    let s = presets::states(&app)[0];
    assert!(s.active && s.edited);
    assert!(!presets::can_undo(&app));
    assert!(app.run("photoMenu.undoPreset", json!({})).is_err());
    app.run("photoMenu.savePreset", json!({"slot": 1})).unwrap();
    assert!(!presets::states(&app)[0].edited);
    // a switch from another arrangement, then back
    set(&mut app, &["delete"]);
    app.run("photoMenu.switchPreset", json!({"slot": 1})).unwrap();
    assert_eq!(menu(&app), ["rename", "reload"]);
    let r = app.run("photoMenu.undoPreset", json!({})).unwrap();
    assert_eq!(menu(&app), ["delete"]);
    assert_eq!(r["canUndoPreset"], false);
    // switching to what is already showing leaves nothing to take back
    set(&mut app, &["rename", "reload"]);
    app.run("photoMenu.switchPreset", json!({"slot": 1})).unwrap();
    assert!(!presets::can_undo(&app));
}

#[test]
fn the_default_menu_is_a_preset_too() {
    let mut app = app();
    app.run("photoMenu.savePreset", json!({"slot": 3})).unwrap();
    set(&mut app, &["rename"]);
    app.run("photoMenu.switchPreset", json!({"slot": 3})).unwrap();
    assert_eq!(app.ui.settings.photo_menu, None, "the default is saved as the default");
    assert_eq!(app.ui.settings.photo_menu_presets.get(2).map(Vec::len), Some(super::DEFAULT.len()));
}

#[test]
fn presets_persist_in_the_settings_and_a_broken_value_loads_as_none() {
    let mut app = app();
    set(&mut app, &["rename"]);
    app.run("photoMenu.savePreset", json!({"slot": 2})).unwrap();
    let saved = serde_json::to_value(&app.ui.settings).unwrap();
    assert_eq!(saved["photoMenuPresets"]["slots"], json!([null, ["rename"], null]));
    assert_eq!(saved["photoMenuPresets"]["active"], 1);
    let back: AppSettings = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(back.photo_menu_presets, app.ui.settings.photo_menu_presets);
    // nothing saved writes nothing
    let default = serde_json::to_value(AppSettings::default()).unwrap();
    assert!(default.get("photoMenuPresets").is_none());
    // a value of the wrong shape costs the presets, not the settings
    for broken in [json!(42), json!("x"), json!({"slots": 7}), json!({"slots": [5, {"a": 1}], "active": "x", "undo": 3})] {
        let mut s = default.clone();
        s["photoMenuPresets"] = broken.clone();
        let back: AppSettings = serde_json::from_value(s).unwrap();
        assert_eq!(back.photo_menu_presets, Presets::default(), "{broken}");
    }
    // stray entries cost only themselves; an active slot that is empty is dropped
    let mut s = default;
    s["photoMenuPresets"] = json!({"slots": [["rename", 5, "---"], null, null, ["extra"]], "active": 2});
    let back: AppSettings = serde_json::from_value(s).unwrap();
    assert_eq!(back.photo_menu_presets.get(0), Some(&vec!["rename".to_string(), "---".to_string()]));
    assert_eq!(back.photo_menu_presets.slots.len(), 3);
    assert_eq!(back.photo_menu_presets.active, None);
}

#[test]
fn export_then_import_restores_the_menu_and_the_presets() {
    let dir = scratch("roundtrip");
    let path = path_in(&dir, "menu.json");
    let mut app = app();
    set(&mut app, &["rename", "---", "cmd:view.compare"]);
    app.run("photoMenu.savePreset", json!({"slot": 1})).unwrap();
    set(&mut app, &["delete"]);
    app.run("photoMenu.savePreset", json!({"slot": 3})).unwrap();
    set(&mut app, &["reload", "export"]);
    let r = app.run("photoMenu.export", json!({"path": path})).unwrap();
    assert_eq!(r["path"], path.as_str());
    assert_eq!(r["presets"], 2);
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file["kind"], KIND);
    assert_eq!(file["version"], VERSION);
    assert_eq!(file["items"], json!(["reload", "export"]));
    assert_eq!(file["presets"]["slots"], json!([["rename", "---", "cmd:view.compare"], null, ["delete"]]));

    // another app takes it
    let mut other = app_with_menu();
    let r = other.run("photoMenu.import", json!({"path": path})).unwrap();
    assert_eq!(r["skipped"], json!([]));
    assert_eq!(menu(&other), ["reload", "export"]);
    assert_eq!(other.ui.settings.photo_menu_presets.slots, app.ui.settings.photo_menu_presets.slots);
    assert_eq!(other.ui.settings.photo_menu_presets.active, Some(2));
    other.run("photoMenu.switchPreset", json!({"slot": 1})).unwrap();
    assert_eq!(menu(&other), ["rename", "---", "cmd:view.compare"]);
    std::fs::remove_dir_all(&dir).ok();
}

fn app_with_menu() -> LightcraftApp {
    let mut a = app();
    set(&mut a, &["openInDetail"]);
    a.run("photoMenu.savePreset", json!({"slot": 2})).unwrap();
    a
}

#[test]
fn the_default_menu_exports_and_imports_as_the_default() {
    let dir = scratch("default");
    let path = path_in(&dir, "menu.json");
    let mut app = app();
    app.run("photoMenu.export", json!({"path": path})).unwrap();
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(file.get("presets").is_none(), "no presets, none written");
    let mut other = app_with_menu();
    other.run("photoMenu.import", json!({"path": path})).unwrap();
    assert_eq!(other.ui.settings.photo_menu, None);
    assert!(!other.ui.settings.photo_menu_presets.is_empty(), "a file without presets leaves the presets as they are");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_wrong_foreign_or_newer_file_is_refused_and_changes_nothing() {
    let dir = scratch("refused");
    let mut app = app_with_menu();
    let before = (app.ui.settings.photo_menu.clone(), app.ui.settings.photo_menu_presets.clone());
    let write = |name: &str, bytes: &[u8]| {
        let p = path_in(&dir, name);
        std::fs::write(&p, bytes).unwrap();
        p
    };
    let newer = json!({"kind": KIND, "version": VERSION + 1, "items": ["rename"]}).to_string();
    let cases: Vec<(String, &str)> = vec![
        (write("not-json.json", b"{not json"), "not JSON"),
        (write("binary.json", &[0xff, 0xfe, 0x00, 0x80]), "not text"),
        (write("list.json", b"[1, 2]"), "not a JSON object"),
        (write("nokind.json", br#"{"version": 1, "items": []}"#), "no kind"),
        (write("foreign.json", br#"{"kind": "lightcraft-top-bar", "version": 1, "items": []}"#), "lightcraft-top-bar"),
        (write("newer.json", newer.as_bytes()), "newer LightCraft"),
        (write("noversion.json", format!(r#"{{"kind": "{KIND}", "items": []}}"#).as_bytes()), "no valid version"),
        (write("zero.json", format!(r#"{{"kind": "{KIND}", "version": 0, "items": []}}"#).as_bytes()), "no valid version"),
        (write("noitems.json", format!(r#"{{"kind": "{KIND}", "version": 1}}"#).as_bytes()), "no `items`"),
        (write("empty.json", b""), "not JSON"),
        (path_in(&dir, "missing.json"), "Could not read"),
    ];
    for (path, says) in cases {
        let e = app.run("photoMenu.import", json!({"path": path})).unwrap_err();
        assert!(e.contains(says), "{path}: {e}");
        assert_eq!((app.ui.settings.photo_menu.clone(), app.ui.settings.photo_menu_presets.clone()), before, "{path}");
    }
    // a file too big to be a backup is not read
    let big = write("big.json", &vec![b' '; (1 << 20) + 1]);
    assert!(app.run("photoMenu.import", json!({"path": big})).unwrap_err().contains("too big"));
    assert!(app.run("photoMenu.import", json!({"path": ""})).is_err());
    assert!(app.run("photoMenu.export", json!({"path": path_in(&dir, "no/such/folder/x.json")})).unwrap_err().contains("Could not write"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unknown_items_are_skipped_and_named() {
    let dir = scratch("unknown");
    let path = path_in(&dir, "menu.json");
    let file = json!({
        "kind": KIND, "version": VERSION,
        "items": ["rename", "fromAFutureVersion", "---", "cmd:no.suchCommand", "rename", 5, "cmd:view.compare", "---"],
        "presets": {"slots": [["delete", "alsoNew"], 7, ["reload"]], "active": 1, "extraKey": true},
        "somethingNew": {"a": 1},
    });
    std::fs::write(&path, file.to_string()).unwrap();
    let mut app = app();
    let r = app.run("photoMenu.import", json!({"path": path})).unwrap();
    assert_eq!(menu(&app), ["rename", "---", "cmd:view.compare", "---"]);
    let skipped: Vec<&str> = r["skipped"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(skipped, ["fromAFutureVersion", "cmd:no.suchCommand", "rename", "5", "alsoNew"]);
    let p = &app.ui.settings.photo_menu_presets;
    assert_eq!(p.get(0), Some(&vec!["delete".to_string()]));
    assert_eq!(p.get(1), None, "a slot that is not a list is empty");
    assert_eq!(p.get(2), Some(&vec!["reload".to_string()]));
    assert_eq!(p.active, None, "the active slot was empty");
    let note = backup::skipped_note(&skipped.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(note, "5 unknown items skipped: fromAFutureVersion, cmd:no.suchCommand, rename, 5 and 1 more");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_command_without_a_path_asks_a_dialog_and_runs_again_with_the_answer() {
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Arc, Mutex};
    let dir = scratch("dialog");
    let path = path_in(&dir, "chosen.json");
    let mut app = app();
    let asked: Arc<Mutex<Vec<(crate::pick::PickRequest, Sender<Vec<String>>)>>> = Default::default();
    let a = asked.clone();
    app.services.picker = Some(Box::new(move |req| {
        let (tx, rx) = channel();
        a.lock().unwrap().push((req, tx));
        Ok(rx)
    }));
    set(&mut app, &["rename"]);
    // Export…: a save dialog for a .json file; the command runs again when it closes
    assert_eq!(app.run("photoMenu.export", json!({})).unwrap(), Value::Null);
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0.kind, crate::pick::PickKind::Save);
        assert_eq!(asked[0].0.filter.as_ref().map(|f| f.1), Some(&["json"][..]));
        assert_eq!(asked[0].0.file_name.as_deref(), Some("Photo Menu.json"));
        asked[0].1.send(vec![path.clone()]).unwrap();
    }
    crate::pick::poll(&mut app, &egui::Context::default());
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file["items"], json!(["rename"]));
    // Import…: an open dialog; cancelling changes nothing
    set(&mut app, &["delete"]);
    assert_eq!(app.run("photoMenu.import", json!({})).unwrap(), Value::Null);
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked[1].0.kind, crate::pick::PickKind::File);
        asked[1].1.send(vec![]).unwrap();
    }
    crate::pick::poll(&mut app, &egui::Context::default());
    assert_eq!(menu(&app), ["delete"]);
    // with no dialog on the platform, `path` is asked for
    app.services.picker = None;
    let e = app.run("photoMenu.import", json!({})).unwrap_err();
    assert!(e.contains("path"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn get_reports_the_presets() {
    let mut app = app();
    set(&mut app, &["rename"]);
    app.run("photoMenu.savePreset", json!({"slot": 2})).unwrap();
    let r = app.run("photoMenu.get", json!({})).unwrap();
    assert_eq!(r["presets"].as_array().map(Vec::len), Some(3));
    assert_eq!(r["presets"][1], json!({"slot": 2, "saved": true, "active": true, "edited": false, "items": ["rename"]}));
    assert_eq!(r["presets"][0], json!({"slot": 1, "saved": false, "active": false, "edited": false, "items": null}));
}

#[test]
fn clear_empties_a_slot_and_keeps_the_menu() {
    let mut app = app();
    set(&mut app, &["rename"]);
    app.run("photoMenu.savePreset", json!({"slot": 2})).unwrap();
    app.run("photoMenu.savePreset", json!({"slot": 3})).unwrap();
    let r = app.run("photoMenu.clearPreset", json!({"slot": 3})).unwrap();
    assert_eq!(menu(&app), ["rename"]);
    assert_eq!(r["presets"][2]["saved"], false);
    assert_eq!(r["presets"][2]["active"], false, "the active slot, once cleared, is no longer active");
    assert_eq!(r["presets"][1]["saved"], true);
    // already empty, or no such slot
    assert!(app.run("photoMenu.clearPreset", json!({"slot": 3})).unwrap_err().contains("already empty"));
    for bad in [json!({}), json!({"slot": 0}), json!({"slot": 4})] {
        assert!(app.run("photoMenu.clearPreset", bad).is_err());
    }
}
