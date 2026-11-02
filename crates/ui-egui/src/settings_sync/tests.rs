//! Tests of Settings sync: the file and the keys that stay on one Mac, reading and applying a file
//! (newer, older, damaged), the commands, the prompt and Keep in Sync. Folders are temporary ones and
//! the Mac's name and the shared file are set on the app, so nothing real is touched.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use lightcraft_engine::settings_folder::{self as folder, FILE_NAME, SyncEnv};

use super::{plan, rules};
use crate::state::{Dialog, UiState};
use crate::{LightcraftApp, Services};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-settings-sync-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An app on a Mac called `computer`, whose shared folder file is inside `home`.
fn app_on(computer: &str, home: &Path) -> LightcraftApp {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
    app.sync_settings.env = SyncEnv { shared_file: Some(folder::shared_file_in(home)), computer: Some(computer.into()) };
    app
}

fn file_text(app: &LightcraftApp) -> String {
    rules::text(&rules::document(&app.ui, None))
}

/// Settings of every kind a person changes.
fn customise(app: &mut LightcraftApp) {
    app.ui.settings.dark_theme = crate::state::DarkTheme::ALL[1];
    app.ui.settings.keymap.insert("view.survey".into(), "K".into());
    app.ui.settings.keymap.insert("view.survey#2".into(), "Cmd+K S".into());
    app.ui.settings.top_bar_hidden = vec!["bell".into()];
    app.ui.settings.film_names = !app.ui.settings.film_names;
    app.ui.brush_size = 0.123;
    app.ui.remove_opacity = 40.0;
    app.ui.click_zoom = 400;
    app.ui.folded_sections = vec!["keywording".into()];
    app.ui.edit_sections.set_hidden(crate::panels::edit_sections::EditSection::Detail, true);
    app.ui.sidebar.set_collapsed(crate::sidebar::SidebarSection::ALL[1], true);
}

/// What one Mac keeps to itself, set to something recognisable.
fn mark_this_mac(app: &mut LightcraftApp) {
    app.ui.left_width = 333.0;
    app.ui.right_width = 444.0;
    app.ui.local_roots = vec!["/Volumes/Mine".into()];
    app.ui.zoom = crate::state::Zoom::Fit;
    app.ui.pan = (0.2, 0.7);
    app.ui.settings.library_path = "/Users/me/Library-A".into();
    app.ui.settings.display_profile = "/Library/ColorSync/Mine.icc".into();
    app.ui.settings.lens_database_dir = "/Users/me/lensfun".into();
}

fn import_text(app: &mut LightcraftApp, text: &str) -> Result<Value, String> {
    app.run("settings.import", json!({"text": text}))
}

#[test]
fn the_text_limit_is_the_folders() {
    assert_eq!(super::MAX_BYTES, folder::MAX_BYTES);
}

// ------------------------------------------------------------------ the keys

#[test]
fn every_ui_key_is_decided() {
    // A key of `ui.json` that is in neither list is a setting nobody decided about: it is synced (the file is
    // built from a deny list), but decide: name it in `SYNCED` for the file, or in `LEFT_OUT` with the reason it
    // belongs to one Mac (a window, a screen, a folder, a hardware choice, what is open right now).
    let ui = serde_json::to_value(UiState::default()).unwrap();
    let keys = ui.as_object().unwrap();
    let undecided: Vec<&String> = keys.keys().filter(|k| rules::left_out(k).is_none() && !rules::SYNCED.contains(&k.as_str())).collect();
    assert!(undecided.is_empty(), "ui.json keys that are neither synced nor left out (add them to settings_sync::rules): {undecided:?}");
    let settings = keys["settings"].as_object().unwrap();
    let undecided: Vec<&String> =
        settings.keys().filter(|k| rules::left_out_setting(k).is_none() && !rules::SYNCED_SETTINGS.contains(&k.as_str())).collect();
    assert!(undecided.is_empty(), "settings keys that are neither synced nor left out (add them to settings_sync::rules): {undecided:?}");
}

