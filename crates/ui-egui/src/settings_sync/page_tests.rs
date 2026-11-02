//! Settings ▸ Sync as a person uses it: the page's buttons run the commands, and the file buttons
//! ask the host's dialog for the file.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use lightcraft_engine::settings_folder::{self as folder, FILE_NAME, SyncEnv};

use crate::headless::Headless;
use crate::pick::{PickKind, PickRequest};
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

type Asked = Arc<Mutex<Vec<(PickRequest, Sender<Vec<String>>)>>>;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-settings-page-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A windowless app on a Mac called "Studio", whose host shows file dialogs "elsewhere": each request is
/// recorded with the sender that answers it.
fn app(home: &std::path::Path) -> (Headless, Asked) {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
    app.sync_settings.env = SyncEnv { shared_file: Some(folder::shared_file_in(home)), computer: Some("Studio".into()) };
    let asked: Asked = Default::default();
    let a = asked.clone();
    app.services.picker = Some(Box::new(move |req| {
        let (tx, rx) = channel();
        a.lock().unwrap().push((req, tx));
        Ok(rx)
    }));
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    h.settle(T);
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "sync"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..4 {
        h.step();
    }
    (h, asked)
}

/// A click as the host delivers it: a person's.
fn click(h: &mut Headless, id: &str) {
    let at = h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| r.center()).unwrap_or_else(|| panic!("no widget {id}"));
    let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.events.extend([egui::Event::PointerMoved(at), press(true), press(false)]);
    for _ in 0..3 {
        h.step();
    }
}

/// A click through the control channel: an agent's.
fn agent_click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    for _ in 0..3 {
        h.step();
    }
}

