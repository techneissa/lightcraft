//! Help ▸ Keyboard Shortcuts (⌘/): every command with its two shortcuts and the condition ("When")
//! they work under, grouped by menu, searchable and editable. One resizable window (opens about
//! 1000 × 640, remembered, [`size`]). Click a shortcut, or pick a row and press Add Shortcut, and
//! press the keys like in VS Code: each combination is added (two make a chord), the clash warning
//! shows under the list before Enter, Enter saves, Esc cancels ([`super::capture`], [`record`]).
//! A click on a row picks it; ⌫ / Delete or the right-click menu removes the picked shortcut, the
//! ↺ on a changed row restores the declared one. Click a condition to type it with autocomplete, or
//! press + to build one from menus ([`when_cell`]); right-click a row for Edit Condition…. A shortcut
//! that another command also has while both conditions can hold is drawn in the caution colour,
//! with the reason. The search also reads modifier words and symbols ([`search`]). Changes go
//! through `app.setShortcut`, the same path as the right-click menu.

use std::collections::HashMap;

use egui::{Align, Layout, RichText, vec2};

use crate::LightcraftApp;
use crate::shortcuts::{self, Bindable, Keymap};
use crate::theme::Tokens;
use crate::widgets::register;

use super::{capture, chord, model};

mod clashes;
mod fixed;
mod record;
mod rows;
pub mod search;
pub mod size;
mod when_cell;
pub use when_cell::Edit as WhenEdit;

use clashes::Clashes;

const SEARCH: &str = "field:shortcutsSearch";
const SEARCH_ID: &str = "shortcuts-search";
const SHOW_ALL_ID: &str = "shortcuts-show-all";
/// The room the list's scroll bar takes beside the rows.
const SCROLL_BAR: f32 = 14.0;
const ADD_W: f32 = 118.0;

/// A command as the list shows it.
#[derive(Clone)]
pub(super) struct Row {
    pub b: &'static Bindable,
    pub slots: [Option<String>; model::SLOTS],
    /// The menu it sits in (`View`), or [`OTHER`].
    pub group: String,
    /// The path inside the menu (`View › Panels`), empty for a command that is in no menu.
    pub path: String,
}

/// The group of commands that are in no menu.
const OTHER: &str = "Other Commands";

/// Draw the Keyboard Shortcuts window: the search with Add Shortcut at its right, one row per
/// command that has a shortcut (every command when "Show commands without a shortcut" is on, or
/// when searching), the fixed keys that belong to no command, and the footer.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
    let keymap = app.ui.settings.keymap.clone();
    let (search_id, all_id) = (egui::Id::unique(SEARCH_ID), egui::Id::unique(SHOW_ALL_ID));
    let mut search: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let mut show_all: bool = ui.data(|d| d.get_temp(all_id)).unwrap_or(false);
    // a click outside the recording cell, or the search taking the keyboard, ends a recording (like Esc)
    record::end_on_outside_press(app, ui.ctx());
    let mut listed = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let width = (ui.available_width() - ADD_W - ui.spacing().item_spacing.x).max(120.0);
        crate::text_field::TextField::singleline(SEARCH, &mut search)
            .hint(crate::i18n::tr("Search commands or shortcuts, ⌘ ⇧ ⌃ ⌥"))
            .label(crate::i18n::tr("Search Commands"))
            .width(width)
            .show(ui);
        let needle = search.trim().to_lowercase();
        let (rows, clashes) = listing(app, ui.ctx(), &keymap, &needle, show_all, mac);
        // a row the search hides is not picked: Add Shortcut and ⌫ leave it alone
        let picked = picked_listed(app, &rows);
        let full = picked.and_then(|(id, _)| rows.iter().find(|r| r.b.id == id)).is_some_and(|r| r.slots.iter().all(Option::is_some));
        let label = crate::i18n::tr(if full { "Change Shortcut" } else { "Add Shortcut" });
        let r = ui.add_enabled(picked.is_some(), egui::Button::new(label).min_size(vec2(ADD_W, 0.0)));
        register(ui, "button:shortcutsAdd", r.rect);
        if r.clicked()
            && let Some((id, _)) = picked
        {
            let _ = capture::open_inline(app, id, None);
        }
        listed = Some((needle, rows, clashes));
    });
    let Some((needle, rows, clashes)) = listed else { return };
    ui.data_mut(|d| {
        d.insert_temp(search_id, search.clone());
        d.insert_temp(all_id, show_all);
    });
    remove_picked_on_delete(app, ui.ctx(), &rows);
    ui.add_space(2.0);

    let conflicts = record::live_conflicts(app, &keymap);
    let cols = rows::Cols::for_width(ui.available_width() - SCROLL_BAR);
    rows::header(ui, t, &cols);
    let h = size::list_height(ui);
    let cx = rows::Ctx { keymap: &keymap, clashes: &clashes, cols, mac, live_clash: !conflicts.is_empty() };
    egui::ScrollArea::vertical().max_height(h).min_scrolled_height(h).auto_shrink([false, false]).id_salt("shortcuts-list").show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut group = "";
        for row in rows.iter() {
            if row.group != group {
                group = &row.group;
                rows::group_header(ui, t, group);
            }
            rows::draw_row(app, ui, t, row, &cx);
        }
        if rows.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(crate::i18n::tr("No matching commands")).color(t.text_dim));
        }
        fixed::fixed_keys(ui, t, mac, &needle, &keymap);
    });
    size::note_list_bottom(ui, ui.min_rect().bottom());
    when_cell::damaged_note(app, ui, t);
    super::backup::notes(app, ui, t);
    record::status(app, ui, t, &keymap, &clashes, &conflicts, mac);
    footer(app, ui, &keymap, &mut show_all);
    ui.data_mut(|d| d.insert_temp(all_id, show_all));
}

