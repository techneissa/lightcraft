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

/// The key each field of `struct_name` has in `ui.json`, read from the struct's source: its name in camelCase (or its
/// `rename`), leaving out fields that are never written (`skip`, `skip_serializing`). Fields that are only left out
/// while empty (`skip_serializing_if`) are in: no state built in a test could set a field the test doesn't know of.
fn written_keys(source: &str, struct_name: &str) -> Vec<String> {
    let start = source.find(&format!("pub struct {struct_name} {{")).expect("the struct is in the source");
    let (mut keys, mut attrs) = (Vec::new(), String::new());
    for line in source[start..].lines().skip(1) {
        let line = line.trim();
        if line == "}" {
            break;
        }
        if line.starts_with("///") || line.starts_with("//") || line.is_empty() {
            continue;
        }
        let Some(field) = line.strip_prefix("pub ").and_then(|r| r.split_once(':')).map(|(name, _)| name.trim().to_string()) else {
            attrs.push_str(line);
            continue;
        };
        let words: Vec<&str> = attrs.split(|c: char| !c.is_alphanumeric() && c != '_').collect();
        let never_written = words.iter().any(|w| *w == "skip" || *w == "skip_serializing");
        let renamed = attrs.split("rename = \"").nth(1).and_then(|r| r.split('"').next()).map(str::to_string);
        if !never_written {
            let camel =
                field.split('_').enumerate().map(|(n, w)| if n == 0 { w.to_string() } else { w[..1].to_uppercase() + &w[1..] }).collect::<String>();
            keys.push(renamed.unwrap_or(camel));
        }
        attrs.clear();
    }
    keys
}

/// A state with every `Option` and every field that is left out while empty filled in.
fn full_state() -> UiState {
    let mut ui = UiState {
        window: Some(crate::window_place::WindowPlace { size: [1200.0, 800.0], zoomed: false }),
        docking: Some(Default::default()),
        show_clipping: Some(true),
        crop_default_aspect: Some(json!("free")),
        renaming_component: Some((1, 2, "x".into())),
        ..Default::default()
    };
    ui.settings.photo_menu = Some(vec!["---".into()]);
    ui.settings.top_bar_hidden = vec!["bell".into()];
    ui
}

#[test]
fn every_ui_key_is_decided() {
    // A key of `ui.json` that is in neither list is a setting nobody decided about: it is synced (the file is
    // built from a deny list), but decide: name it in `SYNCED` for the file, or in `LEFT_OUT` with the reason it
    // belongs to one Mac (a window, a screen, a folder, a hardware choice, what is open right now).
    let decided = |k: &str| rules::left_out(k).is_some() || rules::SYNCED.contains(&k);
    let decided_setting = |k: &str| rules::left_out_setting(k).is_some() || rules::SYNCED_SETTINGS.contains(&k);
    // what a populated state writes
    let ui = serde_json::to_value(full_state()).unwrap();
    let undecided: Vec<&String> = ui.as_object().unwrap().keys().filter(|k| !decided(k)).collect();
    assert!(undecided.is_empty(), "ui.json keys that are neither synced nor left out (add them to settings_sync::rules): {undecided:?}");
    let undecided: Vec<&String> = ui["settings"].as_object().unwrap().keys().filter(|k| !decided_setting(k)).collect();
    assert!(undecided.is_empty(), "settings keys that are neither synced nor left out (add them to settings_sync::rules): {undecided:?}");
    // and every field of the two structs, whatever it holds
    let top = written_keys(include_str!("../state.rs"), "UiState");
    let settings = written_keys(include_str!("../state/settings.rs"), "AppSettings");
    assert!(top.len() > 60 && settings.len() > 30, "the scan found {} and {} fields", top.len(), settings.len());
    let undecided: Vec<&String> = top.iter().filter(|k| !decided(k)).collect();
    assert!(undecided.is_empty(), "fields of UiState that are neither synced nor left out (add them to settings_sync::rules): {undecided:?}");
    let undecided: Vec<&String> = settings.iter().filter(|k| !decided_setting(k)).collect();
    assert!(undecided.is_empty(), "fields of AppSettings that are neither synced nor left out (add them to settings_sync::rules): {undecided:?}");
    // the lists name fields that exist
    for name in rules::LEFT_OUT.iter().map(|(k, _)| *k).chain(rules::SYNCED.iter().copied()) {
        assert!(top.iter().any(|k| k == name), "`{name}` is not a field of UiState");
    }
    for name in rules::LEFT_OUT_SETTINGS.iter().map(|(k, _)| *k).chain(rules::SYNCED_SETTINGS.iter().copied()) {
        assert!(settings.iter().any(|k| k == name), "`{name}` is not a field of AppSettings");
    }
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
    let p = plan::plan(&text, Some(&[])).unwrap();
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
    assert_eq!(unknown_line(&[], 0, true), None);
    assert_eq!(unknown_line(&names[..3], 3, true).unwrap(), "3 settings from a newer LightCraft were skipped: ui.s1, ui.s2, ui.s3");
    assert_eq!(unknown_line(&names[..1], 1, true).unwrap(), "1 setting from a newer LightCraft was skipped: ui.s1");
    assert_eq!(
        unknown_line(&names, 8, true).unwrap(),
        "8 settings from a newer LightCraft were skipped: ui.s1, ui.s2, ui.s3, ui.s4, ui.s5, and 3 more"
    );
    // a file of this version that holds a left-out key is not "from a newer LightCraft"
    assert_eq!(unknown_line(&names[..2], 2, false).unwrap(), "2 settings were skipped: ui.s1, ui.s2");
    assert_eq!(unknown_line(&names[..1], 1, false).unwrap(), "1 setting was skipped: ui.s1");
    // the hover has all of them, and says when the list itself was cut
    assert_eq!(unknown_list(&names, 8).lines().count(), 8);
    assert!(unknown_list(&names, 250).ends_with("…and 242 more"));
}

