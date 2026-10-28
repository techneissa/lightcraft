//! Help ▸ Keyboard Shortcuts (⌘/): every command with its two shortcuts and the condition ("When")
//! they work under, grouped by menu, searchable and editable. Click a shortcut to open the Assign
//! Shortcut box ([`super::capture`]), × removes it, ↺ restores the declared one; click a condition
//! to type it with autocomplete, or press + to build one from menus ([`when_cell`]). A shortcut that
//! another command also has while both conditions can hold is drawn in the caution colour, with the
//! reason. Changes go through `app.setShortcut`, the same path as the right-click menu.

use std::collections::HashMap;

use egui::{Align, Layout, RichText, Sense, UiBuilder, vec2};

use crate::LightcraftApp;
use crate::shortcuts::{self, Bindable, Keymap};
use crate::theme::Tokens;
use crate::widgets::register;

use super::chord::{self, Step};
use super::{capture, model};

mod when_cell;
pub use when_cell::Edit as WhenEdit;

const SEARCH: &str = "field:shortcutsSearch";
const SEARCH_ID: &str = "shortcuts-search";
const SHOW_ALL_ID: &str = "shortcuts-show-all";

const ROW_H: f32 = 30.0;
const ACTION_W: f32 = 230.0;
const SLOT_BUTTON_W: f32 = 118.0;
const CLEAR_W: f32 = 22.0;
const SLOT_W: f32 = SLOT_BUTTON_W + CLEAR_W + 6.0;
const WHEN_W: f32 = 250.0;
const RESET_W: f32 = 28.0;
const GAP: f32 = 10.0;
const TOTAL_W: f32 = ACTION_W + 2.0 * SLOT_W + WHEN_W + RESET_W + 4.0 * GAP;

/// A command as the list shows it.
struct Row {
    b: &'static Bindable,
    slots: [Option<String>; model::SLOTS],
    /// The menu it sits in (`View`), or [`OTHER`].
    group: String,
    /// The path inside the menu (`View › Panels`), empty for a command that is in no menu.
    path: String,
}

/// The group of commands that are in no menu.
const OTHER: &str = "Other Commands";

/// Draw the Keyboard Shortcuts window: search and "show all", Reset All, one row per command that
/// has a shortcut (every command when "Show commands without a shortcut" is on), and the fixed
/// keys that belong to no command.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
    ui.set_min_width(TOTAL_W.min(ui.ctx().content_rect().width() - 80.0));
    let keymap = app.ui.settings.keymap.clone();
    let (search_id, all_id) = (egui::Id::unique(SEARCH_ID), egui::Id::unique(SHOW_ALL_ID));
    let mut search: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let mut show_all: bool = ui.data(|d| d.get_temp(all_id)).unwrap_or(false);
    ui.horizontal(|ui| {
        crate::text_field::TextField::singleline(SEARCH, &mut search)
            .hint(crate::i18n::tr("Search commands or keys"))
            .label(crate::i18n::tr("Search Commands"))
            .width(260.0)
            .show(ui);
        let r = ui.checkbox(&mut show_all, crate::i18n::tr("Show commands without a shortcut"));
        register(ui, "check:shortcuts.showAll", r.rect);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = ui.add_enabled(model::has_entries(&keymap), egui::Button::new(crate::i18n::tr("Reset All to Default")));
            register(ui, "button:shortcutsResetAll", r.rect);
            if r.clicked() {
                let _ = app.run("app.resetShortcuts", serde_json::json!({}));
                capture::close(app);
            }
        });
    });
    ui.data_mut(|d| {
        d.insert_temp(search_id, search.clone());
        d.insert_temp(all_id, show_all);
    });
    ui.label(
        RichText::new(crate::i18n::tr(
            "Click a shortcut to change it. Shortcut 1 is the one menus show; Shortcut 2 works too. A chord is two keys in a row.",
        ))
        .size(11.0)
        .color(t.text_dim),
    );
    ui.add_space(6.0);

    let needle = search.trim().to_lowercase();
    let rows = rows(app, &keymap, &needle, show_all, mac);
    let clashes = Clashes::of(&keymap);
    header(ui, t);
    // a fixed height, so the dialog doesn't jump around while the search narrows the list
    // (shorter on a short window, so the dialog fits it)
    let h = crate::panels::dialogs::list_height(ui, 440.0);
    egui::ScrollArea::vertical().max_height(h).min_scrolled_height(h).auto_shrink([false, false]).id_salt("shortcuts-list").show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut group = "";
        for row in &rows {
            if row.group != group {
                group = &row.group;
                group_header(ui, t, group);
            }
            draw_row(app, ui, t, row, &keymap, &clashes, mac);
        }
        if rows.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(crate::i18n::tr("No matching commands")).color(t.text_dim));
        }
        fixed_keys(ui, t, mac, &needle, &keymap);
    });
    when_cell::damaged_note(app, ui, t);
    clash_note(ui, t, &clashes, &keymap, mac);
}