/// Under the list: the "show all" box and Reset All at the left, Export and Import as icons at the right.
fn footer(app: &mut LightcraftApp, ui: &mut egui::Ui, keymap: &Keymap, show_all: &mut bool) {
    ui.horizontal(|ui| {
        let r = ui.checkbox(show_all, crate::i18n::tr("Show commands without a shortcut"));
        register(ui, "check:shortcuts.showAll", r.rect);
        let r = ui.add_enabled(model::has_entries(keymap), egui::Button::new(crate::i18n::tr("Reset All to Default")));
        register(ui, "button:shortcutsResetAll", r.rect);
        if r.clicked() {
            let _ = app.run("app.resetShortcuts", serde_json::json!({}));
            app.keybinds.import_notes.clear();
            capture::close(app);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Import first, so Export sits to its left
            let icon = vec2(28.0, 24.0);
            if app.services.pick_shortcuts_file.is_some()
                && crate::widgets::icon_button(ui, "shortcutsImport", crate::icons::Icon::TrayIn, icon, false, true, "Import Shortcuts…").clicked()
            {
                super::backup::import_dialog(app, ui.ctx());
            }
            if app.services.save_shortcuts_file.is_some()
                && crate::widgets::icon_button(ui, "shortcutsExport", crate::icons::Icon::TrayOut, icon, false, true, "Export Shortcuts…").clicked()
            {
                super::backup::export_dialog(app, ui.ctx());
            }
        });
    });
}

/// The picked row if the list shows it now; a row the search hides stops being picked.
fn picked_listed(app: &mut LightcraftApp, rows: &[Row]) -> Option<(&'static str, usize)> {
    let picked = app.keybinds.selected?;
    if rows.iter().any(|r| r.b.id == picked.0) {
        Some(picked)
    } else {
        app.keybinds.selected = None;
        None
    }
}

/// ⌫ / Delete removes the shortcut of the picked row, when it is listed and nothing is being typed or recorded.
fn remove_picked_on_delete(app: &mut LightcraftApp, ctx: &egui::Context, rows: &[Row]) {
    let Some((id, slot)) = picked_listed(app, rows) else { return };
    if app.keybinds.capture.is_some() || app.keybinds.when_edit.is_some() || ctx.text_edit_focused() || egui::Popup::is_any_open(ctx) {
        return;
    }
    let pressed =
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) | i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
    if !pressed {
        return;
    }
    let slots = model::effective_slots(&app.ui.settings.keymap, id).map(|s| s.map(str::to_string));
    if let Some(slot) = rows::removable_slot(&slots, slot) {
        capture::apply(app, ctx, id, slot, None);
    }
}

/// What the rows were built for: the list is rebuilt only when one of these changes, not each frame
/// (building it lists every command's menu path and enablement).
#[derive(Clone, PartialEq)]
struct ListKey {
    keymap: Keymap,
    needle: String,
    show_all: bool,
    mac: bool,
    capturing: Option<String>,
    language: crate::i18n::Locale,
}

#[derive(Clone)]
struct Listing {
    key: ListKey,
    rows: std::sync::Arc<Vec<Row>>,
}