#[test]
fn a_left_out_key_in_a_file_of_this_version_is_skipped_and_named_without_a_newer_lightcraft() {
    let home = scratch("left-out-key");
    let mut app = app_on("B", &home);
    let text = json!({"kind": "lightcraft-settings", "version": 1, "ui": {"leftWidth": 999, "brushFlow": 22.0, "settings": {"libraryPath": "/x"}}})
        .to_string();
    let p = plan::plan(&text, None).unwrap();
    assert_eq!(p.newer, None);
    assert_eq!(p.unknown, vec!["ui.leftWidth".to_string(), "ui.settings.libraryPath".to_string()]);
    let line = super::prompt::unknown_line(&p.unknown, p.unknown_total, p.newer.is_some()).unwrap();
    assert!(!line.contains("newer"), "{line}");
    import_text(&mut app, &text).unwrap();
    assert_eq!(app.ui.brush_flow, 22.0);
    assert_eq!(app.ui.left_width, UiState::default().left_width, "left out means left out");
    let _ = std::fs::remove_dir_all(&home);
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
        // read leniently from the app's own ui.json (a bad value is the default there), never from a file
        (json!({"settings": {"atLaunch": "whenever"}}), "atLaunch"),
        (json!({"settings": {"atLaunch": 3}}), "atLaunch"),
        (json!({"settings": {"deleteFiles": "shred"}}), "deleteFiles"),
        (json!({"settings": {"keywordChips": "sparkles"}}), "keywordChips"),
        (json!({"settings": {"loupeInfo": "all of it"}}), "loupeInfo"),
        (json!({"settings": {"lightroomImport": "sometimes"}}), "lightroomImport"),
        (json!({"settings": {"photoMenuPresets": [1, 2]}}), "photoMenuPresets"),
        (json!({"settings": {"topBarHidden": "bell"}}), "topBarHidden"),
        (json!({"settings": {"topBarHidden": ["bell", 7]}}), "topBarHidden"),
        (json!({"settings": {"photoMenu": "---"}}), "photoMenu"),
        (json!({"foldedSections": "keywording"}), "foldedSections"),
        (json!({"foldedSections": ["keywording", false]}), "foldedSections"),
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
    folder::SavedBy { computer: "x".into(), at: at.into(), id: None }.when().is_some() && at.len() >= 20
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
    let by = folder::SavedBy { computer: computer.into(), at: at.into(), id: None };
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

fn make_preset(name: &str, exposure: f64) -> lightcraft_develop::Preset {
    lightcraft_develop::Preset {
        id: String::new(),
        name: name.into(),
        group: "User Presets".into(),
        settings: json!({"light": {"exposure": exposure}}),
        favorite: false,
        builtin: false,
    }
}

fn user_preset(app: &mut LightcraftApp, name: &str) {
    app.session.add_presets(vec![make_preset(name, 0.5)]);
}

fn with_presets(app: &LightcraftApp) -> String {
    let by = folder::SavedBy { computer: "A".into(), at: "2026-11-02T10:00:00Z".into(), id: None };
    folder::stamped_text(rules::document(&app.ui, rules::presets_part(&app.session.presets)), &by)
}

fn user_presets(app: &LightcraftApp) -> Vec<(String, Value)> {
    app.session.presets.iter().filter(|p| !p.builtin).map(|p| (p.name.clone(), p.settings.clone())).collect()
}

#[test]
fn user_presets_travel_when_included_and_are_never_the_built_in_ones() {
    let home = scratch("presets");
    let mut a = app_on("A", &home);
    user_preset(&mut a, "Warm Look");
    assert!(a.session.presets.iter().any(|p| p.builtin), "built-in presets exist");
    assert!(a.ui.sync.presets, "on by default");
    let text = with_presets(&a);
    let doc: Value = serde_json::from_str(&text).unwrap();
    let names: Vec<&str> = doc["presets"]["presets"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
    assert_eq!(names, vec!["Warm Look"]);

    let mut b = app_on("B", &home);
    let before = b.session.presets.len();
    let r = import_text(&mut b, &text).unwrap();
    assert_eq!((r["presetsAdded"].clone(), r["presetsReplaced"].clone()), (json!(1), json!(0)));
    assert_eq!(b.session.presets.len(), before + 1);
    assert!(b.session.presets.iter().any(|p| p.name == "Warm Look" && !p.builtin));
    // again: an identical preset changes nothing and the plan says so
    let r = import_text(&mut b, &text).unwrap();
    assert_eq!((r["presets"].clone(), r["presetsAdded"].clone(), r["presetsReplaced"].clone()), (json!(0), json!(0), json!(0)));
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

#[test]
fn a_preset_of_the_same_name_with_other_settings_replaces_the_one_here() {
    let home = scratch("presets-replace");
    let mut a = app_on("A", &home);
    a.session.add_presets(vec![make_preset("Warm Look", 0.9), make_preset("New Look", 0.1), make_preset("Same Look", 0.3)]);
    let text = with_presets(&a);
    let mut b = app_on("B", &home);
    b.session.add_presets(vec![make_preset("Warm Look", 0.5), make_preset("Same Look", 0.3), make_preset("Mine Only", 0.7)]);
    let ids_before: Vec<String> = b.session.presets.iter().filter(|p| p.name == "Warm Look").map(|p| p.id.clone()).collect();

    // the prompt counts only what would change: one added, one replaced, the identical one not at all
    let r = b.run("settings.import", json!({"text": text, "prompt": true})).unwrap();
    assert_eq!(r["dialog"], true);
    let prompt = b.sync_settings.prompt.clone().unwrap();
    assert_eq!((prompt.plan.presets_added, prompt.plan.presets_replaced), (1, 1));
    assert_eq!(super::prompt::presets_line(&prompt.plan).unwrap(), "Presets: 1 added, 1 replaced.");
    // nothing changes before OK
    assert_eq!(user_presets(&b).iter().find(|(n, _)| n == "Warm Look").unwrap().1["light"]["exposure"], 0.5);
    crate::panels::dialogs::confirm_dialog(&mut b, &Dialog::SettingsImport).unwrap();

    let mine = user_presets(&b);
    assert_eq!(mine.iter().filter(|(n, _)| n == "Warm Look").count(), 1, "replaced, not added beside it");
    assert_eq!(mine.iter().find(|(n, _)| n == "Warm Look").unwrap().1["light"]["exposure"], 0.9);
    let ids_after: Vec<String> = b.session.presets.iter().filter(|p| p.name == "Warm Look").map(|p| p.id.clone()).collect();
    assert_eq!(ids_before, ids_after, "the preset keeps its place and id");
    assert!(mine.iter().any(|(n, _)| n == "New Look") && mine.iter().any(|(n, _)| n == "Mine Only"));
    assert_eq!(mine.iter().filter(|(n, _)| n == "Same Look").count(), 1);
    assert_eq!(super::prompt::presets_line(&plan::Plan { presets_added: 2, ..Default::default() }).unwrap(), "Presets: 2 added.");
    assert_eq!(super::prompt::presets_line(&plan::Plan { presets_replaced: 3, ..Default::default() }).unwrap(), "Presets: 3 replaced.");
    assert_eq!(super::prompt::presets_line(&plan::Plan::default()), None);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_prompt_lists_only_the_parts_the_file_has() {
    let home = scratch("areas");
    let line = |ui: Value| {
        let text = json!({"kind": "lightcraft-settings", "version": 1, "ui": ui}).to_string();
        super::prompt::replaces_line(&plan::plan(&text, None).unwrap())
    };
    assert_eq!(line(json!({"settings": {"keymap": {}}})).unwrap(), "This replaces your keyboard shortcuts.");
    assert_eq!(
        line(json!({"language": "en", "settings": {"keymap": {}, "darkTheme": "midnight"}, "sidebar": []})).unwrap(),
        "This replaces your appearance, keyboard shortcuts and arrangements."
    );
    assert_eq!(
        line(json!({"brushSize": 0.1, "removeSize": 0.1, "settings": {"topBarHidden": []}})).unwrap(),
        "This replaces your arrangements and tool defaults."
    );
    assert_eq!(line(json!({"settings": {"dimRejected": true}})).unwrap(), "This replaces your other settings.");
    assert_eq!(line(json!({})), None);
    // a file with presets only has no settings line
    let only =
        json!({"kind": "lightcraft-settings", "version": 1, "presets": {"format": "lightcraft.preset", "version": 1, "presets": []}}).to_string();
    assert_eq!(super::prompt::replaces_line(&plan::plan(&only, Some(&[])).unwrap()), None);
    let _ = std::fs::remove_dir_all(&home);
}

// ------------------------------------------------------------------ Save over newer settings

fn put_file(dir: &Path, computer: &str, at: &str, id: Option<&str>) {
    let by = folder::SavedBy { computer: computer.into(), at: at.into(), id: id.map(str::to_string) };
    folder::save(&dir.to_string_lossy(), json!({"kind": "lightcraft-settings", "version": 1, "ui": {"brushFlow": 66.0}}), by).unwrap();
}

#[test]
fn save_to_folder_asks_before_replacing_newer_settings_from_another_mac() {
    let home = scratch("ask-home");
    let sync = scratch("ask-folder");
    let mut app = app_on("Studio", &home);
    chosen(&mut app, &sync);
    app.ui.sync.last = "2026-11-02T10:00:00+01:00".into();
    put_file(&sync, "Laptop", "2026-11-02T12:00:00+01:00", None);
    let original = std::fs::read_to_string(sync.join(FILE_NAME)).unwrap();

    // a person: asked, in words, and nothing is written yet
    let r = app.run("settings.syncSave", json!({})).unwrap();
    assert_eq!(r["dialog"], true);
    assert!(matches!(app.ui.dialog, Some(Dialog::SettingsReplace)));
    let question = app.sync_settings.replace.clone().unwrap();
    assert!(question.starts_with("The folder has newer settings from Laptop, ") && question.ends_with(". Replace them?"), "{question}");
    assert!(question.contains("2026"), "{question}");
    assert_eq!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);
    // Cancel: still nothing. Replace: saved, and noted
    app.ui.dialog = None;
    assert_eq!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);
    app.run("settings.syncSave", json!({})).unwrap();
    crate::panels::dialogs::confirm_dialog(&mut app, &Dialog::SettingsReplace).unwrap();
    assert_ne!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);
    let by = folder::read(&sync.to_string_lossy()).unwrap().by.unwrap();
    assert_eq!(by.computer, "Studio");
    assert_eq!(app.ui.sync.last, by.at);
    // it is this Mac's own file now: no question
    app.ui.dialog = None;
    let r = app.run("settings.syncSave", json!({})).unwrap();
    assert!(r.get("dialog").is_none(), "{r}");
    assert!(app.ui.dialog.is_none());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn an_agent_saving_over_newer_settings_gets_an_error_unless_it_says_replace() {
    let home = scratch("agent-ask-home");
    let sync = scratch("agent-ask-folder");
    let mut app = app_on("Studio", &home);
    chosen(&mut app, &sync);
    put_file(&sync, "Laptop", "2026-11-02T12:00:00+01:00", None);
    let original = std::fs::read_to_string(sync.join(FILE_NAME)).unwrap();
    app.from_control = true;
    let e = app.run("settings.syncSave", json!({})).unwrap_err();
    assert!(e.contains("Replace them?") && e.contains("\"replace\": true"), "{e}");
    assert!(app.run("settings.syncSave", json!({"replace": false})).is_err());
    assert_eq!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);
    assert!(app.ui.dialog.is_none(), "an agent gets no dialog");
    let r = app.run("settings.syncSave", json!({"replace": true})).unwrap();
    assert_eq!(r["savedBy"]["computer"], "Studio");
    assert_ne!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), original);
    app.from_control = false;
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn a_file_that_cannot_be_read_as_to_who_or_when_is_treated_as_newer() {
    let home = scratch("unknown-home");
    let sync = scratch("unknown-folder");
    let mut app = app_on("Studio", &home);
    chosen(&mut app, &sync);
    app.ui.sync.keep = true;
    app.ui.sync.last = "2026-11-02T10:00:00+01:00".into();
    for (text, expect) in [
        (r#"{"kind": "lightcraft-settings", "version": 1}"#, "don't say which Mac"),
        (r#"{"kind": "lightcraft-settings", "version": 1, "savedBy": {"computer": 5}}"#, "don't say which Mac"),
        (r#"{"kind": "lightcraft-settings", "version": 1, "savedBy": {"computer": "Laptop", "at": "last week"}}"#, "can't be read"),
        ("{ damaged", "don't say which Mac"),
    ] {
        std::fs::write(sync.join(FILE_NAME), text).unwrap();
        // Save asks (an agent is refused)
        let r = app.run("settings.syncSave", json!({})).unwrap();
        assert_eq!(r["dialog"], true, "{text}");
        assert!(app.sync_settings.replace.as_ref().unwrap().contains(expect), "{text}: {:?}", app.sync_settings.replace);
        app.ui.dialog = None;
        app.from_control = true;
        assert!(app.run("settings.syncSave", json!({})).is_err(), "{text}");
        app.from_control = false;
        // quitting does not overwrite it
        super::save_on_quit(&mut app);
        assert_eq!(std::fs::read_to_string(sync.join(FILE_NAME)).unwrap(), text, "quit replaced {text}");
        // and it is never offered at start (it would be, at every start)
        app.sync_settings.started = false;
        super::offer_at_start(&mut app);
        assert!(app.ui.dialog.is_none(), "{text}");
    }
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn two_macs_with_one_name_are_told_apart_by_the_id_this_mac_keeps() {
    let home = scratch("ids-home");
    let sync = scratch("ids-folder");
    let mut a = app_on("MacBook", &home);
    let mut b = app_on("MacBook", &home);
    chosen(&mut a, &sync);
    chosen(&mut b, &sync);
    assert!(a.ui.sync.id.is_empty(), "made when first needed");
    a.run("settings.syncSave", json!({})).unwrap();
    let id_a = a.ui.sync.id.clone();
    assert_eq!(id_a.len(), 32);
    let saved = folder::read(&sync.to_string_lossy()).unwrap().by.unwrap();
    assert_eq!(saved.id.as_deref(), Some(id_a.as_str()));
    // kept: a second save uses the same id, and it is in the settings that ui.json holds
    a.run("settings.syncSave", json!({})).unwrap();
    assert_eq!(a.ui.sync.id, id_a);
    assert_eq!(serde_json::to_value(&a.ui).unwrap()["sync"]["id"], id_a.as_str());
    // B has the same name, and has not loaded A's file: it is another Mac
    let r = b.run("settings.syncSave", json!({})).unwrap();
    assert_eq!(r["dialog"], true, "same name, other id: asked");
    assert_ne!(b.ui.sync.id, id_a);
    b.ui.dialog = None;
    b.ui.sync.keep = true;
    super::save_on_quit(&mut b);
    assert_eq!(folder::read(&sync.to_string_lossy()).unwrap().by.unwrap().id.as_deref(), Some(id_a.as_str()), "quit did not replace it");
    // B loads it (now it has the file): saving is fine, and A is then the one that is asked
    b.run("settings.syncLoad", json!({})).unwrap();
    crate::panels::dialogs::confirm_dialog(&mut b, &Dialog::SettingsImport).unwrap();
    b.ui.dialog = None;
    let r = b.run("settings.syncSave", json!({})).unwrap();
    assert!(r.get("dialog").is_none(), "{r}");
    // (times are to the second: say A's last save was well before B's)
    a.ui.sync.last = "2026-01-01T00:00:00+00:00".into();
    let r = a.run("settings.syncSave", json!({})).unwrap();
    assert_eq!(r["dialog"], true, "B saved after A's last save");
    // a file with no id (PhotoCraft's kind, or an older one) is told apart by name
    put_file(&sync, "MacBook", "2030-01-01T00:00:00+01:00", None);
    a.ui.dialog = None;
    let r = a.run("settings.syncSave", json!({})).unwrap();
    assert!(r.get("dialog").is_none(), "the same name: this Mac's own: {r}");
    put_file(&sync, "Other Mac", "2030-01-01T00:00:00+01:00", None);
    let r = a.run("settings.syncSave", json!({})).unwrap();
    assert_eq!(r["dialog"], true);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

// ------------------------------------------------------------------ small things

#[test]
fn a_leftover_temporary_file_goes_at_start_and_nothing_else() {
    let home = scratch("leftover-home");
    let sync = scratch("leftover-folder");
    let mut app = app_on("Studio", &home);
    chosen(&mut app, &sync);
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for (name, aged) in [(".lightcraft.lcsettings.77-0.tmp", true), (".photocraft.pcsettings.77-0.tmp", true), ("mine.tmp", true), (FILE_NAME, true)]
    {
        let path = sync.join(name);
        std::fs::write(&path, "x").unwrap();
        if aged {
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        }
    }
    super::offer_at_start(&mut app);
    assert!(!sync.join(".lightcraft.lcsettings.77-0.tmp").exists());
    for kept in [".photocraft.pcsettings.77-0.tmp", "mine.tmp", FILE_NAME] {
        assert!(sync.join(kept).exists(), "{kept} was removed");
    }
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&sync);
}

#[test]
fn when_applying_fails_the_prompt_stays_with_the_real_error() {
    let home = scratch("fail-home");
    let app = app_on("B", &home);
    let mut h = crate::headless::Headless::new(app, [1200.0, 800.0], 1.0);
    let t = std::time::Duration::from_secs(20);
    // a plan that can't be read back as settings (what a bug or a race could hand over)
    let mut bad = plan::Plan::default();
    bad.ui.insert("brushSize".into(), json!("not a number"));
    h.app.sync_settings.prompt = Some(super::Prompt { plan: bad, name: "x.lcsettings".into(), saved_line: None, saved_at: None, from_folder: false });
    h.app.ui.dialog = Some(Dialog::SettingsImport);
    let before = serde_json::to_value(&h.app.ui.settings).unwrap();
    let r = h.request("ui.dialog.confirm", json!({}), t);
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("can't be used"), "{r}");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SettingsImport)), "the dialog stays usable");
    assert!(h.app.sync_settings.prompt.is_some(), "so does the prompt");
    assert_eq!(serde_json::to_value(&h.app.ui.settings).unwrap(), before);
    let r = h.request("ui.dialog.cancel", json!({}), t);
    assert_eq!(r["ok"], true, "{r}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_deny_list_leaves_out_every_open_or_visible_flag() {
    for key in ["openSections", "navigator", "filterBar", "filmstrip", "presets", "histogram", "leftPanel", "right"] {
        assert!(rules::left_out(key).is_some(), "{key} belongs to one screen");
    }
}
