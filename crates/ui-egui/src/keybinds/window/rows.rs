//! The list's rows: a command, its two shortcut cells, the reset icon and the When cell. A click
//! on a row picks it (a quiet band), a click on a shortcut records into it, right-click offers the
//! rest, ⌫ / Delete removes the picked shortcut.

use egui::{Align, Align2, Layout, RichText, Sense, UiBuilder, vec2};

use crate::LightcraftApp;
use crate::shortcuts::{self, Bindable, Keymap};
use crate::theme::Tokens;
use crate::widgets::register;

use super::super::{capture, chord, model};
use super::clashes::Clashes;
use super::{Row, when_cell};

pub(super) const ROW_H: f32 = 28.0;
const SLOT_W: f32 = 132.0;
const RESET_W: f32 = 26.0;
const GAP: f32 = 8.0;
const ACTION_MIN: f32 = 170.0;
const WHEN_MIN: f32 = 230.0;
const CELL_H: f32 = 22.0;

/// The widths of the columns for a list `width` points wide: the fixed ones, then the Action and
/// When columns share what is left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cols {
    pub action: f32,
    pub when: f32,
    pub total: f32,
}

impl Cols {
    pub fn for_width(width: f32) -> Cols {
        let fixed = 2.0 * SLOT_W + RESET_W + 4.0 * GAP;
        let flex = (width - fixed).max(ACTION_MIN + WHEN_MIN);
        let action = (flex * 0.42).max(ACTION_MIN);
        Cols { action, when: flex - action, total: fixed + flex }
    }

    /// Left edge of Shortcut 1, Shortcut 2, the reset icon and When, from the row's left.
    fn slot_x(&self, slot: usize) -> f32 {
        self.action + GAP + slot as f32 * (SLOT_W + GAP)
    }

    fn reset_x(&self) -> f32 {
        self.slot_x(model::SLOTS)
    }

    fn when_x(&self) -> f32 {
        self.reset_x() + RESET_W + GAP
    }
}

/// The column titles.
pub(super) fn header(ui: &mut egui::Ui, t: &Tokens, cols: &Cols) {
    let (rect, _) = ui.allocate_exact_size(vec2(cols.total, 22.0), Sense::hover());
    for (title, x) in [("Action", 0.0), ("Shortcut 1", cols.slot_x(0)), ("Shortcut 2", cols.slot_x(1)), ("When", cols.when_x())] {
        let at = egui::pos2(rect.left() + x + 4.0, rect.center().y);
        ui.painter().text(at, Align2::LEFT_CENTER, crate::i18n::tr(title), t.semibold(11.5), t.text_label);
    }
    // the one line of help the window keeps, on the first shortcut column's title
    let hover = egui::Rect::from_min_size(egui::pos2(rect.left() + cols.slot_x(0), rect.top()), vec2(SLOT_W * 2.0 + GAP, 22.0));
    ui.interact(hover, ui.scope_id().with("shortcut-titles"), Sense::hover())
        .on_hover_text(crate::i18n::tr("Shortcut 1 is the one menus show; Shortcut 2 works too. A chord is two keys in a row."));
    ui.painter().hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.divider));
}

pub(super) fn group_header(ui: &mut egui::Ui, t: &Tokens, group: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(crate::i18n::tr(group)).font(t.semibold(12.5)).color(t.accent));
    ui.add_space(2.0);
}

/// What a row needs besides its command.
pub(super) struct Ctx<'a> {
    pub keymap: &'a Keymap,
    pub clashes: &'a Clashes,
    pub cols: Cols,
    pub mac: bool,
    /// Whether the keys recorded now would take a shortcut from another command.
    pub live_clash: bool,
}

/// The slot ⌫ / Delete removes for a row picked at `slot`: that one, else the other one that has a shortcut.
pub(super) fn removable_slot(row_slots: &[Option<String>; model::SLOTS], slot: usize) -> Option<usize> {
    let filled = |s: usize| row_slots.get(s).is_some_and(Option::is_some);
    if filled(slot) { Some(slot) } else { (0..model::SLOTS).find(|s| filled(*s)) }
}

/// Pick `row`; `slot` is the shortcut cell clicked, or the first one that has a shortcut.
fn pick(app: &mut LightcraftApp, row: &Row, slot: Option<usize>) {
    let slot = slot.unwrap_or_else(|| removable_slot(&row.slots, 0).unwrap_or(0));
    app.keybinds.selected = Some((row.b.id, slot));
}

/// Draw into `rect` of the row without moving the list's cursor (the row was allocated whole).
fn in_cell(ui: &mut egui::Ui, rect: egui::Rect, add: impl FnOnce(&mut egui::Ui)) {
    let mut cell = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    add(&mut cell);
}