#[derive(Clone)]
struct ClashCache {
    keymap: Keymap,
    clashes: std::sync::Arc<Clashes>,
}

/// The rows and the clashes, kept in the context's memory and rebuilt when the keymap, the search,
/// the capture or the language changes.
fn listing(
    app: &LightcraftApp,
    ctx: &egui::Context,
    keymap: &Keymap,
    needle: &str,
    show_all: bool,
    mac: bool,
) -> (std::sync::Arc<Vec<Row>>, std::sync::Arc<Clashes>) {
    let (rows_id, clash_id) = (egui::Id::unique("shortcuts-rows"), egui::Id::unique("shortcuts-clashes"));
    let key = ListKey {
        keymap: keymap.clone(),
        needle: needle.to_string(),
        show_all,
        mac,
        capturing: app.keybinds.capture.as_ref().map(|c| c.id.clone()),
        language: crate::i18n::language(),
    };
    let rows = match ctx.data(|d| d.get_temp::<Listing>(rows_id)).filter(|l| l.key == key) {
        Some(l) => l.rows,
        None => {
            let rows = std::sync::Arc::new(rows(app, keymap, needle, show_all, mac));
            ctx.data_mut(|d| d.insert_temp(rows_id, Listing { key, rows: rows.clone() }));
            rows
        }
    };
    let clashes = match ctx.data(|d| d.get_temp::<ClashCache>(clash_id)).filter(|c| &c.keymap == keymap) {
        Some(c) => c.clashes,
        None => {
            let clashes = std::sync::Arc::new(Clashes::of(keymap));
            ctx.data_mut(|d| d.insert_temp(clash_id, ClashCache { keymap: keymap.clone(), clashes: clashes.clone() }));
            clashes
        }
    };
    (rows, clashes)
}

/// The rows to show, grouped by menu in the menu bar's order, then by label. A search also lists
/// the commands that have no shortcut yet, so one can be found and given a key.
fn rows(app: &LightcraftApp, keymap: &Keymap, needle: &str, show_all: bool, mac: bool) -> Vec<Row> {
    let paths: HashMap<String, Vec<String>> = crate::menus::menu_entries(app).into_iter().map(|e| (e.id, e.menu)).collect();
    let capturing = app.keybinds.capture.as_ref().map(|c| c.id.as_str());
    let query = search::parse(needle);
    let mut rows: Vec<Row> = shortcuts::bindable()
        .iter()
        .map(|b| {
            let slots = model::effective_slots(keymap, b.id).map(|s| s.map(str::to_string));
            let path = paths.get(b.id).cloned().unwrap_or_default();
            let group = path.first().cloned().unwrap_or_else(|| OTHER.to_string());
            Row { b, slots, group, path: path.join(" › ") }
        })
        .filter(|r| show_all || !query.is_empty() || r.slots.iter().any(Option::is_some) || capturing == Some(r.b.id))
        .filter(|r| {
            let saved: Vec<&str> = r.slots.iter().flatten().map(String::as_str).collect();
            let hit = |text: &str| {
                crate::i18n::tr(r.b.label).to_lowercase().contains(text)
                    || r.b.id.to_lowercase().contains(text)
                    || r.path.to_lowercase().contains(text)
                    || saved.iter().any(|sc| sc.to_lowercase().contains(text) || chord::text(sc, mac).to_lowercase().contains(text))
            };
            query.matches(&saved, &hit)
        })
        .collect();
    let order = |g: &str| crate::menubar::MENUS.iter().position(|m| *m == g).unwrap_or(usize::MAX);
    rows.sort_by_cached_key(|r| (order(&r.group), r.group.clone(), crate::i18n::tr(r.b.label).to_lowercase()));
    rows
}

/// Restore both shortcuts of `id` (the declared one in Shortcut 1, none in Shortcut 2).
fn reset(app: &mut LightcraftApp, ctx: &egui::Context, id: &str) {
    match app.run("app.setShortcut", serde_json::json!({"id": id, "reset": true})) {
        Ok(r) => {
            let lost: Vec<String> = r["removedFrom"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .map(|id| shortcuts::find_bindable(id).map_or(id, |b| crate::i18n::tr(b.label)).to_string())
                .collect();
            if !lost.is_empty() {
                app.toast_for(ctx, crate::i18n::tr_format!("Removed from {}", lost.join(", ")), 3.0);
            }
        }
        Err(e) => app.toast_for(ctx, e, 3.0),
    }
}

#[cfg(test)]
mod tests;
