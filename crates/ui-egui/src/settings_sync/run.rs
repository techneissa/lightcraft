//! The commands of Settings sync, the folder, and Keep in Sync (desktop only).
//!
//! - `settings.export {path?}` / `settings.import {path?, text?, dryRun?, prompt?}`: one file.
//! - `settings.syncSave` / `settings.syncLoad {dryRun?}`: the file in the sync folder, which is
//!   `ui.sync.folder`: no command takes a folder, and `settings.syncChoose {path}` (the Choose…
//!   button) is for a person only, as is a `path`: an agent gets the settings as text (`settings.export`
//!   replies `document`, `settings.import` takes `text`) and the folder as the user chose it.
//!
//! Called by a person without parameters, Export and Import ask for the file, Save writes at once and
//! Load shows the prompt; with parameters, or from an agent, the engine does the work.

use std::path::Path;

use serde_json::{Value, json};

use lightcraft_engine::settings_folder::{self as folder, EXTENSION, FolderFile, SavedBy};

use super::{Plan, Prompt, plan, rules};
use crate::LightcraftApp;
use crate::pick::{PickRequest, Picked};
use crate::state::Dialog;

/// The file name a save dialog suggests.
const SUGGESTED: &str = "LightCraft Settings.lcsettings";

/// `path` with the home folder written `~`, as the page shows it.
pub(crate) fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.strip_prefix(home.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/')) => {
            format!("~{}", path.get(home.len()..).unwrap_or(""))
        }
        _ => path.to_string(),
    }
}

/// The settings file's JSON now (user presets when Include User Presets is on).
fn document_now(app: &LightcraftApp) -> Value {
    let presets = if app.ui.sync.presets { rules::presets_part(&app.session.presets) } else { None };
    rules::document(&app.ui, presets)
}

fn dry_run(p: &Value) -> bool {
    p.get("dryRun").and_then(Value::as_bool) == Some(true)
}

fn has(p: &Value, key: &str) -> bool {
    p.get(key).is_some_and(|v| !v.is_null())
}

/// A `path` is for a person: an agent can't point a command at a file.
fn refuse_path(app: &LightcraftApp, id: &str, p: &Value) -> Result<(), String> {
    if app.from_control && has(p, "path") {
        return Err(format!("{id}: agents can't give a path (settings.export replies the settings as `document`; settings.import takes `text`)"));
    }
    Ok(())
}

/// By a person, without a file given: ask for one, and come back with it.
enum Chosen {
    Path(String),
    /// The dialog is up (or was cancelled): nothing more to do now.
    Later,
}

fn choose_file(app: &mut LightcraftApp, id: &str, p: &Value, request: PickRequest, then: Value) -> Result<Chosen, String> {
    if let Some(path) = p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        return Ok(Chosen::Path(path.to_string()));
    }
    match crate::pick::ask(app, id, &then, "path", request, |_| None) {
        Picked::Now(paths) => Ok(paths.into_iter().next().map_or(Chosen::Later, Chosen::Path)),
        Picked::Later => Ok(Chosen::Later),
        Picked::Unavailable => Err(format!("{id}: there is no file dialog on this platform; give `path`")),
    }
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

fn toast(app: &mut LightcraftApp, text: String) {
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    app.toast(&ctx, text);
}

/// Dispatch the commands of Settings sync.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    match id {
        "settings.export" => export(app, p),
        "settings.import" => import(app, p),
        "settings.syncSave" => sync_save(app),
        "settings.syncLoad" => sync_load(app, p),
        "settings.syncChoose" => sync_choose(app, p),
        _ => Err(format!("unknown command `{id}`")),
    }
}

// ------------------------------------------------------------------ one file

fn export(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let id = "settings.export";
    refuse_path(app, id, p)?;
    let doc = document_now(app);
    if app.from_control {
        return Ok(json!({"document": doc}));
    }
    let request = PickRequest::save(crate::i18n::tr("Export Settings"), crate::i18n::tr("LightCraft Settings"), &[EXTENSION], SUGGESTED);
    let path = match choose_file(app, id, p, request, json!({}))? {
        Chosen::Path(path) => path,
        Chosen::Later => return Ok(Value::Null),
    };
    let text = folder::stamped_text(doc, &app.sync_settings.env.saved_by());
    folder::write_atomic(Path::new(&path), &text)?;
    toast(app, crate::i18n::tr_format!("Exported the settings to {name}", name = file_name(&path)));
    Ok(json!({"path": path}))
}

