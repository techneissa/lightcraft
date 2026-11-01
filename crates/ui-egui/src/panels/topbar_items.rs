//! The top bar's items, which a person turns on and off (Settings ▸ Top Bar, [`editor`], or a
//! right-click on the bar): the sidebar button, Back and Forward, Filters, and the icons on the
//! right ([`ITEMS`]). The search field, the window's own buttons and the in-window menus stay.
//!
//! What is saved is the list of items turned off ([`AppSettings::top_bar_hidden`]), so a person who
//! never changed it gets every item, and new ones as they come. Ids this version doesn't know
//! (saved by a newer one) are ignored, and dropped when the list is next changed here.
//!
//! An item turned off only leaves the bar: its command still runs from the menu bar (each item's
//! menu path is shown in the editor), the keyboard, the command palette and the control channel.
//! The one exception to "off is off" is the Library Status icon, which comes back while saving is
//! failing, so a warning about unsaved changes is never hidden.
//!
//! Agents read and set it with `topBar.get`, `topBar.set {hidden}`, `topBar.setItem {item, shown}`
//! and `topBar.reset`.
//!
//! [`AppSettings::top_bar_hidden`]: crate::state::AppSettings::top_bar_hidden

use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::menus::menubar::{MenuNode, display_item_label};

pub mod editor;

#[cfg(test)]
mod tests;

/// The Settings tab of the editor (`app.settings {"tab": "topbar"}`).
pub const SETTINGS_TAB: &str = "topbar";

/// One item of the bar a person can turn off.
#[derive(Debug)]
pub struct Item {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: Icon,
    /// The command it runs ("" for an indicator that runs none).
    pub command: &'static str,
    /// Where else to find it when the menu bar has no row for [`Item::command`].
    pub elsewhere: &'static str,
}

/// The items in the order of the bar, from the left.
pub const ITEMS: &[Item] = &[
    Item { id: "sidebar", label: "My Photos Panel", icon: Icon::Sidebar, command: "view.leftPanel", elsewhere: "" },
    Item { id: "back", label: "Back and Forward", icon: Icon::Back, command: "view.back", elsewhere: "Esc key" },
    Item { id: "filter", label: "Filters", icon: Icon::Filter, command: "view.filterBar", elsewhere: "" },
    Item { id: "appearance", label: "Appearance Mode", icon: Icon::Monitor, command: "view.appearance", elsewhere: "" },
    Item { id: "bell", label: "Activity", icon: Icon::Bell, command: "panel.activity", elsewhere: "" },
    Item { id: "import", label: "Import Photos", icon: Icon::Plus, command: "file.addPhotos", elsewhere: "" },
    Item { id: "share", label: "Export", icon: Icon::Share, command: "dialog.export", elsewhere: "" },
    Item { id: "help", label: "Keyboard Shortcuts", icon: Icon::Help, command: "app.shortcuts", elsewhere: "" },
    Item { id: "cloud", label: "Library Status", icon: Icon::Cloud, command: "", elsewhere: "Shows when saving fails" },
];

/// The item `id`.
pub fn item(id: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|i| i.id == id)
}

/// The saved list, read leniently: a list keeps its text entries (anything else in it, from a
/// hand-edited file, is skipped); a value that isn't a list is no item turned off. The rest of the
/// settings file is read either way.
pub fn read_saved<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let value = <Value as serde::Deserialize>::deserialize(d)?;
    Ok(value.as_array().map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default())
}

/// Whether the bar shows item `id`.
pub fn shown(app: &LightcraftApp, id: &str) -> bool {
    !app.ui.settings.top_bar_hidden.iter().any(|h| h == id)
}

/// Turn the items in `hidden` off and every other on. Unknown ids are dropped; the list is kept in
/// the bar's order, so the same choice is always saved the same way.
pub fn set_hidden(app: &mut LightcraftApp, hidden: &[String]) {
    app.ui.settings.top_bar_hidden = ITEMS.iter().filter(|i| hidden.iter().any(|h| h == i.id)).map(|i| i.id.to_string()).collect();
}

