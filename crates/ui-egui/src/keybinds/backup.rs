//! Export and Import of the keyboard shortcuts as a JSON file, a backup you can keep or move to
//! another Mac. The file lists every customised command with its two shortcuts (a chord is
//! `Cmd+K C`) and its When condition; what is absent stays at its declared value. A file that is
//! not ours, or newer than this build, is refused with a clear message; entries this build can't
//! use (an unknown command, a shortcut or condition that doesn't parse) are skipped with a notice,
//! the rest is taken. `app.exportShortcuts {path}` and `app.importShortcuts {path}` are the commands;
//! the buttons of the Keyboard Shortcuts window call them after the save / open panel.

use serde_json::{Map, Value, json};

use crate::LightcraftApp;
use crate::shortcuts::{self, Keymap, RESERVED, find_bindable};

use super::chord;
use super::model::{self, conditions};

/// What the file says it is.
pub const KIND: &str = "lightcraft-shortcuts";
/// The version this build writes and the newest it reads.
pub const VERSION: u64 = 1;
/// The biggest file read (a keymap is a few kilobytes).
const MAX_BYTES: usize = 4_000_000;

/// A keymap as the file: every customised command with the parts the user changed (`null` = the
/// shortcut was removed, or the condition set to "always").
pub fn to_json(keymap: &Keymap) -> Value {
    let mut commands: std::collections::BTreeMap<String, Map<String, Value>> = Default::default();
    for (key, text) in keymap {
        let id = conditions::command_of(key);
        if find_bindable(id).is_none() {
            continue;
        }
        let field = if key.ends_with(conditions::SUFFIX) {
            "when"
        } else if model::split_key(key).1 > 0 {
            "shortcut2"
        } else {
            "shortcut"
        };
        let value = if text.is_empty() { Value::Null } else { json!(text) };
        commands.entry(id.to_string()).or_default().insert(field.to_string(), value);
    }
    json!({"kind": KIND, "version": VERSION, "app": "LightCraft", "shortcuts": commands})
}

/// What an import produced: the new keymap, how many commands it set, and what it skipped.
#[derive(Debug)]
pub struct Imported {
    pub keymap: Keymap,
    pub commands: usize,
    pub notices: Vec<String>,
}

fn name(id: &str) -> String {
    find_bindable(id).map_or(id.to_string(), |b| format!("{} ({id})", crate::i18n::tr(b.label)))
}

/// Read a shortcuts file over `current` (the keymap now): every customisation of a command this
/// build knows is replaced by the file's; entries for commands it doesn't know are kept.
pub fn from_text(text: &str, current: &Keymap) -> Result<Imported, String> {
    if text.len() > MAX_BYTES {
        return Err(crate::i18n::tr("This file is too big to be a shortcuts file").to_string());
    }
    let doc: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| crate::i18n::tr_format!("This isn't a LightCraft shortcuts file: it isn't readable JSON ({})", e))?;
    match doc.get("kind").and_then(Value::as_str) {
        Some(KIND) => {}
        Some(other) => return Err(crate::i18n::tr_format!("This isn't a LightCraft shortcuts file (its kind is “{}”)", other)),
        None => return Err(crate::i18n::tr("This isn't a LightCraft shortcuts file (it has no \"kind\")").to_string()),
    }
    match doc.get("version").and_then(Value::as_u64) {
        Some(v) if (1..=VERSION).contains(&v) => {}
        Some(v) if v > VERSION => {
            return Err(crate::i18n::tr_format!(
                "This shortcuts file was made by a newer LightCraft (file version {}; this LightCraft reads up to {}). Update LightCraft to read it.",
                v,
                VERSION
            ));
        }
        _ => return Err(crate::i18n::tr("This shortcuts file has no readable \"version\"").to_string()),
    }
    let list = doc
        .get("shortcuts")
        .and_then(Value::as_object)
        .ok_or_else(|| crate::i18n::tr("This shortcuts file has no \"shortcuts\" list").to_string())?;

    // entries this build can't read stay as they are; everything it knows is the file's
    let mut keymap: Keymap =
        current.iter().filter(|(k, _)| find_bindable(conditions::command_of(k)).is_none()).map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut notices = Vec::new();
    let mut commands = 0;
    for (id, entry) in list {
        let Some(b) = find_bindable(id) else {
            notices.push(crate::i18n::tr_format!("Skipped {}: not a command of this LightCraft", id));
            continue;
        };
        let Some(entry) = entry.as_object() else {
            notices.push(crate::i18n::tr_format!("Skipped {}: its entry isn't an object", name(id)));
            continue;
        };
        let mut used = false;
        for (field, slot) in [("shortcut", 0), ("shortcut2", 1)] {
            let Some(v) = entry.get(field) else { continue };
            match v {
                Value::Null => {
                    model::put(&mut keymap, b.id, slot, None);
                    used = true;
                }
                Value::String(sc) if sc.trim().is_empty() => {
                    model::put(&mut keymap, b.id, slot, None);
                    used = true;
                }
                Value::String(sc) => match checked(sc) {
                    Ok(canonical) => {
                        model::put(&mut keymap, b.id, slot, Some(&canonical));
                        used = true;
                    }
                    Err(why) => notices.push(crate::i18n::tr_format!("Skipped the shortcut “{}” of {}: {}", sc, name(id), why)),
                },
                _ => notices.push(crate::i18n::tr_format!("Skipped the {} of {}: it isn't text", field, name(id))),
            }
        }
        match entry.get("when") {
            None => {}
            Some(v @ (Value::Null | Value::String(_))) => match conditions::set(&mut keymap, b.id, v.as_str()) {
                Ok(_) => used = true,
                Err(why) => notices.push(crate::i18n::tr_format!("Dropped the condition of {}: {}", name(id), why)),
            },
            Some(_) => notices.push(crate::i18n::tr_format!("Dropped the condition of {}: it isn't text", name(id))),
        }
        commands += usize::from(used);
    }
    Ok(Imported { keymap, commands, notices })
}

