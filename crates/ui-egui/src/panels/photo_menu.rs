//! The photo menu (a right-click on a photo in the grid, filmstrip, Detail or Compare), which a
//! person arranges: its rows are the built-in items ([`items::BUILTINS`]), any command of the menu
//! bar, and separators, in the order they choose (Settings ▸ Photo Menu, [`editor`]).
//!
//! The arrangement is a list of slots in the app settings ([`AppSettings::photo_menu`]): a
//! built-in item's id, `cmd:` and a menu item's key (`cmd:view.compare`, or with its parameters
//! after a `|`), or [`SEPARATOR`]. None means the default menu ([`default_slots`]), so a person who
//! never changed it gets new items as they come. Ids this version doesn't know (saved by a newer
//! one) are skipped, and dropped when the menu is next changed here.
//!
//! With one photo selected, items that need several (Photo Merge, Export Animated GIF…, Group into
//! Stack, Sync Settings) are left out, and so are items that don't apply (Locate Missing File…
//! for a photo that is not missing). Separators that would then start or end the menu, or follow
//! one another, are dropped.
//!
//! Agents read and set it with `photoMenu.get`, `photoMenu.set {items}` and `photoMenu.reset`.
//! Three presets keep arrangements to switch between ([`presets`]), and a `.json` backup holds the menu
//! and the presets ([`backup`]).
//!
//! [`AppSettings::photo_menu`]: crate::state::AppSettings::photo_menu

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::menus::menubar::{MenuNode, display_item_label, menu_bar, run_item};

pub mod backup;
pub mod editor;
pub mod items;
pub mod presets;
mod presets_ui;

#[cfg(test)]
mod presets_tests;
#[cfg(test)]
mod tests;

/// A separator's slot.
pub const SEPARATOR: &str = "---";
/// The prefix of a menu-bar command's slot.
const COMMAND: &str = "cmd:";
/// The Settings tab of the editor (`app.settings {"tab": "menu"}`).
pub const SETTINGS_TAB: &str = "menu";

/// Commands that work on two or more photos: hidden with one selected.
const NEEDS_SEVERAL: &[&str] =
    &["dialog.animatedGif", "app.animatedGif", "dialog.mergeHdr", "dialog.mergePanorama", "dialog.mergeHdrPanorama", "stack.group", "develop.sync"];

/// Menu-bar submenus listing what is there right now: not offered.
const TRANSIENT: &[&str] = &["Open Recent Library", "Import from Device"];

/// The default menu: the official one, with Before/After Split under Open in Detail and Customize
/// Menu… at the end.
pub const DEFAULT: &[&str] = &[
    "openInDetail",
    "beforeAfterSplit",
    "editIn",
    "findSimilar",
    "setReference",
    SEPARATOR,
    "rating",
    "flag",
    "colorLabel",
    "addToAlbum",
    "removeFromAlbum",
    "setAlbumCover",
    "rename",
    "moveToFolder",
    "reload",
    "virtualCopy",
    "createVersion",
    "stack",
    SEPARATOR,
    "copySettings",
    "pasteSettings",
    "pasteSelected",
    "resetEdits",
    "photoMerge",
    SEPARATOR,
    "rotateLeft",
    "rotateRight",
    SEPARATOR,
    "locateMissing",
    "showInFinder",
    "showInFolders",
    "export",
    "exportWithPreset",
    "exportAnimatedGif",
    SEPARATOR,
    "delete",
    SEPARATOR,
    "customize",
];

/// One row of the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    Item(&'static items::Builtin),
    /// A menu-bar command, run as the menu bar runs it.
    Command {
        id: String,
        params: Value,
    },
    Separator,
}

impl PartialEq for items::Builtin {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl std::fmt::Debug for items::Builtin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id)
    }
}

impl Slot {
    /// The slot as it is saved.
    pub fn key(&self) -> String {
        match self {
            Slot::Item(b) => b.id.to_string(),
            Slot::Command { id, params } => format!("{COMMAND}{}", MenuNode::key(id, params)),
            Slot::Separator => SEPARATOR.to_string(),
        }
    }

    /// A saved slot, or None for one this version doesn't know (or that is malformed).
    pub fn parse(s: &str) -> Option<Slot> {
        let s = s.trim();
        if s == SEPARATOR {
            return Some(Slot::Separator);
        }
        if let Some(cmd) = s.strip_prefix(COMMAND) {
            let (id, params) = match cmd.split_once('|') {
                Some((id, p)) => (id, serde_json::from_str::<Value>(p).ok()?),
                None => (cmd, Value::Null),
            };
            if id.is_empty() {
                return None;
            }
            return Some(Slot::Command { id: id.to_string(), params });
        }
        items::builtin(s).map(Slot::Item)
    }

    /// Whether the slot works on two or more photos only.
    pub fn several(&self) -> bool {
        match self {
            Slot::Item(b) => b.several,
            Slot::Command { id, .. } => NEEDS_SEVERAL.contains(&id.as_str()),
            Slot::Separator => false,
        }
    }
}

/// The default menu's slots.
pub fn default_slots() -> Vec<Slot> {
    DEFAULT.iter().filter_map(|s| Slot::parse(s)).collect()
}