/// The column titles.
fn header(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(vec2(TOTAL_W, 22.0), Sense::hover());
    let mut x = rect.left();
    for (title, w) in [("Action", ACTION_W), ("Shortcut 1", SLOT_W), ("Shortcut 2", SLOT_W), ("When", WHEN_W)] {
        ui.painter().text(egui::pos2(x + 4.0, rect.center().y), egui::Align2::LEFT_CENTER, crate::i18n::tr(title), t.semibold(11.5), t.text_label);
        x += w + GAP;
    }
    ui.painter().hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.divider));
}

fn group_header(ui: &mut egui::Ui, t: &Tokens, group: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(crate::i18n::tr(group)).font(t.semibold(12.5)).color(t.accent));
    ui.add_space(2.0);
}

/// The rows to show, grouped by menu in the menu bar's order, then by label.
fn rows(app: &LightcraftApp, keymap: &Keymap, needle: &str, show_all: bool, mac: bool) -> Vec<Row> {
    let paths: HashMap<String, Vec<String>> = crate::menus::menu_entries(app).into_iter().map(|e| (e.id, e.menu)).collect();
    let capturing = app.keybinds.capture.as_ref().map(|c| c.id.as_str());
    let mut rows: Vec<Row> = shortcuts::bindable()
        .iter()
        .map(|b| {
            let slots = model::effective_slots(keymap, b.id).map(|s| s.map(str::to_string));
            let path = paths.get(b.id).cloned().unwrap_or_default();
            let group = path.first().cloned().unwrap_or_else(|| OTHER.to_string());
            Row { b, slots, group, path: path.join(" › ") }
        })
        .filter(|r| show_all || r.slots.iter().any(Option::is_some) || capturing == Some(r.b.id))
        .filter(|r| {
            needle.is_empty()
                || crate::i18n::tr(r.b.label).to_lowercase().contains(needle)
                || r.b.id.to_lowercase().contains(needle)
                || r.path.to_lowercase().contains(needle)
                || r.slots.iter().flatten().any(|sc| sc.to_lowercase().contains(needle) || chord::text(sc, mac).to_lowercase().contains(needle))
        })
        .collect();
    let order = |g: &str| crate::menubar::MENUS.iter().position(|m| *m == g).unwrap_or(usize::MAX);
    rows.sort_by_cached_key(|r| (order(&r.group), r.group.clone(), crate::i18n::tr(r.b.label).to_lowercase()));
    rows
}