/// A shortcut text this build accepts, in its canonical spelling.
fn checked(sc: &str) -> Result<String, String> {
    let steps = chord::parse_chord(sc.trim()).ok_or_else(|| crate::i18n::tr("not a shortcut").to_string())?;
    let canonical = chord::spell(&steps).ok_or_else(|| crate::i18n::tr("not a shortcut").to_string())?;
    if RESERVED.iter().filter_map(|r| shortcuts::parse(r)).any(|r| steps.contains(&r)) {
        return Err(crate::i18n::tr("it is reserved for the app menu").to_string());
    }
    Ok(canonical)
}

/// `app.exportShortcuts {path}`: write the file. The reply has the `path` and how many commands it holds.
pub fn export_command(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let path = p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("missing path")?;
    let doc = to_json(&app.ui.settings.keymap);
    let count = doc["shortcuts"].as_object().map_or(0, Map::len);
    let mut bytes = serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    match app.services.write.as_mut() {
        Some(write) => write(path, &bytes)?,
        None => lightcraft_engine::export::write_file(path, &bytes)?,
    }
    Ok(json!({"path": path, "commands": count}))
}

/// `app.importShortcuts {path}`: read the file and replace the customisations with its own. The reply has
/// `imported` (commands set) and `notices` (what was skipped); a refused file changes nothing.
pub fn import_command(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let path = p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("missing path")?;
    let text = std::fs::read(path)
        .map_err(|e| crate::i18n::tr_format!("Can't read {}: {}", path, e))
        .and_then(|b| String::from_utf8(b).map_err(|_| crate::i18n::tr("This isn't a LightCraft shortcuts file: it isn't text").to_string()))?;
    let imported = from_text(&text, &app.ui.settings.keymap)?;
    app.ui.settings.keymap = imported.keymap;
    app.keybinds.capture = None;
    app.keybinds.when_edit = None;
    app.keybinds.pending = None;
    app.keybinds.import_notes = imported.notices.clone();
    Ok(json!({"imported": imported.commands, "notices": imported.notices}))
}

// ------------------------------------------------------------------ the window's buttons

/// Export…: the save panel, then `app.exportShortcuts`.
pub fn export_dialog(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(path) = app.services.save_shortcuts_file.as_mut().and_then(|save| save("LightCraft Shortcuts.json")) else { return };
    match app.run("app.exportShortcuts", json!({"path": path})) {
        Ok(r) => {
            let n = r["commands"].as_u64().unwrap_or(0);
            app.toast_for(ctx, crate::i18n::tr_format!("Exported {} customised shortcuts to {}", n, path), 4.0);
        }
        Err(e) => app.toast_for(ctx, e, 5.0),
    }
}

/// Import…: the open panel, then `app.importShortcuts`; a refused file says why.
pub fn import_dialog(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(path) = app.services.pick_shortcuts_file.as_mut().and_then(|pick| pick().into_iter().next()) else { return };
    match app.run("app.importShortcuts", json!({"path": path})) {
        Ok(r) => {
            let n = r["imported"].as_u64().unwrap_or(0);
            let skipped = r["notices"].as_array().map_or(0, Vec::len);
            let text = if skipped == 0 {
                crate::i18n::tr_format!("Imported the shortcuts of {} commands", n)
            } else {
                crate::i18n::tr_format!("Imported the shortcuts of {} commands; {} entries were skipped (see below the list)", n, skipped)
            };
            app.toast_for(ctx, text, 5.0);
        }
        Err(e) => {
            app.keybinds.import_notes = vec![e.clone()];
            app.toast_for(ctx, e, 6.0);
        }
    }
}

/// Under the list: what the last import skipped or refused, with a button to dismiss it.
pub fn notes(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &crate::theme::Tokens) {
    if app.keybinds.import_notes.is_empty() {
        return;
    }
    ui.add_space(4.0);
    let all = app.keybinds.import_notes.clone();
    for line in all.iter().take(3) {
        ui.label(egui::RichText::new(line).size(11.5).color(t.caution));
    }
    ui.horizontal(|ui| {
        if all.len() > 3 {
            ui.label(egui::RichText::new(crate::i18n::tr_format!("…and {} more", all.len() - 3)).size(11.0).color(t.text_dim));
        }
        let r = ui.small_button(crate::i18n::tr("Dismiss"));
        crate::widgets::register(ui, "button:shortcutsNotesDismiss", r.rect);
        if r.clicked() {
            app.keybinds.import_notes.clear();
        }
    });
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
