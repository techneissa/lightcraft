//! The Command Palette (⌘P, View ▸ Command Palette…): a search box over every command a person can
//! run from the menus. Typing filters by label and menu path (word prefixes first, then substrings
//! and loose letter matches), ↑/↓ move the highlight, Return or a click runs it, Esc closes.
//!
//! The list is the menu bar's own model ([`crate::menus::menubar::menu_bar`]): the same labels,
//! live shortcuts and enabled state, and a pick goes through [`crate::menus::menubar::run_item`],
//! so undo, dialogs and disabled rows behave exactly as from the menu. A disabled command is shown
//! greyed and does not run. Commands that are not in a menu (they need parameters) are not listed.

use egui::{Align2, Key, Modifiers, Sense, vec2};
use serde_json::Value;

use crate::LightcraftApp;
use crate::menus::menubar::{MenuNode, display_item_label, menu_bar, run_item, shortcut_text};
use crate::theme::Tokens;

/// The command that opens (and closes) the palette.
pub const COMMAND: &str = "view.commandPalette";
/// The automation id of the search box; rows are `palette:row:<command id>` (with the parameters
/// after a `|` for a menu item that has some, as in the native menu's keys).
pub const SEARCH_WIDGET: &str = "palette:search";

const WIDTH: f32 = 580.0;
const ROW_H: f32 = 28.0;
const MAX_LIST_H: f32 = 400.0;
/// How far PageUp / PageDown move the highlight.
const PAGE: usize = 8;

/// The open palette: what is typed, which row is highlighted, and where the list is scrolled.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub query: String,
    /// Index into the rows the query leaves.
    pub selected: usize,
    scroll: f32,
    /// The highlight moved by key: bring its row into view on the next draw.
    reveal: bool,
    opened: bool,
}

/// One runnable command, as a row.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub params: Value,
    pub label: String,
    /// The menu it lives in: `Photo › Rotate`.
    pub path: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
}

impl Entry {
    fn key(&self) -> String {
        MenuNode::key(&self.id, &self.params)
    }
}

/// Open the palette, or close it when it is open (the shortcut toggles).
pub fn toggle(app: &mut LightcraftApp) {
    app.ui.palette = if app.ui.palette.is_some() { None } else { Some(State::default()) };
}