/// One command.
pub(super) fn draw_row(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, row: &Row, cx: &Ctx) {
    let b = row.b;
    let (rect, _) = ui.allocate_exact_size(vec2(cx.cols.total, ROW_H), Sense::hover());
    let resp = ui.interact(rect, ui.scope_id().with(("shortcut-row", b.id)), Sense::CLICK);
    register(ui, format!("row:shortcut-{}", b.id), rect);
    let picked = app.keybinds.selected.is_some_and(|(id, _)| id == b.id);
    let hovered = resp.contains_pointer() && !egui::Popup::is_any_open(ui.ctx());
    if picked {
        ui.painter().rect_filled(rect.expand2(vec2(4.0, 0.0)), 4.0, t.accent.gamma_multiply(0.16));
    } else if hovered {
        ui.painter().rect_filled(rect.expand2(vec2(4.0, 0.0)), 4.0, t.hover.gamma_multiply(0.4));
    }
    ui.painter().hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.divider.gamma_multiply(0.35)));
    let changed = model::is_changed(cx.keymap, b.id);
    let column = |x: f32, w: f32| egui::Rect::from_min_size(egui::pos2(rect.left() + x, rect.top()), vec2(w, ROW_H));

    in_cell(ui, column(4.0, cx.cols.action - 4.0), |ui| {
        let mut name = RichText::new(crate::i18n::tr(b.label)).color(t.text);
        if changed {
            name = name.strong();
        }
        ui.add(egui::Label::new(name).truncate().selectable(false)).on_hover_text(hover_text(b, row));
    });
    for slot in 0..model::SLOTS {
        in_cell(ui, column(cx.cols.slot_x(slot), SLOT_W), |ui| {
            slot_cell(app, ui, t, row, slot, cx);
        });
    }
    in_cell(ui, column(cx.cols.reset_x(), RESET_W), |ui| {
        if changed {
            reset_icon(app, ui, t, b, cx.mac);
        }
    });
    let reason = cx.clashes.reasons.get(b.id).map(|(other, why)| {
        let name = shortcuts::find_bindable(other).map_or(*other, |o| crate::i18n::tr(o.label));
        crate::i18n::tr_format!("Same key as {}: {}.", name, why)
    });
    let cell = column(cx.cols.when_x(), cx.cols.when);
    in_cell(ui, cell, |ui| {
        when_cell::cell(app, ui, t, b.id, cell, reason.as_deref());
    });

    if resp.clicked() {
        pick(app, row, None);
    }
    crate::widgets::context_menu(&resp, |ui| {
        pick(app, row, None);
        row_menu(app, ui, row, changed);
    });
}

fn hover_text(b: &Bindable, row: &Row) -> String {
    // keys whose command changes in the Library grids (see `shortcuts::handle`)
    let hover = match b.id {
        "panel.presets" => format!("{} — {}", b.id, crate::i18n::tr("In Photo Grid and Square Grid: Flag as Pick and advance")),
        "view.softProof" => format!("{} — {}", b.id, crate::i18n::tr("In Photo Grid and Square Grid: expand or collapse the stack")),
        _ => b.id.to_string(),
    };
    if row.path.is_empty() { hover } else { format!("{hover}\n{}", row.path) }
}

/// Right-click on a row: record another shortcut, give it a condition, put it back as declared,
/// take the picked shortcut away.
fn row_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, row: &Row, changed: bool) {
    let id = row.b.id;
    let full = row.slots.iter().all(Option::is_some);
    let r = ui.button(crate::i18n::tr(if full { "Change Shortcut" } else { "Add Shortcut" }));
    register(ui, "menu:shortcutAdd", r.rect);
    if r.clicked() {
        let _ = capture::open_inline(app, id, None);
        ui.close();
    }
    let r = ui.button(crate::i18n::tr("Edit Condition…"));
    register(ui, "menu:shortcutCondition", r.rect);
    if r.clicked() {
        when_cell::start_edit(app, id);
        ui.close();
    }
    let slot = app.keybinds.selected.and_then(|(_, s)| removable_slot(&row.slots, s));
    let r = ui.add_enabled(slot.is_some(), egui::Button::new(crate::i18n::tr("Remove Shortcut")));
    register(ui, "menu:shortcutRemove", r.rect);
    if r.clicked()
        && let Some(slot) = slot
    {
        capture::apply(app, ui.ctx(), id, slot, None);
        ui.close();
    }
    let r = ui.add_enabled(changed, egui::Button::new(crate::i18n::tr("Reset to Default")));
    register(ui, "menu:shortcutReset", r.rect);
    if r.clicked() {
        super::reset(app, ui.ctx(), id);
        ui.close();
    }
}

/// What the ↺ restores: "Reset to ⌘N".
pub(super) fn reset_tip(b: &Bindable, mac: bool) -> String {
    match b.default {
        Some(d) => crate::i18n::tr_format!("Reset to {}", chord::text(d, mac)),
        None => crate::i18n::tr("Reset to no shortcut").to_string(),
    }
}

/// The ↺ on a changed row; its tooltip says what it restores.
fn reset_icon(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, b: &Bindable, mac: bool) {
    let (rect, r) = ui.allocate_exact_size(vec2(RESET_W, CELL_H), Sense::click());
    let tip = reset_tip(b, mac);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip.clone()));
    register(ui, format!("button:shortcutReset-{}", b.id), rect);
    if r.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover.gamma_multiply(0.7));
    }
    let color = if r.hovered() { t.text } else { t.icon };
    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center(), vec2(14.0, 14.0)), crate::icons::Icon::Reset, color);
    if r.on_hover_text(tip).clicked() {
        super::reset(app, ui.ctx(), b.id);
    }
}

