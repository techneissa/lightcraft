//! Headless tests of Export / Import for keyboard shortcuts (fork feature `feat/shortcut-when`):
//! the commands `app.exportShortcuts` / `app.importShortcuts` and the two icon buttons of the
//! Keyboard Shortcuts window.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::keybinds::model::conditions;
use crate::shortcuts::{Keymap, shortcut_of};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("/tmp/lcraw/when").join(format!("backup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn app_with(services: Services) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1500.0, 950.0], 1.0);
    h.settle(Duration::from_secs(120));
    h
}

fn customise(h: &mut Headless) {
    h.app.run("app.setShortcut", json!({"id": "view.survey", "shortcut": "K", "when": "!(view == detail)"})).unwrap();
    h.app.run("app.setShortcut", json!({"id": "view.survey", "slot": 2, "shortcut": "Cmd+K S"})).unwrap();
    h.app.run("app.setShortcut", json!({"id": "view.detail", "shortcut": null})).unwrap();
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

#[test]
fn the_commands_round_trip_a_customised_keymap_through_a_file() {
    let mut h = app_with(Services::default());
    customise(&mut h);
    conditions::set(&mut h.app.ui.settings.keymap, "view.people", Some("view != grid && !textFocus")).unwrap();
    let saved: Keymap = h.app.ui.settings.keymap.clone();
    let path = scratch("round.json");
    let r = h.app.run("app.exportShortcuts", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["path"], path.to_string_lossy().as_ref());
    assert!(r["commands"].as_u64().unwrap() >= 3, "{r}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file["kind"], "lightcraft-shortcuts");
    assert_eq!(file["version"], 1);
    assert_eq!(file["shortcuts"]["view.survey"]["shortcut2"], "Cmd+K S", "both slots, a chord");
    assert_eq!(file["shortcuts"]["view.survey"]["when"], "!(view == detail)");
    // lose everything, then bring it back
    h.app.run("app.resetShortcuts", json!({})).unwrap();
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("N"));
    let r = h.app.run("app.importShortcuts", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["notices"], json!([]), "{r}");
    assert_eq!(h.app.ui.settings.keymap, saved);
    assert_eq!(conditions::text(&h.app.ui.settings.keymap, "view.survey").as_deref(), Some("!(view == detail)"));
}

#[test]
fn a_refused_file_changes_nothing_and_says_why() {
    let mut h = app_with(Services::default());
    customise(&mut h);
    let before = h.app.ui.settings.keymap.clone();
    let run = |h: &mut Headless, name: &str, body: &str| {
        let path = scratch(name);
        std::fs::write(&path, body).unwrap();
        h.app.run("app.importShortcuts", json!({"path": path.to_string_lossy()})).unwrap_err()
    };
    let e = run(&mut h, "foreign.json", r#"{"kind": "vscode-keybindings", "version": 1}"#);
    assert!(e.contains("isn't a LightCraft shortcuts file"), "{e}");
    let e = run(&mut h, "newer.json", r#"{"kind": "lightcraft-shortcuts", "version": 99, "shortcuts": {}}"#);
    assert!(e.contains("newer LightCraft"), "{e}");
    let e = run(&mut h, "junk.json", "this is not json");
    assert!(e.contains("readable JSON"), "{e}");
    let e = h.app.run("app.importShortcuts", json!({"path": "/tmp/lcraw/when/does-not-exist.json"})).unwrap_err();
    assert!(e.contains("Can't read"), "{e}");
    assert_eq!(h.app.run("app.importShortcuts", json!({})).unwrap_err(), "missing path");
    assert_eq!(h.app.run("app.exportShortcuts", json!({})).unwrap_err(), "missing path");
    assert_eq!(h.app.ui.settings.keymap, before, "nothing changed");
}

#[test]
fn unknown_commands_and_bad_conditions_are_reported_in_the_reply() {
    let mut h = app_with(Services::default());
    let path = scratch("partly.json");
    let doc = json!({"kind": "lightcraft-shortcuts", "version": 1, "shortcuts": {
        "gone.command": {"shortcut": "Q"},
        "view.survey": {"shortcut": "K", "when": "view =="},
        "view.compare": {"when": "view != grid"},
    }});
    std::fs::write(&path, doc.to_string()).unwrap();
    let r = h.app.run("app.importShortcuts", json!({"path": path.to_string_lossy()})).unwrap();
    let notices: Vec<&str> = r["notices"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices.iter().any(|n| n.contains("gone.command")));
    assert!(notices.iter().any(|n| n.contains("Dropped the condition")));
    assert_eq!(shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("K"), "the good part of the entry is taken");
    assert_eq!(conditions::text(&h.app.ui.settings.keymap, "view.compare").as_deref(), Some("view != grid"));
    assert_eq!(h.app.keybinds.import_notes.len(), 2);
}

#[test]
fn the_window_has_two_icon_buttons_that_export_and_import() {
    let path = scratch("window.json");
    let _ = std::fs::remove_file(&path);
    let (p1, p2) = (path.to_string_lossy().to_string(), path.to_string_lossy().to_string());
    let services = Services {
        save_shortcuts_file: Some(Box::new(move |_| Some(p1.clone()))),
        pick_shortcuts_file: Some(Box::new(move || vec![p2.clone()])),
        ..Default::default()
    };
    let mut h = app_with(services);
    customise(&mut h);
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    for id in ["icon:shortcutsExport", "icon:shortcutsImport"] {
        assert!(h.app.widgets.iter().any(|(w, _)| w == id), "{id}");
    }
    let saved = h.app.ui.settings.keymap.clone();
    click(&mut h, "icon:shortcutsExport");
    assert!(path.exists(), "Export wrote the file");
    h.app.run("app.resetShortcuts", json!({})).unwrap();
    click(&mut h, "icon:shortcutsImport");
    assert_eq!(h.app.ui.settings.keymap, saved, "Import brought it back");
    // a bad file shows its reason under the list, and the keymap stays
    std::fs::write(&path, r#"{"kind": "other", "version": 1}"#).unwrap();
    click(&mut h, "icon:shortcutsImport");
    assert_eq!(h.app.ui.settings.keymap, saved);
    let shown = h.painted_text().join("\n");
    assert!(shown.contains("isn't a LightCraft shortcuts file"), "{shown}");
    click(&mut h, "button:shortcutsNotesDismiss");
    assert!(h.app.keybinds.import_notes.is_empty());
}

#[test]
fn a_cancelled_save_panel_writes_nothing() {
    let calls = Rc::new(RefCell::new(0));
    let c = calls.clone();
    let services = Services {
        save_shortcuts_file: Some(Box::new(move |_| {
            *c.borrow_mut() += 1;
            None
        })),
        pick_shortcuts_file: Some(Box::new(Vec::new)),
        ..Default::default()
    };
    let mut h = app_with(services);
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    click(&mut h, "icon:shortcutsExport");
    click(&mut h, "icon:shortcutsImport");
    assert_eq!(*calls.borrow(), 1, "the panel was asked once");
    assert!(h.app.keybinds.import_notes.is_empty());
}