/// For each binding that shares its key with another command's while the two conditions can hold
/// together: who. Commands separated by conditions (the X pair) are no clash.
fn clashes(keymap: &Keymap) -> HashMap<(&'static str, usize), Vec<&'static str>> {
    let mut by_first: HashMap<String, Vec<(&'static str, usize, Vec<Step>)>> = HashMap::new();
    for b in shortcuts::bindable() {
        for slot in 0..model::SLOTS {
            if let Some(steps) = model::effective(keymap, b.id, slot).and_then(chord::parse_chord)
                && let Some(first) = steps.first().and_then(|s| chord::spell(&[*s]))
            {
                by_first.entry(first).or_default().push((b.id, slot, steps));
            }
        }
    }
    let mut out: HashMap<(&'static str, usize), Vec<&'static str>> = HashMap::new();
    for group in by_first.values().filter(|g| g.len() > 1) {
        for (id, slot, steps) in group {
            for (other, _, theirs) in group {
                if other != id && chord::overlaps(steps, theirs) && model::conditions::overlap(keymap, id, other).is_some() {
                    out.entry((*id, *slot)).or_default().push(other);
                }
            }
        }
    }
    out
}

/// The clashes of a keymap, and why each command's is one.
struct Clashes {
    slots: HashMap<(&'static str, usize), Vec<&'static str>>,
    /// Per command: the other command it clashes with and the reason, in words.
    reasons: HashMap<&'static str, (&'static str, String)>,
}

impl Clashes {
    fn of(keymap: &Keymap) -> Self {
        let slots = clashes(keymap);
        let mut reasons = HashMap::new();
        for ((id, _), others) in &slots {
            if let Some(other) = others.first() {
                reasons.entry(*id).or_insert((*other, reason(keymap, id, other)));
            }
        }
        Clashes { slots, reasons }
    }
}

/// Why two commands on one key clash: when both work.
fn reason(keymap: &Keymap, id: &str, other: &str) -> String {
    use super::when::expr;
    let (a, b) = (model::conditions::of(keymap, id), model::conditions::of(keymap, other));
    match expr::overlap(a.as_ref(), b.as_ref()) {
        _ if a.is_none() && b.is_none() => crate::i18n::tr("both always work").to_string(),
        Some(cx) => crate::i18n::tr_format!("both work when {}", expr::describe(&cx, a.as_ref(), b.as_ref())),
        None => String::new(),
    }
}

/// Under the list: the first clash in words, in the caution colour.
fn clash_note(ui: &mut egui::Ui, t: &Tokens, clashes: &Clashes, keymap: &Keymap, mac: bool) {
    let Some((id, (other, why))) = clashes.reasons.iter().min_by_key(|(id, _)| **id) else { return };
    let name = |id: &str| shortcuts::find_bindable(id).map_or(id.to_string(), |b| crate::i18n::tr(b.label).to_string());
    let key = (0..model::SLOTS)
        .find(|slot| clashes.slots.contains_key(&(*id, *slot)))
        .and_then(|slot| model::effective(keymap, id, slot))
        .map(|sc| chord::text(sc, mac))
        .unwrap_or_default();
    let more = clashes.reasons.len().saturating_sub(2);
    ui.add_space(4.0);
    let mut line = crate::i18n::tr_format!("{} and {} share {}: {}. Give one of them a condition to tell them apart.", name(id), name(other), key, why);
    if more > 0 {
        line.push_str(&crate::i18n::tr_format!(" {} more conflicts.", more));
    }
    ui.label(RichText::new(line).size(11.5).color(t.caution));
}

/// One command: its name, two shortcut cells, the menu it is in and a restore button.
fn draw_row(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    row: &Row,
    keymap: &Keymap,
    clashes: &Clashes,
    mac: bool,
) {
    let b = row.b;
    let (rect, _) = ui.allocate_exact_size(vec2(TOTAL_W, ROW_H), Sense::hover());
    let hovered = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p)) && !egui::Popup::is_any_open(ui.ctx());
    if hovered {
        ui.painter().rect_filled(rect.expand2(vec2(4.0, 0.0)), 4.0, t.hover.gamma_multiply(0.55));
    }
    ui.painter().hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.divider.gamma_multiply(0.35)));
    let changed = model::is_changed(keymap, b.id);
    let column = |x: f32, w: f32| egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(w, ROW_H));
    let mut x = rect.left();

    ui.scope_builder(UiBuilder::new().max_rect(column(x + 4.0, ACTION_W - 4.0)).layout(Layout::left_to_right(Align::Center)), |ui| {
        let mut name = RichText::new(crate::i18n::tr(b.label)).color(t.text);
        if changed {
            name = name.strong();
        }
        // keys whose command changes in the Library grids (see `shortcuts::handle`)
        let hover = match b.id {
            "panel.presets" => format!("{} — {}", b.id, crate::i18n::tr("In Photo Grid and Square Grid: Flag as Pick and advance")),
            "view.softProof" => format!("{} — {}", b.id, crate::i18n::tr("In Photo Grid and Square Grid: expand or collapse the stack")),
            _ => b.id.to_string(),
        };
        let hover = if row.path.is_empty() { hover } else { format!("{hover}\n{}", row.path) };
        ui.add(egui::Label::new(name).truncate()).on_hover_text(hover);
    });
    x += ACTION_W + GAP;

    for slot in 0..model::SLOTS {
        ui.scope_builder(UiBuilder::new().max_rect(column(x, SLOT_W)).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            slot_cell(app, ui, t, row, slot, &clashes.slots, mac);
        });
        x += SLOT_W + GAP;
    }

    let reason = clashes.reasons.get(b.id).map(|(other, why)| {
        let name = shortcuts::find_bindable(other).map_or(*other, |o| crate::i18n::tr(o.label));
        crate::i18n::tr_format!("Same key as {}: {}.", name, why)
    });
    when_cell::cell(app, ui, t, b.id, column(x, WHEN_W), reason.as_deref());
    x += WHEN_W + GAP;

    ui.scope_builder(UiBuilder::new().max_rect(column(x, RESET_W)).layout(Layout::left_to_right(Align::Center)), |ui| {
        let r = ui.add_enabled(changed, egui::Button::new("↺").small()).on_hover_text(match b.default {
            Some(d) => crate::i18n::tr_format!("Reset to Default ({})", chord::text(d, mac)),
            None => crate::i18n::tr("Reset to Default (no shortcut)").to_string(),
        });
        register(ui, format!("button:shortcutReset-{}", b.id), r.rect);
        if r.clicked() {
            reset(app, ui.ctx(), b.id);
        }
    });
}

