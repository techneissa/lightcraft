//! A backup file of the photo menu: the menu and its three presets as one `.json` file
//! (`photoMenu.export {path}`, `photoMenu.import {path}`; the Export and Import icons in Settings ▸ Photo Menu).
//!
//! ```json
//! { "kind": "lightcraft-photo-menu", "version": 1,
//!   "items": ["openInDetail", "---", "cmd:view.compare"],
//!   "presets": { "slots": [["rename"], null, ["delete"]], "active": 0 } }
//! ```
//!
//! An import checks the whole file before it changes anything: a file of another kind, one a newer
//! LightCraft wrote, or one that isn't a menu is refused with a message and nothing changes. Items this
//! build doesn't know (a built-in id from a newer version, a command the menu bar doesn't have, a
//! repeated item) are left out, and the reply names them. `presets` is optional: a file without it
//! leaves the presets as they are.

use std::collections::HashSet;

use serde_json::{Value, json};

use super::presets::{self, COUNT, Presets};
use super::{Slot, menu_commands};
use crate::LightcraftApp;
use crate::menus::menubar::MenuNode;

/// The file's `kind`.
pub const KIND: &str = "lightcraft-photo-menu";
/// The file's `version`: the newest this build reads and the one it writes.
pub const VERSION: u64 = 1;
/// A backup is a few kilobytes; a bigger file is not one.
const MAX_BYTES: u64 = 1 << 20;
/// Entries read from one list; the rest are ignored.
const MAX_ITEMS: usize = 2000;

/// The file's text for the menu and presets as they are now.
pub fn render(app: &LightcraftApp) -> Result<String, String> {
    let p = &app.ui.settings.photo_menu_presets;
    let mut file = json!({"kind": KIND, "version": VERSION, "items": presets::current(app)});
    if !p.is_empty() {
        file["presets"] = json!({"slots": (0..COUNT).map(|i| p.get(i)).collect::<Vec<_>>(), "active": p.active});
    }
    serde_json::to_string_pretty(&file).map(|s| s + "\n").map_err(|e| e.to_string())
}

/// The menu and presets a file holds, checked.
#[derive(Debug, PartialEq)]
pub struct Parsed {
    pub items: Vec<String>,
    /// None when the file has no `presets`.
    pub presets: Option<Presets>,
    /// Entries left out, as they were written.
    pub skipped: Vec<String>,
}

/// Check `text` as a backup file. `commands` is the menu bar's items, for the check that a saved
/// command exists.
pub fn parse(text: &str, commands: &[(MenuNode, String)]) -> Result<Parsed, String> {
    let file: Value = serde_json::from_str(text).map_err(|_| "This file is not a LightCraft photo menu backup (it is not JSON).".to_string())?;
    let Some(obj) = file.as_object() else {
        return Err("This file is not a LightCraft photo menu backup (it is not a JSON object).".into());
    };
    match obj.get("kind").and_then(Value::as_str) {
        Some(KIND) => {}
        Some(other) => {
            let other: String = other.chars().take(40).collect();
            return Err(format!("This file is not a LightCraft photo menu backup (its kind is \"{other}\")."));
        }
        None => return Err("This file is not a LightCraft photo menu backup (it has no kind).".into()),
    }
    match obj.get("version").and_then(Value::as_u64) {
        Some(VERSION) => {}
        Some(v) if v > VERSION => {
            return Err(format!(
                "This backup was made by a newer LightCraft (file version {v}; this one reads up to {VERSION}). Update LightCraft to import it."
            ));
        }
        _ => return Err("This photo menu backup has no valid version.".into()),
    }
    let Some(list) = obj.get("items").and_then(Value::as_array) else {
        return Err("This photo menu backup has no `items` list.".into());
    };
    let mut skipped = Vec::new();
    let items = clean(list, commands, &mut skipped);
    let presets = obj.get("presets").and_then(Value::as_object).map(|saved| {
        let slots = (0..COUNT)
            .map(|i| {
                saved
                    .get("slots")
                    .and_then(Value::as_array)
                    .and_then(|a| a.get(i))
                    .and_then(Value::as_array)
                    .map(|l| clean(l, commands, &mut skipped))
            })
            .collect::<Vec<_>>();
        let active =
            saved.get("active").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).filter(|i| slots.get(*i).is_some_and(Option::is_some));
        Presets { slots, active, undo: None }
    });
    Ok(Parsed { items, presets, skipped })
}

/// The entries of `list` this build can show, as slots are saved; the others go to `skipped`.
fn clean(list: &[Value], commands: &[(MenuNode, String)], skipped: &mut Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (n, v) in list.iter().enumerate() {
        if n >= MAX_ITEMS {
            skipped.push(format!("{} more", list.len() - MAX_ITEMS));
            break;
        }
        let known = v.as_str().and_then(Slot::parse).filter(|s| match s {
            Slot::Command { id, params } => {
                commands.iter().any(|(n, _)| matches!(n, MenuNode::Item { id: i, params: p, .. } if i == id && p == params))
            }
            _ => true,
        });
        match known {
            Some(slot) if slot == Slot::Separator || seen.insert(slot.key()) => out.push(slot.key()),
            _ => {
                let shown = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                skipped.push(shown.chars().take(60).collect());
            }
        }
    }
    out
}

/// What the notice says about `skipped`: the first few, then how many more.
pub fn skipped_note(skipped: &[String]) -> String {
    let shown: Vec<&str> = skipped.iter().take(4).map(String::as_str).collect();
    let more = skipped.len().saturating_sub(shown.len());
    let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
    format!("{} unknown item{} skipped: {}{tail}", skipped.len(), if skipped.len() == 1 { "" } else { "s" }, shown.join(", "))
}

/// Write the backup to `path`.
pub fn export(app: &LightcraftApp, path: &str) -> Result<Value, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("photoMenu.export: path is empty".into());
    }
    std::fs::write(path, render(app)?).map_err(|e| format!("Could not write {path}: {e}"))?;
    let saved = (0..COUNT).filter(|i| app.ui.settings.photo_menu_presets.get(*i).is_some()).count();
    Ok(json!({"path": path, "items": presets::current(app).len(), "presets": saved}))
}

/// Read the backup at `path` and make it the menu (and the presets, when it holds them).
pub fn import(app: &mut LightcraftApp, path: &str) -> Result<Value, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("photoMenu.import: path is empty".into());
    }
    let size = std::fs::metadata(path).map_err(|e| format!("Could not read {path}: {e}"))?.len();
    if size > MAX_BYTES {
        return Err("This file is too big to be a LightCraft photo menu backup.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {path}: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "This file is not a LightCraft photo menu backup (it is not text).".to_string())?;
    let parsed = parse(&text, &menu_commands(app))?;
    presets::set_menu(app, parsed.items);
    if let Some(p) = parsed.presets {
        app.ui.settings.photo_menu_presets = if p.is_empty() { Presets::default() } else { p };
    }
    let saved = (0..COUNT).filter(|i| app.ui.settings.photo_menu_presets.get(*i).is_some()).count();
    Ok(json!({"path": path, "items": presets::current(app).len(), "presets": saved, "skipped": parsed.skipped}))
}