fn import(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let id = "settings.import";
    refuse_path(app, id, p)?;
    let by_person = !app.from_control;
    let given = has(p, "path") || has(p, "text");
    let prompting = by_person && (p.get("prompt").and_then(Value::as_bool) == Some(true) || !given);
    let (text, name) = match p.get("text").and_then(Value::as_str) {
        Some(text) => (text.to_string(), "settings".to_string()),
        None if !by_person && !has(p, "path") => return Err(format!("{id}: give the settings as `text`")),
        None => {
            let request = PickRequest::file(crate::i18n::tr("Import Settings"), crate::i18n::tr("LightCraft Settings"), &[EXTENSION]);
            let path = match choose_file(app, id, p, request, json!({"prompt": true}))? {
                Chosen::Path(path) => path,
                Chosen::Later => return Ok(Value::Null),
            };
            let text = folder::read_text(Path::new(&path), folder::MAX_BYTES)?;
            (text, file_name(&path))
        }
    };
    let by = folder::saved_by(&text);
    let plan = plan(&text, app.ui.sync.presets)?;
    if prompting {
        open_prompt(app, name, plan, by.as_ref(), false);
        return Ok(json!({"dialog": true}));
    }
    finish(app, plan, dry_run(p))
}

/// Report `plan`, applying it unless `dry`.
fn finish(app: &mut LightcraftApp, plan: Plan, dry: bool) -> Result<Value, String> {
    let mut report = plan.report(!dry);
    if !dry {
        let added = super::apply(app, plan)?;
        report["presetsAdded"] = json!(added.len());
    }
    Ok(report)
}

fn open_prompt(app: &mut LightcraftApp, name: String, plan: Plan, by: Option<&SavedBy>, from_folder: bool) {
    let saved_line = by.map(|by| crate::i18n::tr_format!("Saved on {computer}, {when}", computer = by.computer, when = by.when_text()));
    let saved_at = by.filter(|_| from_folder).map(|by| by.at.clone());
    app.sync_settings.prompt = Some(Prompt { plan, name, saved_line, saved_at, from_folder });
    app.ui.dialog = Some(Dialog::SettingsImport);
}

// ------------------------------------------------------------------ the folder

/// Save to Folder: the settings now in the sync folder.
pub(crate) fn save_now(app: &mut LightcraftApp) -> Result<folder::Saved, String> {
    let saved = folder::save(&app.ui.sync.folder, document_now(app), app.sync_settings.env.saved_by())?;
    app.ui.sync.last.clone_from(&saved.by.at);
    Ok(saved)
}

fn sync_save(app: &mut LightcraftApp) -> Result<Value, String> {
    let saved = save_now(app)?;
    if !app.from_control {
        toast(app, crate::i18n::tr_format!("Saved the settings to {path}", path = tilde(&saved.path.to_string_lossy())));
    }
    Ok(json!({"file": saved.path, "savedBy": saved.by}))
}

fn sync_load(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let file = folder::read(&app.ui.sync.folder)?;
    let plan = plan(&file.text, app.ui.sync.presets)?;
    let prompting = !app.from_control && (p.get("prompt").and_then(Value::as_bool) == Some(true) || p.as_object().is_none_or(|m| m.is_empty()));
    let (path, by) = (file.path.clone(), file.by.clone());
    if prompting {
        open_prompt(app, folder::FILE_NAME.to_string(), plan, by.as_ref(), true);
        return Ok(json!({"dialog": true, "file": path, "savedBy": by}));
    }
    let dry = dry_run(p);
    let mut report = finish(app, plan, dry)?;
    if !dry && let Some(by) = &by {
        app.ui.sync.last.clone_from(&by.at);
    }
    report["file"] = json!(path);
    report["savedBy"] = json!(by);
    Ok(report)
}