/// Every command in the menus as one flat list, in menu order, each once.
pub fn entries(app: &LightcraftApp, mac: bool) -> Vec<Entry> {
    fn walk(nodes: &[MenuNode], path: &mut Vec<String>, mac: bool, out: &mut Vec<Entry>) {
        for node in nodes {
            match node {
                MenuNode::Separator => {}
                MenuNode::Submenu { label, children } => {
                    path.push(crate::i18n::tr(label).to_string());
                    walk(children, path, mac, out);
                    path.pop();
                }
                MenuNode::Item { id, params, label, shortcut, enabled, .. } => {
                    if id == COMMAND || label.trim().is_empty() {
                        continue;
                    }
                    let entry = Entry {
                        id: id.clone(),
                        params: params.clone(),
                        label: display_item_label(id, params, label).to_string(),
                        path: path.join(" › "),
                        shortcut: shortcut.as_deref().map(|sc| shortcut_text(sc, mac)),
                        enabled: *enabled,
                    };
                    if !out.iter().any(|e| e.key() == entry.key()) {
                        out.push(entry);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (title, nodes) in menu_bar(app) {
        walk(&nodes, &mut vec![crate::i18n::tr(&title).to_string()], mac, &mut out);
    }
    out
}

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

/// How well one word of the query matches an entry (higher is better), or `None`.
fn token_score(token: &str, label: &str, path: &str) -> Option<i32> {
    if let Some(i) = words(label).position(|w| w.starts_with(token)) {
        return Some(if i == 0 { 120 } else { 100 });
    }
    if words(path).any(|w| w.starts_with(token)) {
        return Some(60);
    }
    if label.contains(token) {
        return Some(40);
    }
    is_subsequence(token, label).then_some(15)
}

/// The score of an entry for a query (words separated by spaces: every word must match), or
/// `None` when it does not match. An empty query matches everything equally.
pub fn score(query: &str, entry: &Entry) -> Option<i32> {
    let label = entry.label.to_lowercase();
    let path = entry.path.to_lowercase();
    let mut total = 0i32;
    for token in query.to_lowercase().split_whitespace() {
        total = total.saturating_add(token_score(token, &label, &path)?);
    }
    // between equal matches the shorter label is the likelier target, and one that can run now
    // comes before one that cannot
    let off = if entry.enabled { 0 } else { 2 };
    Some(total.saturating_sub(i32::try_from(label.chars().count() / 4).unwrap_or(0)).saturating_sub(off))
}

/// The entries a query leaves, best first (equal scores keep the menu order).
pub fn filter(entries: &[Entry], query: &str) -> Vec<Entry> {
    if query.trim().is_empty() {
        return entries.to_vec();
    }
    let mut scored: Vec<(i32, &Entry)> = entries.iter().filter_map(|e| score(query, e).map(|s| (s, e))).collect();
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    scored.into_iter().map(|(_, e)| e.clone()).collect()
}

/// The highlight after a key: ↑/↓ wrap around the list.
fn step(selected: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = isize::try_from(len).unwrap_or(isize::MAX);
    let next = (isize::try_from(selected).unwrap_or(0).saturating_add(delta)).rem_euclid(len);
    usize::try_from(next).unwrap_or(0)
}

/// Draw the palette if it is open and handle its keys. A pick closes it first and then runs the
/// command, so a command that opens a dialog (or the palette again) finds it gone.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(mut state) = app.ui.palette.take() else { return };
    // a dialog, a prompt or the library window owns the screen: the palette makes way for it
    if app.ui.dialog.is_some() || crate::panels::notices::blocks_input(app) || crate::panels::library_problem::blocks_dialog_input(app) {
        return;
    }
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let all = entries(app, mac);
    let mut shown = filter(&all, &state.query);
    state.selected = state.selected.min(shown.len().saturating_sub(1));

    // keys first, before the text field sees them (↑/↓ would move its cursor, Esc would restore it);
    // the press that opened the palette is still in this frame's input, so it only closes it later
    let toggle_key = crate::shortcuts::shortcut_of(&app.ui.settings.keymap, COMMAND)
        .or(Some("Cmd+P"))
        .and_then(crate::shortcuts::parse)
        .filter(|(m, _)| *m != Modifiers::NONE && state.opened);
    let (close, enter, delta) = ctx.input_mut(|i| {
        let close = i.consume_key(Modifiers::NONE, Key::Escape) || toggle_key.is_some_and(|(m, k)| i.consume_key(m, k));
        let enter = i.consume_key(Modifiers::NONE, Key::Enter);
        let mut delta = 0isize;
        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            delta += 1;
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            delta -= 1;
        }
        let page = isize::try_from(PAGE).unwrap_or(8);
        if i.consume_key(Modifiers::NONE, Key::PageDown) {
            delta += page;
        }
        if i.consume_key(Modifiers::NONE, Key::PageUp) {
            delta -= page;
        }
        (close, enter, delta)
    });
    if close {
        return;
    }
    if delta != 0 {
        state.selected = if delta.unsigned_abs() >= PAGE {
            // a page stops at the ends instead of wrapping
            usize::try_from(isize::try_from(state.selected).unwrap_or(0).saturating_add(delta).max(0)).unwrap_or(0).min(shown.len().saturating_sub(1))
        } else {
            step(state.selected, delta, shown.len())
        };
        state.reveal = true;
    }
    let mut run: Option<Entry> = None;
    if enter {
        run = shown.get(state.selected).filter(|e| e.enabled).cloned();
    }

    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let top = (screen.height() * 0.12).clamp(24.0, 120.0);
    let width = WIDTH.min(screen.width() * 0.92);
    let area = egui::Modal::default_area(egui::Id::unique("lightcraft-command-palette")).anchor(Align2::CENTER_TOP, [0.0, top]);
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(10, 10));
    let modal = egui::Modal::new(egui::Id::unique("lightcraft-command-palette"))
        .area(area)
        .backdrop_color(egui::Color32::from_black_alpha(110))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 6.0;
            let before = state.query.clone();
            let hint = crate::i18n::tr("Type a command…").to_string();
            let field = crate::text_field::TextField::singleline(SEARCH_WIDGET, &mut state.query)
                .hint(hint)
                .label(crate::i18n::tr("Command Palette"))
                .width(width)
                .font(t.font(15.0))
                .begin_edit(!state.opened)
                .show(ui);
            state.opened = true;
            // keep typing possible after a click elsewhere in the palette
            if !field.editing && !ui.memory(|m| m.focused().is_some()) {
                ui.memory_mut(|m| m.request_focus(field.response.id));
            }
            if state.query != before {
                shown = filter(&all, &state.query);
                state.selected = 0;
                state.scroll = 0.0;
            }
            ui.add_space(2.0);
            if shown.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(crate::i18n::tr("No matching commands")).color(t.text_dim));
                ui.add_space(6.0);
                return;
            }
            let height = (ROW_H * shown.len() as f32).min(MAX_LIST_H);
            let mut scroll = egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]);
            if std::mem::take(&mut state.reveal) {
                // the highlighted row stays inside the window of rows on show
                let top = state.selected as f32 * ROW_H;
                let bottom = top + ROW_H;
                if top < state.scroll {
                    state.scroll = top;
                } else if bottom > state.scroll + height {
                    state.scroll = bottom - height;
                }
            }
            scroll = scroll.vertical_scroll_offset(state.scroll);
            let moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
            let out = scroll.show_rows(ui, ROW_H, shown.len(), |ui, range| {
                for index in range {
                    let Some(entry) = shown.get(index) else { continue };
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    crate::widgets::register(ui, format!("palette:row:{}", entry.key()), rect);
                    if response.hovered() && moved {
                        state.selected = index;
                    }
                    if index == state.selected {
                        ui.painter().rect_filled(rect, 4.0, if entry.enabled { t.hover } else { t.inset });
                    }
                    let painter = ui.painter();
                    let font = t.font(13.0);
                    let small = t.font(12.0);
                    let label_color = if entry.enabled { t.text } else { t.text_disabled };
                    let label = painter.text(rect.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, &entry.label, font, label_color);
                    let mut right = rect.right() - 10.0;
                    if let Some(sc) = &entry.shortcut {
                        let r = painter.text(egui::pos2(right, rect.center().y), Align2::RIGHT_CENTER, sc, small.clone(), t.text_dim);
                        right = r.left() - 14.0;
                    }
                    let left = label.right() + 12.0;
                    if right > left {
                        let clip = egui::Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(right, rect.bottom()));
                        painter.with_clip_rect(clip).text(egui::pos2(left, rect.center().y), Align2::LEFT_CENTER, &entry.path, small, t.text_dim);
                    }
                    if response.clicked() && entry.enabled {
                        run = Some(entry.clone());
                    }
                }
            });
            state.scroll = out.state.clamped_offset().y;
        });
    if modal.backdrop_response.clicked() {
        return;
    }
    if let Some(entry) = run {
        // the palette is closed (not put back) before the command runs
        if let Err(e) = run_item(app, &entry.id, entry.params.clone()) {
            app.toast(ctx, e);
        }
        return;
    }
    app.ui.palette = Some(state);
}

#[cfg(test)]
mod tests;