/// One shortcut cell: a field showing the shortcut (or the keys being recorded). A click records
/// into it; right-click edits or removes.
fn slot_cell(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, row: &Row, slot: usize, cx: &Ctx) {
    let b = row.b;
    let sc = row.slots.get(slot).and_then(Option::as_deref);
    let cap = app.keybinds.capture.as_ref().filter(|c| c.inline && c.id == b.id && c.slot == slot);
    let recording = cap.is_some();
    let clash = cx.clashes.slots.get(&(b.id, slot));
    let (rect, r) = ui.allocate_exact_size(vec2(SLOT_W, CELL_H), Sense::click());
    let widget = if slot == 0 { format!("button:shortcut-{}", b.id) } else { format!("button:shortcut{}-{}", slot + 1, b.id) };
    register(ui, widget, rect);
    let (text, color) = match (cap, sc) {
        (Some(c), _) => match c.text() {
            Some(s) => (chord::text(&s, cx.mac), if cx.live_clash { t.caution } else { t.text }),
            None => (crate::i18n::tr("Press the keys…").to_string(), t.text_dim),
        },
        (None, Some(sc)) => (chord::text(sc, cx.mac), if clash.is_some() { t.caution } else { t.text }),
        (None, None) => (String::new(), t.text_dim),
    };
    let label =
        crate::i18n::tr_format!("Shortcut {}: {}", slot + 1, if text.is_empty() { crate::i18n::tr("none").to_string() } else { text.clone() });
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} — {}", crate::i18n::tr(b.label), label)));
    let picked_here = app.keybinds.selected == Some((b.id, slot));
    let stroke = if recording {
        egui::Stroke::new(1.5, if cx.live_clash { t.caution } else { t.accent })
    } else if r.hovered() {
        egui::Stroke::new(1.0, t.button_border)
    } else if picked_here {
        egui::Stroke::new(1.0, t.accent.gamma_multiply(0.6))
    } else {
        egui::Stroke::new(1.0, t.field_border.gamma_multiply(0.55))
    };
    ui.painter().rect(rect, 4.0, t.field, stroke, egui::StrokeKind::Inside);
    if !text.is_empty() {
        let font = if recording { t.semibold(12.5) } else { t.font(12.5) };
        ui.painter().with_clip_rect(rect.shrink(3.0)).text(rect.center(), Align2::CENTER_CENTER, text, font, color);
    }
    // a bare trash icon at the cell's right edge (no box): on the cell the pointer is over, and on the picked one
    if sc.is_some() && !recording && (ui.rect_contains_pointer(rect) || picked_here) {
        trash(app, ui, t, b, slot, rect);
    }
    let mut r = r;
    if let Some(others) = clash {
        let names: Vec<_> = others.iter().map(|id| shortcuts::find_bindable(id).map_or(*id, |o| crate::i18n::tr(o.label))).collect();
        r = r.on_hover_text(crate::i18n::tr_format!("Also used by {}", names.join(", ")));
    }
    if r.clicked() {
        app.keybinds.selected = Some((b.id, slot));
        let _ = capture::open_inline(app, b.id, Some(slot));
    }
    crate::widgets::context_menu(&r, |ui| {
        app.keybinds.selected = Some((b.id, slot));
        if ui.button(crate::i18n::tr("Edit Shortcut…")).clicked() {
            let _ = capture::open_inline(app, b.id, Some(slot));
            ui.close();
        }
        let r = ui.add_enabled(sc.is_some(), egui::Button::new(crate::i18n::tr("Remove Shortcut")));
        register(ui, format!("menu:shortcutRemove-{slot}"), r.rect);
        if r.clicked() {
            capture::apply(app, ui.ctx(), b.id, slot, None);
            ui.close();
        }
    });
}

/// The trash icon that removes the shortcut in `slot`: no background, no border, quiet until hovered.
fn trash(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, b: &Bindable, slot: usize, cell: egui::Rect) {
    let rect = egui::Rect::from_center_size(egui::pos2(cell.right() - 13.0, cell.center().y), vec2(20.0, 20.0));
    let r = ui.interact(rect, ui.scope_id().with(("shortcut-clear", b.id, slot)), Sense::CLICK | Sense::FOCUSABLE);
    let tip = crate::i18n::tr("Remove Shortcut");
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    let id = if slot == 0 { format!("button:shortcutClear-{}", b.id) } else { format!("button:shortcutClear{}-{}", slot + 1, b.id) };
    register(ui, id, rect);
    let color = if r.hovered() { t.text } else { t.text_dim };
    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center(), vec2(13.0, 13.0)), crate::icons::Icon::Trash, color);
    if r.on_hover_text(tip).clicked() {
        capture::apply(app, ui.ctx(), b.id, slot, None);
    }
}