/// One shortcut cell: a button that opens the Assign Shortcut box (right-click: edit or remove),
/// and × that removes it.
fn slot_cell(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    row: &Row,
    slot: usize,
    clashes: &HashMap<(&'static str, usize), Vec<&'static str>>,
    mac: bool,
) {
    let b = row.b;
    let sc = row.slots.get(slot).and_then(Option::as_deref);
    let recording = app.keybinds.capture.as_ref().is_some_and(|c| c.id == b.id && c.slot == slot);
    let clash = clashes.get(&(b.id, slot));
    let text = match sc {
        Some(sc) => RichText::new(chord::text(sc, mac)).color(if clash.is_some() { t.caution } else { t.text }),
        None => RichText::new("—").color(t.text_dim),
    };
    let widget = if slot == 0 { format!("button:shortcut-{}", b.id) } else { format!("button:shortcut{}-{}", slot + 1, b.id) };
    let mut r = ui.add(egui::Button::new(text).min_size(vec2(SLOT_BUTTON_W, 22.0)).selected(recording));
    register(ui, widget, r.rect);
    if let Some(others) = clash {
        let names: Vec<_> = others.iter().map(|id| shortcuts::find_bindable(id).map_or(*id, |o| crate::i18n::tr(o.label))).collect();
        r = r.on_hover_text(crate::i18n::tr_format!("Also used by {}", names.join(", ")));
    }
    if r.clicked() {
        let _ = capture::open(app, b.id, Some(slot));
    }
    crate::widgets::context_menu(&r, |ui| {
        if ui.button(crate::i18n::tr("Edit Shortcut…")).clicked() {
            let _ = capture::open(app, b.id, Some(slot));
            ui.close();
        }
        if ui.add_enabled(sc.is_some(), egui::Button::new(crate::i18n::tr("Remove Shortcut"))).clicked() {
            capture::apply(app, ui.ctx(), b.id, slot, None);
            ui.close();
        }
    });
    let clear = if slot == 0 { format!("button:shortcutClear-{}", b.id) } else { format!("button:shortcutClear{}-{}", slot + 1, b.id) };
    let r =
        ui.add_enabled(sc.is_some(), egui::Button::new("×").small().min_size(vec2(CLEAR_W, 0.0))).on_hover_text(crate::i18n::tr("Remove Shortcut"));
    register(ui, clear, r.rect);
    if r.clicked() {
        capture::apply(app, ui.ctx(), b.id, slot, None);
    }
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

/// Keys that aren't a command's own shortcut (ratings, labels, keys with parameters): listed, not editable.
/// One the user gave to a command shows struck through.
fn fixed_keys(ui: &mut egui::Ui, t: &Tokens, mac: bool, needle: &str, keymap: &Keymap) {
    let mut fixed: Vec<(String, &str)> = Vec::new();
    for (sc, id, _) in shortcuts::ALIASES {
        let label = shortcuts::find_bindable(id).map_or(*id, |b| b.label);
        fixed.push((sc.to_string(), label));
    }
    for n in 0..=5 {
        fixed.push((n.to_string(), ["Clear rating", "Rate ★", "Rate ★★", "Rate ★★★", "Rate ★★★★", "Rate ★★★★★"][n]));
        fixed.push((
            format!("Shift+{n}"),
            [
                "Clear rating and advance",
                "Rate ★ and advance",
                "Rate ★★ and advance",
                "Rate ★★★ and advance",
                "Rate ★★★★ and advance",
                "Rate ★★★★★ and advance",
            ][n],
        ));
    }
    for (sc, label) in [("6", "Red label"), ("7", "Yellow label"), ("8", "Green label"), ("9", "Blue label")] {
        fixed.push((sc.to_string(), label));
    }
    // grid navigation (Photo Grid and Square Grid): fixed keys that belong to no single command
    for (sc, label) in [
        ("Home", "Grid: first photo of the view"),
        ("End", "Grid: last photo of the view"),
        ("Shift+Left", "Grid: add the previous photo to the selection"),
        ("Shift+Right", "Grid: add the next photo to the selection"),
    ] {
        fixed.push((sc.to_string(), label));
    }
    let fixed: Vec<_> = fixed
        .into_iter()
        .filter(|(sc, label)| {
            needle.is_empty() || crate::i18n::tr(label).to_lowercase().contains(needle) || chord::text(sc, mac).to_lowercase().contains(needle)
        })
        .collect();
    if fixed.is_empty() {
        return;
    }
    ui.add_space(14.0);
    ui.label(RichText::new(crate::i18n::tr("Fixed Keys")).font(t.semibold(12.5)).color(t.accent));
    ui.add_space(2.0);
    // keys the keymap gives to commands (single keys, second shortcuts, chord leaders) override these
    let taken: Vec<_> = shortcuts::saved_keys(keymap).map(|(_, k)| k).chain(model::extra_taken(keymap)).collect();
    egui::Grid::new("shortcuts-fixed").striped(true).num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        for (sc, label) in fixed {
            ui.label(RichText::new(crate::i18n::tr(label)).color(t.text_label));
            let mut text = RichText::new(chord::text(&sc, mac)).color(t.text_dim);
            if shortcuts::parse(&sc).is_some_and(|k| taken.contains(&k)) {
                text = text.strikethrough();
            }
            ui.label(text);
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests;
