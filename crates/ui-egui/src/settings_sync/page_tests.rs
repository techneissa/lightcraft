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

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    for _ in 0..3 {
        h.step();
    }
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