/// Choose… (`settings.syncChoose {path}`): a person's choice, remembered here and told to the other ArtCraft apps.
fn sync_choose(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let id = "settings.syncChoose";
    if app.from_control {
        return Err(format!("{id}: the sync folder is the user's choice, made in Settings ▸ Sync"));
    }
    let request = PickRequest::folder(crate::i18n::tr("Choose Sync Folder"));
    let path = match p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(path) => path.to_string(),
        None => match crate::pick::ask(app, id, p, "path", request, |s| s.pick_folder.as_mut().map(|pick| pick().into_iter().collect())) {
            Picked::Now(paths) => match paths.into_iter().next() {
                Some(path) => path,
                None => return Ok(Value::Null),
            },
            Picked::Later => return Ok(Value::Null),
            Picked::Unavailable => return Err(format!("{id}: there is no folder dialog on this platform; give `path`")),
        },
    };
    choose_folder(app, &path)?;
    Ok(json!({"folder": path}))
}

/// Use `chosen`: this Mac's choice, and the shared file the other ArtCraft apps read. The folder
/// counts even when the shared file can't be written; that is the error returned.
pub(crate) fn choose_folder(app: &mut LightcraftApp, chosen: &str) -> Result<(), String> {
    let chosen = chosen.trim();
    if app.ui.sync.folder == chosen {
        return Ok(());
    }
    app.ui.sync.folder = chosen.to_string();
    // a different folder holds a different file
    app.ui.sync.last.clear();
    match (chosen.is_empty(), app.sync_settings.env.shared_file()) {
        (false, Some(file)) => {
            folder::write_shared(&file, chosen).map_err(|e| format!("The folder is chosen, but the other ArtCraft apps could not be told: {e}"))
        }
        _ => Ok(()),
    }
}

/// The folder the shared file names, when this Mac has none chosen yet and the file names one.
pub(crate) fn offered_folder(app: &LightcraftApp) -> Option<String> {
    if !app.ui.sync.folder.is_empty() {
        return None;
    }
    folder::read_shared(&app.sync_settings.env.shared_file()?)
}

// ------------------------------------------------------------------ Keep in Sync

fn offer(app: &mut LightcraftApp, file: &FolderFile) {
    match plan(&file.text, app.ui.sync.presets) {
        Ok(plan) => {
            let by = file.by.as_ref();
            let saved_line = by.map(|by| crate::i18n::tr_format!("Saved on {computer}, {when}", computer = by.computer, when = by.when_text()));
            let saved_at = by.map(|by| by.at.clone());
            app.sync_settings.offer = Some(Prompt { plan, name: folder::FILE_NAME.to_string(), saved_line, saved_at, from_folder: true });
        }
        Err(e) => log::warn!("Settings sync: the file in the sync folder can't be used: {e}"),
    }
}

/// Each frame: at start, with Keep in Sync on, a file in the folder that another Mac saved after this
/// Mac last saved or loaded is put to the user as the load prompt, once no other dialog is open.
/// Cancel offers it again at the next start. A folder that can't be read is a message in the status bar.
pub fn offer_at_start(app: &mut LightcraftApp) {
    if !app.sync_settings.started {
        app.sync_settings.started = true;
        let sync = app.ui.sync.clone();
        if sync.keep && !sync.folder.is_empty() {
            match folder::newer_file(&sync.folder, &app.sync_settings.env.computer(), &sync.last) {
                Ok(Some(file)) => offer(app, &file),
                Ok(None) => {}
                Err(e) => app.ui.status = e,
            }
        }
    }
    if app.sync_settings.offer.is_some() && app.ui.dialog.is_none() {
        app.sync_settings.prompt = app.sync_settings.offer.take();
        app.ui.dialog = Some(Dialog::SettingsImport);
    }
}

/// On quit, with Keep in Sync on: save to the folder, unless it holds a newer file from another Mac
/// that this Mac hasn't loaded (saving would replace it unseen). Call before the settings are saved:
/// it notes the time of the save in them.
pub fn save_on_quit(app: &mut LightcraftApp) {
    let sync = app.ui.sync.clone();
    if !sync.keep || sync.folder.is_empty() {
        return;
    }
    match folder::newer_file(&sync.folder, &app.sync_settings.env.computer(), &sync.last) {
        Ok(None) => {}
        Ok(Some(_)) => return,
        Err(e) => return log::warn!("Settings sync: nothing was saved to the sync folder on quit: {e}"),
    }
    if let Err(e) = save_now(app) {
        log::warn!("Settings sync: nothing was saved to the sync folder on quit: {e}");
    }
}