#[test]
fn the_lists_name_real_keys_and_none_twice() {
    // keys that `ui.json` leaves out while they hold nothing
    const ABSENT_AT_DEFAULT: [&str; 5] = ["window", "showClipping", "photoMenu", "photoMenuPresets", "topBarHidden"];
    let ui = serde_json::to_value(UiState::default()).unwrap();
    let keys = ui.as_object().unwrap();
    let settings = keys["settings"].as_object().unwrap();
    for name in rules::LEFT_OUT.iter().map(|(k, _)| *k).chain(rules::SYNCED.iter().copied()) {
        assert!(keys.contains_key(name) || ABSENT_AT_DEFAULT.contains(&name), "`{name}` is not a key of ui.json");
    }
    for name in rules::LEFT_OUT_SETTINGS.iter().map(|(k, _)| *k).chain(rules::SYNCED_SETTINGS.iter().copied()) {
        assert!(settings.contains_key(name) || ABSENT_AT_DEFAULT.contains(&name), "`{name}` is not a key of settings");
    }
    for (k, why) in rules::LEFT_OUT.iter().chain(rules::LEFT_OUT_SETTINGS) {
        assert!(!why.is_empty(), "{k} needs a reason");
    }
    assert!(rules::LEFT_OUT.iter().all(|(k, _)| !rules::SYNCED.contains(k)), "a key is both synced and left out");
    assert!(rules::LEFT_OUT_SETTINGS.iter().all(|(k, _)| !rules::SYNCED_SETTINGS.contains(k)), "a setting is both synced and left out");
}

#[test]
fn the_file_leaves_out_what_belongs_to_one_mac() {
    let home = scratch("left-out");
    let mut app = app_on("A", &home);
    customise(&mut app);
    mark_this_mac(&mut app);
    app.ui.sync.folder = "/Users/me/iCloud/LightCraft".into();
    app.ui.sync.keep = true;
    let doc = rules::document(&app.ui, None);
    assert_eq!(doc["kind"], "lightcraft-settings");
    assert_eq!(doc["version"], 1);
    let ui = doc["ui"].as_object().unwrap();
    for (key, _) in rules::LEFT_OUT {
        assert!(!ui.contains_key(*key), "`{key}` must stay on this Mac");
    }
    let settings = ui["settings"].as_object().unwrap();
    for (key, _) in rules::LEFT_OUT_SETTINGS {
        assert!(!settings.contains_key(*key), "`settings.{key}` must stay on this Mac");
    }
    // what travels
    assert_eq!(settings["keymap"]["view.survey#2"], "Cmd+K S");
    assert_eq!(ui["brushSize"].as_f64().map(|n| (n * 1000.0).round()), Some(123.0));
    let text = rules::text(&doc);
    for private in ["Volumes/Mine", "Library-A", "Mine.icc", "lensfun", "iCloud/LightCraft"] {
        assert!(!text.contains(private), "{private} leaked into the file");
    }
    let _ = std::fs::remove_dir_all(&home);
}

// ------------------------------------------------------------------ one file