fn toast(h: &Headless) -> String {
    h.app.ui.toast.as_ref().map(|(t, ..)| t.clone()).unwrap_or_default()
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

#[test]
fn the_page_has_every_control_and_the_buttons_run_their_commands() {
    let home = scratch("page-home");
    let sync = scratch("page-folder");
    let (mut h, _asked) = app(&home);
    for id in [
        "button:settingsSyncChoose",
        "button:settingsSyncSave",
        "button:settingsSyncLoad",
        "check:settingsSyncKeep",
        "button:settingsSyncExport",
        "button:settingsSyncImport",
        "check:settingsSyncPresets",
    ] {
        assert!(has(&h, id), "{id} is not on the page");
    }
    // no folder yet: no file is written by Save
    assert!(h.app.ui.sync.folder.is_empty());
    h.app.run("settings.syncChoose", json!({"path": sync.to_string_lossy()})).unwrap();
    for _ in 0..3 {
        h.step();
    }
    click(&mut h, "button:settingsSyncSave");
    assert!(sync.join(FILE_NAME).exists(), "Save to Folder wrote the file");
    // Keep in Sync and Include User Presets are switches of this Mac
    click(&mut h, "check:settingsSyncKeep");
    assert!(h.app.ui.sync.keep);
    click(&mut h, "check:settingsSyncPresets");
    assert!(!h.app.ui.sync.presets);
    // Load from Folder shows the prompt, naming who saved the file
    click(&mut h, "button:settingsSyncLoad");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SettingsImport)));
    assert!(h.app.sync_settings.prompt.as_ref().unwrap().saved_line.as_ref().is_some_and(|l| l.starts_with("Saved on Studio, ")));
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn export_and_import_ask_the_hosts_dialog_for_the_file() {
    let home = scratch("dialog-home");
    let (mut h, asked) = app(&home);
    h.app.ui.brush_flow = 31.0;
    click(&mut h, "button:settingsSyncExport");
    let req = asked.lock().unwrap().last().map(|(r, _)| r.clone()).unwrap();
    assert_eq!(req.kind, PickKind::Save);
    assert_eq!(req.file_name.as_deref(), Some("LightCraft Settings.lcsettings"));
    assert_eq!(req.filter.as_ref().map(|(_, e)| *e), Some(&["lcsettings"][..]));
    let path = home.join("Mine.lcsettings");
    asked.lock().unwrap()[0].1.send(vec![path.to_string_lossy().to_string()]).unwrap();
    h.settle(T);
    assert!(path.exists(), "the file the dialog named was written");
    // cancelled: nothing written, nothing pending
    click(&mut h, "button:settingsSyncExport");
    asked.lock().unwrap()[1].1.send(vec![]).unwrap();
    h.settle(T);
    assert!(h.app.pending_picks.is_empty());

    // Import…: the dialog's answer opens the prompt, and OK applies it
    h.app.ui.brush_flow = 5.0;
    click(&mut h, "button:settingsSyncImport");
    let req = asked.lock().unwrap().last().map(|(r, _)| r.clone()).unwrap();
    assert_eq!(req.kind, PickKind::File);
    asked.lock().unwrap()[2].1.send(vec![path.to_string_lossy().to_string()]).unwrap();
    h.settle(T);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SettingsImport)));
    assert_eq!(h.app.ui.brush_flow, 5.0, "nothing changes before OK");
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.brush_flow, 31.0);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn input_from_the_control_channel_cannot_change_the_folder_or_keep_in_sync() {
    let home = scratch("agent-home");
    let sync = scratch("agent-folder");
    let shared = folder::shared_file_in(&home);
    std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
    std::fs::write(&shared, json!({"path": sync.to_string_lossy()}).to_string()).unwrap();
    let (mut h, asked) = app(&home);
    assert!(has(&h, "button:settingsSyncUse"), "the shared folder is offered");

    // an agent's click on Use This Folder or Choose… changes nothing, and says why
    agent_click(&mut h, "button:settingsSyncUse");
    assert!(h.app.ui.sync.folder.is_empty(), "the folder was set through a click");
    assert!(toast(&h).contains("user's choice"), "{}", toast(&h));
    h.app.ui.toast = None;
    agent_click(&mut h, "button:settingsSyncChoose");
    assert!(h.app.ui.sync.folder.is_empty());
    assert!(asked.lock().unwrap().is_empty(), "no folder dialog is opened for an agent");
    assert!(toast(&h).contains("user's choice"));

    // the same for the switches, by a click or by the keyboard: a pointer press at the switch
    h.app.ui.sync.folder = sync.to_string_lossy().into_owned();
    for _ in 0..3 {
        h.step();
    }
    h.app.ui.toast = None;
    agent_click(&mut h, "check:settingsSyncKeep");
    assert!(!h.app.ui.sync.keep, "Keep in Sync was switched on through a click");
    assert!(toast(&h).contains("user's choice"));
    agent_click(&mut h, "check:settingsSyncPresets");
    assert!(h.app.ui.sync.presets, "Include User Presets was switched off through a click");
    let at = h.app.widgets.iter().find(|(w, _)| w == "check:settingsSyncKeep").map(|(_, r)| r.center()).unwrap();
    let r =
        h.request("ui.pointer", json!({"screen": true, "events": [{"kind": "down", "x": at.x, "y": at.y}, {"kind": "up", "x": at.x, "y": at.y}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..3 {
        h.step();
    }
    assert!(!h.app.ui.sync.keep, "Keep in Sync was switched on through ui.pointer");

    // a person's click does all of it
    click(&mut h, "check:settingsSyncKeep");
    assert!(h.app.ui.sync.keep);
    click(&mut h, "check:settingsSyncPresets");
    assert!(!h.app.ui.sync.presets);
    // and a person's Use This Folder
    h.app.ui.sync.folder.clear();
    for _ in 0..3 {
        h.step();
    }
    click(&mut h, "button:settingsSyncUse");
    assert_eq!(h.app.ui.sync.folder, sync.to_string_lossy());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn an_agents_click_on_save_to_folder_is_refused_over_newer_settings_and_the_person_is_asked() {
    let home = scratch("savebtn-home");
    let sync = scratch("savebtn-folder");
    let (mut h, _asked) = app(&home);
    h.app.ui.sync.folder = sync.to_string_lossy().into_owned();
    h.app.ui.sync.last = "2026-11-02T10:00:00+01:00".into();
    let by = folder::SavedBy { computer: "Laptop".into(), at: "2026-11-02T12:00:00+01:00".into(), id: None };
    folder::save(&sync.to_string_lossy(), json!({"kind": "lightcraft-settings", "version": 1}), by).unwrap();
    let original = std::fs::read_to_string(sync.join(FILE_NAME)).unwrap();
    for _ in 0..3 {
        h.step();
    }
    agent_click(&mut h, "button:settingsSyncSave");
    assert_eq!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original, "an agent's click replaced newer settings");
    assert!(!matches!(h.app.ui.dialog, Some(Dialog::SettingsReplace)), "an agent was not asked");
    assert!(toast(&h).contains("Replace them?"), "{}", toast(&h));
    // a person is asked, in words
    click(&mut h, "button:settingsSyncSave");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SettingsReplace)));
    assert!(h.app.sync_settings.replace.as_ref().unwrap().starts_with("The folder has newer settings from Laptop, "));
    assert_eq!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}