/// Turn one item on or off.
pub fn set_shown(app: &mut LightcraftApp, id: &str, on: bool) {
    let mut hidden: Vec<String> = app.ui.settings.top_bar_hidden.clone();
    hidden.retain(|h| h != id);
    if !on {
        hidden.push(id.to_string());
    }
    set_hidden(app, &hidden);
}

/// Show every item.
pub fn reset(app: &mut LightcraftApp) {
    app.ui.settings.top_bar_hidden.clear();
}

/// Where the menu bar has the item's command (`View › Filter Bar`), or what stands in for it.
pub fn reach(commands: &[(MenuNode, String)], item: &Item) -> String {
    let found = commands.iter().find_map(|(n, path)| match n {
        MenuNode::Item { id, params, label, .. } if id == item.command && params.is_null() => {
            Some(format!("{path} › {}", display_item_label(id, params, label)))
        }
        _ => None,
    });
    match found {
        Some(path) => path,
        None if item.elsewhere.is_empty() => crate::i18n::tr("Command only").to_string(),
        None => crate::i18n::tr(item.elsewhere).to_string(),
    }
}

/// A right-click on the bar's empty space: every item with a check mark when shown, then the way
/// to the editor. The menu stays open while items are turned on and off.
pub fn bar_menu(app: &mut LightcraftApp, response: &egui::Response) {
    let close = crate::widgets::context_menu_close(response, egui::PopupCloseBehavior::CloseOnClickOutside);
    egui::Popup::context_menu(response).close_behavior(close).show(|ui| {
        for item in ITEMS {
            let on = shown(app, item.id);
            let r = ui.button(crate::i18n::tr(item.label));
            if on {
                crate::panels::photo_menu::check_mark(ui, r.rect);
            }
            crate::widgets::register(ui, format!("menu:topBarItem:{}", item.id), r.rect);
            if r.clicked() {
                let _ = app.run("topBar.setItem", json!({"item": item.id, "shown": !on}));
            }
        }
        ui.separator();
        let r = ui.button(crate::i18n::tr("Customize Top Bar…"));
        crate::widgets::register(ui, "menu:customizeTopBar", r.rect);
        if r.clicked() {
            let _ = app.run("app.settings", json!({"tab": SETTINGS_TAB}));
            ui.close();
        }
    });
}

/// `topBar.get` / `topBar.set {hidden}` / `topBar.setItem {item, shown}` / `topBar.reset`.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    match id {
        "topBar.get" => {}
        "topBar.reset" => reset(app),
        "topBar.set" => {
            let Some(list) = p.get("hidden").and_then(Value::as_array) else {
                return Err("topBar.set: `hidden` must be a list of item ids (see topBar.get)".into());
            };
            let mut hidden = Vec::with_capacity(list.len());
            let mut unknown = Vec::new();
            for v in list {
                match v.as_str().filter(|s| item(s).is_some()) {
                    Some(s) => hidden.push(s.to_string()),
                    None => unknown.push(v.to_string()),
                }
            }
            if !unknown.is_empty() {
                return Err(format!("topBar.set: unknown items {} (see topBar.get)", unknown.join(", ")));
            }
            set_hidden(app, &hidden);
        }
        "topBar.setItem" => {
            let id = p.get("item").and_then(Value::as_str).unwrap_or_default();
            if item(id).is_none() {
                return Err(format!("topBar.setItem: unknown item `{id}` (see topBar.get)"));
            }
            let Some(on) = p.get("shown").and_then(Value::as_bool) else {
                return Err("topBar.setItem: `shown` must be true or false".into());
            };
            set_shown(app, id, on);
        }
        _ => return Err(format!("unknown command `{id}`")),
    }
    let commands = crate::panels::photo_menu::menu_commands(app);
    let items: Vec<Value> = ITEMS
        .iter()
        .map(|i| json!({"id": i.id, "label": i.label, "shown": shown(app, i.id), "command": i.command, "reach": reach(&commands, i)}))
        .collect();
    let hidden: Vec<&String> = app.ui.settings.top_bar_hidden.iter().collect();
    Ok(json!({"items": items, "hidden": hidden, "custom": !hidden.is_empty()}))
}