#[test]
fn export_on_one_mac_and_import_on_another_gives_the_same_settings() {
    let home = scratch("round-trip");
    let mut a = app_on("Studio", &home);
    customise(&mut a);
    mark_this_mac(&mut a);
    let mut b = app_on("Laptop", &home);
    b.ui.left_width = 222.0;
    b.ui.settings.library_path = "/Users/me/Library-B".into();
    b.ui.local_roots = vec!["/Volumes/Laptop".into()];
    // B's own arrangement differs the other way
    b.ui.settings.dark_theme = crate::state::DarkTheme::ALL[0];
    b.ui.settings.top_bar_hidden = vec!["help".into()];

    let path = home.join("Settings.lcsettings");
    let r = a.run("settings.export", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["path"], path.to_string_lossy().as_ref());
    // written through a temporary name: nothing else is left beside it
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 1);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("savedBy"), "an exported file says who saved it");

    // (a person's Import… button gives `prompt`, as the file dialog's answer does)
    let r = b.run("settings.import", json!({"path": path.to_string_lossy(), "prompt": true})).unwrap();
    assert_eq!(r["dialog"], true, "a person is asked first");
    assert!(matches!(b.ui.dialog, Some(Dialog::SettingsImport)));
    // nothing changed yet
    assert_eq!(b.ui.settings.dark_theme, crate::state::DarkTheme::ALL[0]);
    crate::panels::dialogs::confirm_dialog(&mut b, &Dialog::SettingsImport).unwrap();

    assert_eq!(rules::ui_part(&a.ui), rules::ui_part(&b.ui), "the same theme, shortcuts and arrangements");
    assert_eq!(b.ui.settings.dark_theme, crate::state::DarkTheme::ALL[1]);
    assert_eq!(b.ui.settings.keymap.get("view.survey#2").map(String::as_str), Some("Cmd+K S"));
    assert!(b.ui.settings.top_bar_hidden == vec!["bell".to_string()]);
    // and what belongs to B's screen stayed
    assert_eq!(b.ui.left_width, 222.0);
    assert_eq!(b.ui.settings.library_path, "/Users/me/Library-B");
    assert_eq!(b.ui.local_roots, vec!["/Volumes/Laptop".to_string()]);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_mac_that_went_back_to_the_default_arrangement_takes_the_other_back_too() {
    let home = scratch("defaults");
    let a = app_on("A", &home);
    let mut b = app_on("B", &home);
    b.ui.settings.top_bar_hidden = vec!["bell".into()];
    b.ui.settings.photo_menu = Some(vec!["---".into()]);
    import_text(&mut b, &file_text(&a)).unwrap();
    assert!(b.ui.settings.top_bar_hidden.is_empty());
    assert_eq!(b.ui.settings.photo_menu, None);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn what_work_in_progress_holds_survives_an_import() {
    let home = scratch("carry");
    let mut a = app_on("A", &home);
    customise(&mut a);
    let mut b = app_on("B", &home);
    b.ui.external_edits = vec![7, 8];
    b.ui.status = "Working".into();
    import_text(&mut b, &file_text(&a)).unwrap();
    assert_eq!(b.ui.external_edits, vec![7, 8]);
    assert_eq!(b.ui.status, "Working");
    let _ = std::fs::remove_dir_all(&home);
}

// ------------------------------------------------------------------ newer and older files

fn newer_file(extra_ui: Value, extra_settings: Value) -> String {
    let mut ui = json!({"brushSize": 0.2, "settings": {"filmNames": false}});
    for (k, v) in extra_ui.as_object().unwrap() {
        ui[k] = v.clone();
    }
    for (k, v) in extra_settings.as_object().unwrap() {
        ui["settings"][k] = v.clone();
    }
    json!({"kind": "lightcraft-settings", "version": 2, "app": "LightCraft", "ui": ui, "futurePart": {"x": 1}}).to_string()
}

#[test]
fn a_file_from_a_newer_lightcraft_imports_what_this_build_knows_and_names_the_rest() {
    let home = scratch("newer");
    let mut app = app_on("B", &home);
    app.ui.settings.film_names = true;
    let text = newer_file(
        json!({"holoDeck": true, "toolStripOrder": ["a"], "sidebar": ["keywords", "hologram", "albums"]}),
        json!({"quantumMode": 3, "keymap": {"view.survey": "K", "view.teleport": "Cmd+T"}}),
    );
    let r = import_text(&mut app, &text).unwrap();
    assert_eq!(r["applied"], true);
    // taken
    assert_eq!(app.ui.brush_size, 0.2);
    assert!(!app.ui.settings.film_names);
    assert_eq!(app.ui.settings.keymap.get("view.survey").map(String::as_str), Some("K"));
    // named, never refused
    let unknown: Vec<&str> = r["unknown"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for name in
        ["futurePart", "ui.holoDeck", "ui.toolStripOrder", "ui.settings.quantumMode", "ui.sidebar.hologram", "ui.settings.keymap.view.teleport"]
    {
        assert!(unknown.contains(&name), "{name} not named in {unknown:?}");
    }
    assert_eq!(r["unknownCount"], unknown.len());
    assert_eq!(r["fileVersion"], 2, "{r}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn unknown_settings_are_named_at_most_two_hundred_times_and_counted_all() {
    let mut ui = serde_json::Map::new();
    for i in 0..300 {
        ui.insert(format!("future{i}"), json!(i));
    }
    let text = json!({"kind": "lightcraft-settings", "version": 3, "ui": ui}).to_string();
    let p = plan::plan(&text, true).unwrap();
    assert_eq!(p.unknown.len(), 200);
    assert_eq!(p.unknown_total, 300);
    let r = p.report(false);
    assert_eq!(r["unknownCount"], 300);
    assert_eq!(r["unknown"].as_array().unwrap().len(), 200);
}

#[test]
fn the_prompt_names_the_first_five_then_how_many_more() {
    use super::prompt::{unknown_line, unknown_list};
    let names: Vec<String> = (1..=8).map(|i| format!("ui.s{i}")).collect();
    assert_eq!(unknown_line(&[], 0), None);
    assert_eq!(unknown_line(&names[..3], 3).unwrap(), "3 settings from a newer LightCraft were skipped: ui.s1, ui.s2, ui.s3");
    assert_eq!(unknown_line(&names[..1], 1).unwrap(), "1 setting from a newer LightCraft was skipped: ui.s1");
    assert_eq!(unknown_line(&names, 8).unwrap(), "8 settings from a newer LightCraft were skipped: ui.s1, ui.s2, ui.s3, ui.s4, ui.s5, and 3 more");
    // the hover has all of them, and says when the list itself was cut
    assert_eq!(unknown_list(&names, 8).lines().count(), 8);
    assert!(unknown_list(&names, 250).ends_with("…and 242 more"));
}

#[test]
fn a_file_from_an_older_lightcraft_leaves_the_missing_settings_alone() {
    let home = scratch("older");
    let mut app = app_on("B", &home);
    customise(&mut app);
    let before = rules::ui_part(&app.ui);
    // an older file: one part of `ui`, no presets, no savedBy
    let text = json!({"kind": "lightcraft-settings", "version": 1, "ui": {"brushFlow": 33.0, "settings": {"dimRejected": false}}}).to_string();
    let r = import_text(&mut app, &text).unwrap();
    assert_eq!(r["applied"], true);
    assert_eq!(app.ui.brush_flow, 33.0);
    assert!(!app.ui.settings.dim_rejected);
    let mut after = rules::ui_part(&app.ui);
    // only those two differ
    after["brushFlow"] = before["brushFlow"].clone();
    after["settings"]["dimRejected"] = before["settings"]["dimRejected"].clone();
    assert_eq!(after, before);
    // a file with no parts at all changes nothing either
    let r = import_text(&mut app, &json!({"kind": "lightcraft-settings", "version": 1}).to_string()).unwrap();
    assert_eq!(r["parts"], json!([]));
    let _ = std::fs::remove_dir_all(&home);
}

// ------------------------------------------------------------------ bad values and damaged files

#[test]
fn a_bad_value_for_a_known_setting_names_it_and_changes_nothing() {
    let home = scratch("bad");
    let mut app = app_on("B", &home);
    customise(&mut app);
    let before = serde_json::to_value(&app.ui).unwrap();
    for (ui, expect) in [
        (json!({"brushSize": "big"}), "brushSize"),
        (json!({"brushSize": 5.0}), "out of range"),
        (json!({"clickZoom": 150}), "clickZoom"),
        (json!({"language": "tlh"}), "language"),
        (json!({"maskOverlayMode": 7}), "maskOverlayMode"),
        (json!({"settings": {"darkTheme": "plaid"}}), "darkTheme"),
        (json!({"settings": {"keymap": {"view.survey": "Hyper+Banana"}}}), "view.survey"),
        (json!({"settings": {"filmNames": "yes"}}), "filmNames"),
        (json!({"settings": 5}), "ui.settings"),
        (json!(["not", "an", "object"]), "ui"),
    ] {
        // a good setting beside the bad one must not be taken either
        let mut ui = ui;
        if let Some(m) = ui.as_object_mut() {
            m.insert("brushFlow".into(), json!(10.0));
        }
        let text = json!({"kind": "lightcraft-settings", "version": 1, "ui": ui}).to_string();
        let e = import_text(&mut app, &text).unwrap_err();
        assert!(e.contains(expect), "{expect}: {e}");
        assert!(e.contains("nothing was changed"), "{e}");
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before, "{expect} changed something");
    }
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_bad_value_in_a_newer_file_is_skipped_with_a_note() {
    let home = scratch("bad-newer");
    let mut app = app_on("B", &home);
    let text = newer_file(json!({"brushFlow": "a new kind of flow"}), json!({}));
    let r = import_text(&mut app, &text).unwrap();
    assert_eq!(r["applied"], true);
    assert_eq!(app.ui.brush_size, 0.2, "the rest was taken");
    assert_eq!(app.ui.brush_flow, UiState::default().brush_flow, "the bad one was not");
    assert!(r["skipped"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("brushFlow")), "{r}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_damaged_or_foreign_file_is_a_message_and_changes_nothing() {
    let home = scratch("damaged");
    let mut app = app_on("B", &home);
    customise(&mut app);
    let before = serde_json::to_value(&app.ui).unwrap();
    for (text, expect) in [
        ("", "readable JSON"),
        ("{ \"kind\": ", "readable JSON"),
        ("not json at all", "readable JSON"),
        ("[1, 2]", "isn't a JSON object"),
        ("{\"version\": 1}", "no \"kind\""),
        ("{\"kind\": \"photocraft-settings\", \"version\": 1}", "photocraft-settings"),
        ("{\"kind\": \"lightcraft-settings\"}", "no readable \"version\""),
        ("{\"kind\": \"lightcraft-settings\", \"version\": 0}", "no readable \"version\""),
        ("{\"kind\": \"lightcraft-settings\", \"version\": \"one\"}", "no readable \"version\""),
    ] {
        let e = import_text(&mut app, text).unwrap_err();
        assert!(e.contains(expect), "{text}: {e}");
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
    }
    // a file on disk that isn't text
    let bin = home.join("binary.lcsettings");
    std::fs::write(&bin, [0xff, 0x00, 0xfe]).unwrap();
    assert!(app.run("settings.import", json!({"path": bin.to_string_lossy()})).is_err());
    assert!(app.ui.dialog.is_none());
    let _ = std::fs::remove_dir_all(&home);
}

// ------------------------------------------------------------------ the folder

fn chosen(app: &mut LightcraftApp, dir: &Path) {
    app.run("settings.syncChoose", json!({"path": dir.to_string_lossy()})).unwrap();
}

#[test]
fn save_to_folder_and_load_from_folder_move_settings_between_macs_through_one_file() {
    let home = scratch("folder-home");
    let sync = scratch("folder");
    let mut a = app_on("Studio Mac", &home);
    customise(&mut a);
    chosen(&mut a, &sync);
    let r = a.run("settings.syncSave", json!({})).unwrap();
    assert_eq!(r["savedBy"]["computer"], "Studio Mac");
    assert_eq!(std::fs::read_dir(&sync).unwrap().count(), 1, "one file, no temporary one");
    let text = std::fs::read_to_string(sync.join(FILE_NAME)).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["kind"], "lightcraft-settings");
    assert!(chrono_like(doc["savedBy"]["at"].as_str().unwrap()), "{doc}");
    assert!(!text.contains(&*sync.to_string_lossy()), "the folder itself is this Mac's");

    let mut b = app_on("Laptop", &home);
    chosen(&mut b, &sync);
    let r = b.run("settings.syncLoad", json!({})).unwrap();
    assert_eq!(r["dialog"], true);
    let prompt = b.sync_settings.prompt.clone().unwrap();
    assert_eq!(prompt.name, FILE_NAME);
    let line = prompt.saved_line.clone().unwrap();
    assert!(line.starts_with("Saved on Studio Mac, "), "{line}");
    assert_ne!(b.ui.settings.dark_theme, a.ui.settings.dark_theme);
    crate::panels::dialogs::confirm_dialog(&mut b, &Dialog::SettingsImport).unwrap();
    assert_eq!(rules::ui_part(&a.ui), rules::ui_part(&b.ui));
    assert_eq!(b.ui.sync.last, prompt.saved_at.unwrap(), "this Mac now has that file");
    assert_eq!(b.ui.sync.folder, sync.to_string_lossy());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

/// An RFC 3339 time with an offset, to the second.
fn chrono_like(at: &str) -> bool {
    folder::SavedBy { computer: "x".into(), at: at.into() }.when().is_some() && at.len() >= 20
}

#[test]
fn a_file_without_saved_by_loads_and_the_prompt_leaves_the_line_out() {
    let home = scratch("nosaved-home");
    let sync = scratch("nosaved");
    let mut app = app_on("B", &home);
    chosen(&mut app, &sync);
    for damaged in [r#""savedBy": 5,"#, r#""savedBy": {"computer": 1},"#, ""] {
        std::fs::write(sync.join(FILE_NAME), format!(r#"{{{damaged} "kind": "lightcraft-settings", "version": 1, "ui": {{"brushFlow": 21.0}}}}"#))
            .unwrap();
        app.run("settings.syncLoad", json!({})).unwrap();
        let prompt = app.sync_settings.prompt.take().unwrap();
        assert_eq!(prompt.saved_line, None);
        assert_eq!(prompt.saved_at, None);
        app.sync_settings.prompt = Some(prompt);
        crate::panels::dialogs::confirm_dialog(&mut app, &Dialog::SettingsImport).unwrap();
        assert_eq!(app.ui.brush_flow, 21.0);
        app.ui.brush_flow = 5.0;
    }
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn the_folder_problems_are_messages() {
    let home = scratch("problems-home");
    let mut app = app_on("B", &home);
    // none chosen
    assert!(app.run("settings.syncSave", json!({})).unwrap_err().contains("No sync folder"));
    assert!(app.run("settings.syncLoad", json!({})).unwrap_err().contains("No sync folder"));
    // chosen, then gone
    let gone = home.join("gone");
    app.ui.sync.folder = gone.to_string_lossy().into_owned();
    assert!(app.run("settings.syncSave", json!({})).unwrap_err().contains("Choose the sync folder again"));
    // chosen, empty
    let empty = home.join("empty");
    std::fs::create_dir(&empty).unwrap();
    app.ui.sync.folder = empty.to_string_lossy().into_owned();
    assert!(app.run("settings.syncLoad", json!({})).unwrap_err().contains("There is no lightcraft.lcsettings"));
    // a damaged file in the folder
    std::fs::write(empty.join(FILE_NAME), "{ damaged").unwrap();
    let before = serde_json::to_value(&app.ui).unwrap();
    assert!(app.run("settings.syncLoad", json!({})).unwrap_err().contains("readable JSON"));
    assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
    assert!(app.ui.dialog.is_none());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn choosing_a_folder_tells_the_other_artcraft_apps_and_keeps_their_keys() {
    let home = scratch("shared-home");
    let sync = scratch("shared-folder");
    let shared = folder::shared_file_in(&home);
    std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
    std::fs::write(&shared, r#"{"path": "/old/folder", "photocraft": {"keep": true}}"#).unwrap();
    let mut app = app_on("A", &home);
    app.ui.sync.last = "2026-11-02T10:00:00+01:00".into();
    chosen(&mut app, &sync);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&shared).unwrap()).unwrap();
    assert_eq!(doc["path"], sync.to_string_lossy().as_ref());
    assert_eq!(doc["photocraft"], json!({"keep": true}));
    assert_eq!(app.ui.sync.last, "", "another folder holds another file");
    // a folder is given by its full path
    assert!(app.run("settings.syncChoose", json!({"path": "relative/folder"})).unwrap_err().contains("full path"));
    assert_eq!(app.ui.sync.folder, sync.to_string_lossy());
    // a Mac with none chosen yet is offered the folder the file names
    let fresh = app_on("B", &home);
    assert_eq!(super::run::offered_folder(&fresh).as_deref(), Some(&*sync.to_string_lossy()));
    // and not once it has chosen
    assert_eq!(super::run::offered_folder(&app), None);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

// ------------------------------------------------------------------ Keep in Sync

fn keep_in_sync(app: &mut LightcraftApp, dir: &Path) {
    chosen(app, dir);
    app.ui.sync.keep = true;
}

fn stamp_file(dir: &Path, computer: &str, at: &str) {
    let by = folder::SavedBy { computer: computer.into(), at: at.into() };
    folder::save(&dir.to_string_lossy(), json!({"kind": "lightcraft-settings", "version": 1, "ui": {"brushFlow": 77.0}}), by).unwrap();
}

fn saved_at(dir: &Path) -> Option<String> {
    folder::read(&dir.to_string_lossy()).ok().and_then(|f| f.by).map(|b| b.at)
}

#[test]
fn keep_in_sync_saves_on_quit_and_not_when_it_is_off() {
    let home = scratch("keep-home");
    let sync = scratch("keep");
    let mut app = app_on("Studio", &home);
    chosen(&mut app, &sync);
    super::save_on_quit(&mut app);
    assert!(!sync.join(FILE_NAME).exists(), "the switch is off: by button only");
    app.ui.sync.keep = true;
    super::save_on_quit(&mut app);
    assert!(sync.join(FILE_NAME).exists());
    assert_eq!(app.ui.sync.last, saved_at(&sync).unwrap(), "the time of the save is noted for ui.json");
    // with no folder chosen it does nothing, and a folder that is gone is only logged
    let mut none = app_on("Studio", &home);
    none.ui.sync.keep = true;
    super::save_on_quit(&mut none);
    app.ui.sync.folder = home.join("gone").to_string_lossy().into_owned();
    super::save_on_quit(&mut app);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn a_newer_file_from_another_mac_is_not_overwritten_on_quit_and_is_offered_at_start() {
    let home = scratch("newer-home");
    let sync = scratch("newer-folder");
    let mut app = app_on("Studio", &home);
    keep_in_sync(&mut app, &sync);
    app.ui.sync.last = "2026-11-02T10:00:00+01:00".into();
    stamp_file(&sync, "Laptop", "2026-11-02T12:00:00+01:00");
    let original = std::fs::read_to_string(sync.join(FILE_NAME)).unwrap();

    // quit: not overwritten
    super::save_on_quit(&mut app);
    assert_eq!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);

    // start: offered as the load prompt
    super::offer_at_start(&mut app);
    assert!(matches!(app.ui.dialog, Some(Dialog::SettingsImport)));
    let prompt = app.sync_settings.prompt.clone().unwrap();
    assert!(prompt.saved_line.unwrap().starts_with("Saved on Laptop, "));
    assert!(prompt.from_folder);

    // Cancel: nothing changed, and the next start offers it again
    app.ui.dialog = None;
    app.sync_settings.prompt = None;
    assert_ne!(app.ui.brush_flow, 77.0);
    app.sync_settings.started = false;
    super::offer_at_start(&mut app);
    assert!(matches!(app.ui.dialog, Some(Dialog::SettingsImport)), "offered again at the next start");

    // OK: loaded, noted, and the next quit saves
    crate::panels::dialogs::confirm_dialog(&mut app, &Dialog::SettingsImport).unwrap();
    assert_eq!(app.ui.brush_flow, 77.0);
    assert_eq!(app.ui.sync.last, "2026-11-02T12:00:00+01:00");
    app.ui.dialog = None;
    super::save_on_quit(&mut app);
    assert_eq!(saved_at(&sync).map(|_| ()), Some(()));
    assert_ne!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);
    assert!(folder::read(&sync.to_string_lossy()).unwrap().by.unwrap().computer == "Studio");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn files_that_are_older_or_from_this_mac_are_not_offered_and_do_not_stop_the_save() {
    let home = scratch("older-home");
    let sync = scratch("older-folder");
    let mut app = app_on("Studio", &home);
    keep_in_sync(&mut app, &sync);
    app.ui.sync.last = "2026-11-02T10:00:00+01:00".into();
    // older, from another Mac
    stamp_file(&sync, "Laptop", "2026-11-01T09:00:00+01:00");
    super::offer_at_start(&mut app);
    assert!(app.ui.dialog.is_none());
    super::save_on_quit(&mut app);
    assert_eq!(folder::read(&sync.to_string_lossy()).unwrap().by.unwrap().computer, "Studio");
    // newer, but this Mac's own
    stamp_file(&sync, "Studio", "2030-01-01T00:00:00+01:00");
    app.sync_settings.started = false;
    super::offer_at_start(&mut app);
    assert!(app.ui.dialog.is_none());
    // no file at all yet: the first quit saves
    let _ = std::fs::remove_file(sync.join(FILE_NAME));
    super::save_on_quit(&mut app);
    assert!(sync.join(FILE_NAME).exists());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn keep_in_sync_offers_nothing_while_it_is_off_and_waits_for_a_free_window() {
    let home = scratch("wait-home");
    let sync = scratch("wait-folder");
    let mut app = app_on("Studio", &home);
    chosen(&mut app, &sync);
    stamp_file(&sync, "Laptop", "2026-11-02T12:00:00+01:00");
    super::offer_at_start(&mut app);
    assert!(app.ui.dialog.is_none(), "the switch is off");
    app.ui.sync.keep = true;
    app.sync_settings.started = false;
    app.ui.dialog = Some(Dialog::About);
    super::offer_at_start(&mut app);
    assert!(matches!(app.ui.dialog, Some(Dialog::About)), "another dialog stays");
    app.ui.dialog = None;
    super::offer_at_start(&mut app);
    assert!(matches!(app.ui.dialog, Some(Dialog::SettingsImport)));
    // a folder that is gone is a message, not a prompt
    let mut gone = app_on("Studio", &home);
    gone.ui.sync.keep = true;
    gone.ui.sync.folder = home.join("gone").to_string_lossy().into_owned();
    super::offer_at_start(&mut gone);
    assert!(gone.ui.dialog.is_none());
    assert!(gone.ui.status.contains("Choose the sync folder again"), "{}", gone.ui.status);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

// ------------------------------------------------------------------ agents

#[test]
fn agents_give_no_path_and_cannot_choose_the_folder() {
    let home = scratch("agent-home");
    let sync = scratch("agent-folder");
    let mut app = app_on("A", &home);
    chosen(&mut app, &sync);
    app.from_control = true;
    for (id, params) in [
        ("settings.export", json!({"path": "/tmp/x.lcsettings"})),
        ("settings.import", json!({"path": "/tmp/x.lcsettings"})),
        ("settings.syncChoose", json!({"path": "/tmp/elsewhere"})),
        ("settings.syncChoose", json!({})),
    ] {
        let e = app.run(id, params.clone()).unwrap_err();
        assert!(e.contains("agents") || e.contains("user's choice"), "{id} {params}: {e}");
    }
    assert!(!Path::new("/tmp/x.lcsettings").exists());
    assert_eq!(app.ui.sync.folder, sync.to_string_lossy(), "the folder is as the user chose it");
    // what they can do: read the settings as text, import text, save and load the user's folder
    let exported = app.run("settings.export", json!({})).unwrap();
    assert_eq!(exported["document"]["kind"], "lightcraft-settings");
    assert!(app.run("settings.import", json!({})).unwrap_err().contains("`text`"));
    app.run("settings.syncSave", json!({})).unwrap();
    let dry = app.run("settings.syncLoad", json!({"dryRun": true})).unwrap();
    assert_eq!(dry["applied"], false);
    assert_eq!(dry["unknownCount"], 0);
    let done = app.run("settings.syncLoad", json!({})).unwrap();
    assert_eq!(done["applied"], true, "an agent's load applies at once");
    assert!(app.ui.dialog.is_none());
    app.from_control = false;
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn ui_set_refuses_the_sync_folder() {
    let home = scratch("uiset-home");
    let mut app = app_on("A", &home);
    app.sync_settings.env.shared_file = Some(folder::shared_file_in(&home));
    let mut h = crate::headless::Headless::new(app, [1200.0, 800.0], 1.0);
    let t = std::time::Duration::from_secs(20);
    let r = h.request("ui.set", json!({"sync": {"folder": "/tmp/elsewhere", "keep": true}}), t);
    assert_eq!(r["ok"], false, "{r}");
    assert!(h.app.ui.sync.folder.is_empty() && !h.app.ui.sync.keep);
    // other keys still work
    let r = h.request("ui.set", json!({"brushFlow": 12.0}), t);
    assert_eq!(r["ok"], true, "{r}");
    let _ = std::fs::remove_dir_all(&home);
}

// ------------------------------------------------------------------ presets

fn user_preset(app: &mut LightcraftApp, name: &str) {
    let preset = lightcraft_develop::Preset {
        id: String::new(),
        name: name.into(),
        group: "User Presets".into(),
        settings: json!({"light": {"exposure": 0.5}}),
        favorite: false,
        builtin: false,
    };
    app.session.add_presets(vec![preset]);
}

#[test]
fn user_presets_travel_when_included_and_are_never_the_built_in_ones() {
    let home = scratch("presets");
    let mut a = app_on("A", &home);
    user_preset(&mut a, "Warm Look");
    assert!(a.session.presets.iter().any(|p| p.builtin), "built-in presets exist");
    assert!(a.ui.sync.presets, "on by default");
    let text = folder::stamped_text(rules::document(&a.ui, rules::presets_part(&a.session.presets)), &a.sync_settings.env.saved_by());
    let doc: Value = serde_json::from_str(&text).unwrap();
    let names: Vec<&str> = doc["presets"]["presets"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
    assert_eq!(names, vec!["Warm Look"]);

    let mut b = app_on("B", &home);
    let before = b.session.presets.len();
    let r = import_text(&mut b, &text).unwrap();
    assert_eq!(r["presets"], 1);
    assert_eq!(b.session.presets.len(), before + 1);
    assert!(b.session.presets.iter().any(|p| p.name == "Warm Look" && !p.builtin));
    // again: an identical preset is not added twice
    import_text(&mut b, &text).unwrap();
    assert_eq!(b.session.presets.len(), before + 1);

    // the option off: the file has none, and an import leaves them out
    let mut c = app_on("C", &home);
    a.ui.sync.presets = false;
    a.from_control = true;
    let r = a.run("settings.export", json!({})).unwrap();
    a.from_control = false;
    assert!(r["document"].get("presets").is_none());
    c.ui.sync.presets = false;
    let before = c.session.presets.len();
    let r = import_text(&mut c, &text).unwrap();
    assert_eq!(r["presets"], 0);
    assert_eq!(c.session.presets.len(), before);
    // a file with a bad presets part is an error
    let bad = json!({"kind": "lightcraft-settings", "version": 1, "presets": {"presets": 5}}).to_string();
    assert!(import_text(&mut b, &bad).unwrap_err().contains("presets"));
    let _ = std::fs::remove_dir_all(&home);
}