/// The menu as the person arranged it (the default when they didn't).
pub fn slots(app: &LightcraftApp) -> Vec<Slot> {
    match &app.ui.settings.photo_menu {
        Some(saved) => saved.iter().filter_map(|s| Slot::parse(s)).collect(),
        None => default_slots(),
    }
}

/// The saved menu, read leniently: a list keeps its text entries (anything else in it, from a
/// hand-edited file, is skipped); a value that isn't a list is the default menu. The rest of the
/// settings file is read either way.
pub fn read_saved<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    let value = <Value as serde::Deserialize>::deserialize(d)?;
    Ok(value.as_array().map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()))
}

/// Save `slots` as the menu; the default arrangement is saved as None, so it keeps following the
/// default.
pub fn save(app: &mut LightcraftApp, slots: &[Slot]) {
    let keys: Vec<String> = slots.iter().map(Slot::key).collect();
    app.ui.settings.photo_menu = if keys.iter().map(String::as_str).eq(DEFAULT.iter().copied()) { None } else { Some(keys) };
}

/// Drop separators at the start and end and those following another.
pub fn tidy(slots: Vec<Slot>) -> Vec<Slot> {
    let mut out: Vec<Slot> = Vec::with_capacity(slots.len());
    for s in slots {
        if s == Slot::Separator && out.last().is_none_or(|l| *l == Slot::Separator) {
            continue;
        }
        out.push(s);
    }
    while out.last() == Some(&Slot::Separator) {
        out.pop();
    }
    out
}

/// Every menu-bar item, flattened in menu order, with the path to it (`View › Before/After`),
/// but for the [`TRANSIENT`] lists.
pub fn menu_commands(app: &LightcraftApp) -> Vec<(MenuNode, String)> {
    fn walk(nodes: &[MenuNode], path: &mut Vec<String>, seen: &mut std::collections::HashSet<String>, out: &mut Vec<(MenuNode, String)>) {
        for node in nodes {
            match node {
                MenuNode::Separator => {}
                // lists of what is there right now (recent libraries, connected cameras): their
                // rows come and go, so none of them is offered for the photo menu
                MenuNode::Submenu { label, .. } if TRANSIENT.contains(&label.as_str()) => {}
                MenuNode::Submenu { label, children } => {
                    path.push(crate::i18n::tr(label).to_string());
                    walk(children, path, seen, out);
                    path.pop();
                }
                MenuNode::Item { id, params, label, .. } => {
                    if label.trim().is_empty() || !seen.insert(MenuNode::key(id, params)) {
                        continue;
                    }
                    out.push((node.clone(), path.join(" › ")));
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (title, nodes) in menu_bar(app) {
        walk(&nodes, &mut vec![crate::i18n::tr(&title).to_string()], &mut seen, &mut out);
    }
    out
}

/// A menu-bar command's row: its label, whether it can run now, and its check mark.
struct CommandRow {
    label: String,
    enabled: bool,
    checked: bool,
}

fn command_row(commands: &[(MenuNode, String)], id: &str, params: &Value) -> Option<CommandRow> {
    commands.iter().find_map(|(n, _)| match n {
        MenuNode::Item { id: i, params: p, label, enabled, checked, .. } if i == id && p == params => {
            Some(CommandRow { label: display_item_label(i, p, label).to_string(), enabled: *enabled, checked: *checked == Some(true) })
        }
        _ => None,
    })
}

/// The rows the menu shows for `photo` now: hidden items left out, separators tidied.
fn visible(app: &LightcraftApp, photo: PhotoId, commands: &[(MenuNode, String)]) -> Vec<Slot> {
    let several = app.session.selected().len() >= 2;
    let shown = slots(app)
        .into_iter()
        .filter(|s| match s {
            Slot::Item(b) => items::shown(app, b, photo, several),
            // a command the menu bar doesn't have here (another platform's, a host without it)
            Slot::Command { id, params } => (several || !s.several()) && command_row(commands, id, params).is_some(),
            Slot::Separator => true,
        })
        .collect();
    tidy(shown)
}

/// The photo menu for `photo`: what the right-click on it shows. Selects the photo first when it
/// is not selected (a right-click on another photo acts on that one).
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, photo: PhotoId) {
    if !app.session.selection.contains(photo) {
        let _ = app.run("library.select", json!({"ids": [photo.0]}));
    }
    let custom_commands = slots(app).iter().any(|s| matches!(s, Slot::Command { .. }));
    let commands = if custom_commands { menu_commands(app) } else { Vec::new() };
    let rows = visible(app, photo, &commands);
    // everything taken out (or hidden here): the way back to the editor, rather than nothing
    if rows.is_empty()
        && let Some(b) = items::builtin("customize")
    {
        items::draw(app, ui, b, photo);
        return;
    }
    for slot in rows {
        match slot {
            Slot::Separator => {
                ui.separator();
            }
            Slot::Item(b) => items::draw(app, ui, b, photo),
            Slot::Command { id, params } => {
                let Some(row) = command_row(&commands, &id, &params) else { continue };
                let r = ui.add_enabled(row.enabled, egui::Button::new(row.label));
                if row.checked {
                    check_mark(ui, r.rect);
                }
                crate::widgets::register(ui, format!("button:contextCommand:{}", MenuNode::key(&id, &params)), r.rect);
                crate::keybinds::context::on_menu_item(ui, &r, &id);
                if r.clicked()
                    && let Err(e) = run_item(app, &id, params)
                {
                    app.toast(ui.ctx(), e);
                }
            }
        }
    }
}

/// A check mark at the right end of a row (a command that is on, like Before/After Split).
pub(crate) fn check_mark(ui: &egui::Ui, row: egui::Rect) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let c = egui::Rect::from_center_size(egui::pos2(row.right() - 12.0, row.center().y), egui::vec2(13.0, 13.0));
    crate::icons::paint(ui.painter(), c, crate::icons::Icon::Check, t.text);
}

/// `photoMenu.get` / `photoMenu.set {items}` / `photoMenu.reset`, and the presets' and backup's commands.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    let mut extra = Value::Null;
    match id {
        "photoMenu.get" => {}
        "photoMenu.savePreset" | "photoMenu.switchPreset" | "photoMenu.clearPreset" | "photoMenu.undoPreset" => presets::command(app, id, p)?,
        "photoMenu.export" | "photoMenu.import" => match file_command(app, id, p)? {
            Some(done) => extra = done,
            // the file dialog is up: the command runs again when it closes
            None => return Ok(Value::Null),
        },
        "photoMenu.reset" => app.ui.settings.photo_menu = None,
        "photoMenu.set" => {
            let Some(list) = p.get("items").and_then(Value::as_array) else {
                return Err("photoMenu.set: `items` must be a list of slots (built-in ids, `cmd:<command id>`, or `---`)".into());
            };
            let mut slots = Vec::with_capacity(list.len());
            let mut unknown = Vec::new();
            for v in list {
                match v.as_str().and_then(Slot::parse) {
                    Some(s) => slots.push(s),
                    None => unknown.push(v.to_string()),
                }
            }
            let commands = menu_commands(app);
            let mut seen = std::collections::HashSet::new();
            for s in &slots {
                // each item once (separators as often as wanted)
                if *s != Slot::Separator && !seen.insert(s.key()) {
                    unknown.push(format!("{} (twice)", s.key()));
                }
                if let Slot::Command { id, params } = s
                    && command_row(&commands, id, params).is_none()
                {
                    unknown.push(s.key());
                }
            }
            if !unknown.is_empty() {
                return Err(format!("photoMenu.set: unknown or repeated items {} (see photoMenu.get's `available`)", unknown.join(", ")));
            }
            save(app, &slots);
        }
        _ => return Err(format!("unknown command `{id}`")),
    }
    let items: Vec<String> = slots(app).iter().map(Slot::key).collect();
    let available: Vec<Value> = editor::available(&menu_commands(app), &slots(app), "")
        .into_iter()
        .map(|a| json!({"item": a.slot.key(), "label": a.label, "path": a.path}))
        .collect();
    let mut answer = json!({
        "items": items,
        "custom": app.ui.settings.photo_menu.is_some(),
        "available": available,
        "presets": presets::report(app),
        "canUndoPreset": presets::can_undo(app),
    });
    if let (Some(answer), Some(extra)) = (answer.as_object_mut(), extra.as_object()) {
        answer.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    Ok(answer)
}

/// `photoMenu.export` / `photoMenu.import`: the file is `path`, or one chosen in a dialog. None when
/// the dialog is up (or was cancelled).
fn file_command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Option<Value>, String> {
    use crate::pick::{PickRequest, Picked};
    let exporting = id == "photoMenu.export";
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => path.to_string(),
        None => {
            let (title, name) = (if exporting { "Export Right-Click Menu" } else { "Import Right-Click Menu" }, "Photo Menu.json");
            let filter = crate::i18n::tr("Photo Menu Backup");
            let req = if exporting {
                PickRequest::save(crate::i18n::tr(title), filter, &["json"], name)
            } else {
                PickRequest::file(crate::i18n::tr(title), filter, &["json"])
            };
            match crate::pick::ask(app, id, p, "path", req, |_| None) {
                Picked::Now(paths) => match paths.into_iter().next() {
                    Some(path) => path,
                    None => return Ok(None),
                },
                Picked::Later => return Ok(None),
                Picked::Unavailable => return Err(format!("{id}: there is no file dialog on this platform; give `path`")),
            }
        }
    };
    let done = if exporting { backup::export(app, &path) } else { backup::import(app, &path) }?;
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    let skipped: Vec<String> = done["skipped"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let name = std::path::Path::new(&path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().to_string());
    let verb = crate::i18n::tr(if exporting { "Exported the photo menu to" } else { "Imported the photo menu from" });
    if skipped.is_empty() {
        app.toast(&ctx, format!("{verb} {name}"));
    } else {
        app.toast_for(&ctx, format!("{verb} {name}. {}", backup::skipped_note(&skipped)), 8.0);
    }
    Ok(Some(done))
}
