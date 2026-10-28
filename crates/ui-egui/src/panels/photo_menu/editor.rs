//! Settings ▸ Photo Menu: the photo menu's rows on the left, in order, and what can be added on
//! the right (the built-in items not in it, then every menu-bar command), with a search box.
//!
//! - Drag a row up or down to move it; drag an item from the right into the list to put it there.
//! - × on a row takes it out; + on the right (or a double-click) adds the item below the row
//!   picked on the left, or at the end. Add Separator does the same with a line.
//! - Keys, with a row picked and no text box in use: ↑ / ↓ pick another, ⌥↑ / ⌥↓ move it, Delete
//!   or Backspace takes it out. Reopening the tab picks nothing.
//! - Reset to Default puts the menu back as it came.
//!
//! Changes apply at once, as everywhere in Settings. The search text and the picked row live in
//! egui's memory: they are the window's, not settings.

use egui::{Color32, Key, Modifiers, Rect, Sense, Stroke, pos2, vec2};

use super::{Slot, items, menu_commands, save, slots};
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::panels::settings::{heading, hint};
use crate::theme::Tokens;
use crate::widgets::register;

const ROW_H: f32 = 26.0;
/// The least height of the lists; in a window with less room the page scrolls.
const MIN_LIST_H: f32 = 180.0;
/// The most: the page itself is never taller than the Settings window's page height.
const MAX_LIST_H: f32 = 1400.0;
/// What the columns and the row of buttons under them take besides the list: the "In the Menu"
/// label and the list's frame, the gap, the Add Separator row, and a line of margin at the end.
const BELOW_LIST: f32 = 72.0;
const GAP: f32 = 12.0;
/// The share of the width the menu's list takes; the rest is the list to add from.
const LEFT_SHARE: f32 = 0.54;

/// What is being dragged: a row of the menu (its index), or an item to add.
#[derive(Clone, Debug)]
enum Drag {
    Row(usize),
    Add(String),
}

/// The window's own state: the search text and the row picked on the left.
#[derive(Clone, Debug, Default)]
pub(super) struct View {
    pub(super) query: String,
    pub(super) picked: Option<usize>,
    /// Scroll the picked row into view on the next draw (it was just added or moved by key).
    pub(super) reveal: bool,
    /// The height of the menu's list, from the room the page has (set each pass by [`list_height`]).
    pub(super) list_h: f32,
    /// The pass the tab was last drawn in, and the menu as it left it: when the tab opens again,
    /// or the menu changed elsewhere (`photoMenu.set`), the picked row no longer means anything.
    drawn: u64,
    menu: Vec<String>,
}

/// One thing the right-hand list offers.
#[derive(Clone, Debug)]
pub struct Available {
    pub slot: Slot,
    pub label: String,
    /// Where it comes from: `Photo menu item`, or the menu-bar path (`View › Before/After`).
    pub path: String,
}

/// What can be added to a menu holding `in_menu`, matching `query` (every word in the label or
/// path, any case): the built-in items not in it, then the menu-bar commands not in it.
pub fn available(commands: &[(crate::menus::menubar::MenuNode, String)], in_menu: &[Slot], query: &str) -> Vec<Available> {
    let mut out: Vec<Available> = items::BUILTINS
        .iter()
        .filter(|b| !in_menu.iter().any(|s| matches!(s, Slot::Item(i) if i.id == b.id)))
        .map(|b| Available { slot: Slot::Item(b), label: items::label(b), path: crate::i18n::tr("Photo menu item").to_string() })
        .collect();
    for (node, path) in commands {
        if let crate::menus::menubar::MenuNode::Item { id, params, label, .. } = node {
            let slot = Slot::Command { id: id.clone(), params: params.clone() };
            if in_menu.contains(&slot) {
                continue;
            }
            out.push(Available { label: crate::menus::menubar::display_item_label(id, params, label).to_string(), slot, path: path.clone() });
        }
    }
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    out.retain(|a| {
        let hay = format!("{} {}", a.label, a.path).to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    });
    out
}

/// The note a row carries: when it hides.
fn note(slot: &Slot, commands: &[(crate::menus::menubar::MenuNode, String)]) -> Option<String> {
    let when = match slot {
        Slot::Item(b) => b.when,
        Slot::Command { id, params } if super::command_row(commands, id, params).is_none() => {
            return Some(crate::i18n::tr("not available").to_string());
        }
        _ => None,
    };
    match (slot.several(), when) {
        (true, _) => Some(crate::i18n::tr("2+ Photos").to_string()),
        (false, Some(w)) => Some(crate::i18n::tr(w).to_string()),
        _ => None,
    }
}

/// A row's label in the list.
fn row_label(slot: &Slot, commands: &[(crate::menus::menubar::MenuNode, String)]) -> String {
    match slot {
        Slot::Item(b) => items::label(b),
        Slot::Command { id, params } => {
            super::command_row(commands, id, params).map(|r| r.label).unwrap_or_else(|| crate::menus::menubar::MenuNode::key(id, params))
        }
        Slot::Separator => String::new(),
    }
}

/// The height of the lists: what the page's viewport has left below `used` (what the page holds
/// above the lists) once the buttons under them are counted, never under [`MIN_LIST_H`]: a page
/// with less room scrolls instead. The viewport is the one the page scrolls in, so the height does
/// not change as the page is scrolled.
pub fn list_height(viewport: f32, used: f32) -> f32 {
    (viewport - used - BELOW_LIST).clamp(MIN_LIST_H, MAX_LIST_H)
}

/// The Photo Menu tab.
pub fn tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let origin = ui.min_rect().top();
    let view_id = egui::Id::unique("photo-menu-editor");
    let mut view: View = ui.ctx().data(|d| d.get_temp(view_id)).unwrap_or_default();
    let mut menu = slots(app);
    let before: Vec<String> = menu.iter().map(Slot::key).collect();
    let pass = ui.ctx().cumulative_pass_nr();
    if pass > view.drawn.saturating_add(1) || view.menu != before {
        view.picked = None;
    }
    let commands = menu_commands(app);

    ui.horizontal(|ui| {
        heading(ui, t, "Photo Menu");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| super::presets_ui::show(app, ui, t));
    });
    hint(ui, t, "What a right-click on a photo shows. Items marked \"2+ Photos\" show only when two or more photos are selected.");
    ui.add_space(4.0);

    view.list_h = list_height(ui.clip_rect().height(), ui.cursor().top() - origin);
    let width = ui.available_width() - 2.0;
    let left_w = ((width - GAP) * LEFT_SHARE).floor();
    let right_w = (width - GAP - left_w).max(120.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.vertical(|ui| {
            ui.set_width(left_w);
            ui.label(egui::RichText::new(crate::i18n::tr("In the Menu")).color(t.text_label));
            menu_list(ui, t, &mut menu, &commands, &mut view, left_w);
        });
        ui.vertical(|ui| {
            ui.set_width(right_w);
            add_list(ui, t, &mut menu, &commands, &mut view, right_w);
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let r = ui.button(crate::i18n::tr("Add Separator"));
        register(ui, "button:photoMenuAddSeparator", r.rect);
        if r.clicked() {
            insert(&mut menu, &mut view, Slot::Separator);
        }
        let custom = app.ui.settings.photo_menu.is_some();
        let r = ui.add_enabled(custom, egui::Button::new(crate::i18n::tr("Reset to Default")));
        register(ui, "button:photoMenuReset", r.rect);
        if r.clicked() {
            menu = super::default_slots();
            view.picked = None;
        }
    });

    keys(ui, &mut menu, &mut view);
    if menu.iter().map(Slot::key).ne(before.iter().cloned()) {
        save(app, &menu);
    }
    view.drawn = pass;
    view.menu = slots(app).iter().map(Slot::key).collect();
    ui.ctx().data_mut(|d| d.insert_temp(view_id, view));
}

/// Put `slot` below the picked row (at the end when none is picked) and pick it.
pub(super) fn insert(menu: &mut Vec<Slot>, view: &mut View, slot: Slot) {
    let at = view.picked.map_or(menu.len(), |p| (p + 1).min(menu.len()));
    menu.insert(at, slot);
    view.picked = Some(at);
    view.reveal = true;
}

/// Move the row at `from` so it lands before the row that was at `to` (`to` may be the length:
/// the end).
pub(super) fn move_row(menu: &mut Vec<Slot>, view: &mut View, from: usize, to: usize) {
    if from >= menu.len() || to > menu.len() || to == from || to == from + 1 {
        return;
    }
    let slot = menu.remove(from);
    let at = if to > from { to - 1 } else { to };
    menu.insert(at, slot);
    view.picked = Some(at);
}

pub(super) fn remove(menu: &mut Vec<Slot>, view: &mut View, at: usize) {
    if at >= menu.len() {
        return;
    }
    menu.remove(at);
    view.picked = match view.picked {
        Some(p) if p == at => (!menu.is_empty()).then(|| at.min(menu.len() - 1)),
        Some(p) if p > at => Some(p - 1),
        p => p,
    };
}

/// The six dots of a drag handle.
fn grip(p: &egui::Painter, centre: egui::Pos2, color: Color32) {
    for dx in [-2.5, 2.5] {
        for dy in [-4.0, 0.0, 4.0] {
            p.circle_filled(centre + vec2(dx, dy), 1.2, color);
        }
    }
}

fn menu_list(
    ui: &mut egui::Ui,
    t: &Tokens,
    menu: &mut Vec<Slot>,
    commands: &[(crate::menus::menubar::MenuNode, String)],
    view: &mut View,
    width: f32,
) {
    let frame = egui::Frame::new().fill(t.inset).stroke(Stroke::new(1.0, t.divider)).corner_radius(6.0).inner_margin(4.0);
    let list = frame.show(ui, |ui| {
        ui.set_width(width - 10.0);
        egui::ScrollArea::vertical()
            .id_salt("photo-menu-rows")
            .max_height(view.list_h)
            .min_scrolled_height(view.list_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut rects = Vec::with_capacity(menu.len());
                let mut action: Option<(usize, bool)> = None; // (row, removed)
                for (i, slot) in menu.iter().enumerate() {
                    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
                    rects.push(rect);
                    let id_key = format!("photoMenuRow:{i}");
                    register(ui, &id_key, rect);
                    let picked = view.picked == Some(i);
                    let dragging_this = matches!(egui::DragAndDrop::payload::<Drag>(ui.ctx()).as_deref(), Some(Drag::Row(d)) if *d == i);
                    let p = ui.painter();
                    if picked {
                        p.rect_filled(rect, 4.0, t.cell_selected);
                    } else if r.hovered() && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
                        p.rect_filled(rect, 4.0, t.hover);
                    }
                    let dim = if dragging_this { 0.35 } else { 1.0 };
                    grip(p, pos2(rect.left() + 9.0, rect.center().y), t.text_dim.gamma_multiply(dim));
                    // ✕ at the right end
                    let x_rect = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(18.0, 18.0));
                    let mut text_right = x_rect.left() - 4.0;
                    if *slot == Slot::Separator {
                        let y = rect.center().y;
                        p.line_segment([pos2(rect.left() + 22.0, y), pos2(text_right, y)], Stroke::new(1.0, t.text_dim.gamma_multiply(0.6 * dim)));
                    } else {
                        if matches!(slot, Slot::Item(b) if b.submenu) {
                            let c = Rect::from_center_size(pos2(text_right - 7.0, rect.center().y), vec2(12.0, 12.0));
                            paint(p, c, Icon::ChevronRight, t.text_dim.gamma_multiply(dim));
                            text_right -= 16.0;
                        }
                        if let Some(n) = note(slot, commands) {
                            let g = p.layout_no_wrap(n, t.font(10.5), t.text_dim.gamma_multiply(dim));
                            text_right -= g.size().x;
                            p.galley(pos2(text_right, rect.center().y - g.size().y / 2.0), g, t.text_dim);
                            text_right -= 6.0;
                        }
                        let color = match slot {
                            Slot::Command { id, params } if super::command_row(commands, id, params).is_none() => t.text_disabled,
                            Slot::Command { .. } => t.text_label,
                            _ => t.text,
                        };
                        let label = row_label(slot, commands);
                        let max = (text_right - rect.left() - 22.0).max(10.0);
                        let mut job = egui::text::LayoutJob::single_section(label, egui::TextFormat::simple(t.font(12.5), color.gamma_multiply(dim)));
                        job.wrap = egui::text::TextWrapping::truncate_at_width(max);
                        let g = ui.fonts_mut(|f| f.layout_job(job));
                        ui.painter().galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, color);
                    }
                    if ui.rect_contains_pointer(rect) || picked {
                        let xr = ui.interact(x_rect, ui.scope_id().with(("photoMenuRemove", i)), Sense::click());
                        register(ui, format!("button:photoMenuRemove:{i}"), x_rect);
                        let c = if xr.hovered() { t.text } else { t.text_dim };
                        if xr.hovered() {
                            ui.painter().rect_filled(x_rect, 4.0, t.pressed);
                        }
                        paint(ui.painter(), x_rect.shrink(3.0), Icon::Close, c);
                        if xr.clicked() {
                            action = Some((i, true));
                        }
                        xr.on_hover_text(crate::i18n::tr("Take out of the menu"));
                    }
                    crate::widgets::announce(
                        &r,
                        if *slot == Slot::Separator { crate::i18n::tr("Separator").to_string() } else { row_label(slot, commands) },
                    );
                    if r.clicked() {
                        action = action.or(Some((i, false)));
                    }
                    if r.drag_started() {
                        egui::DragAndDrop::set_payload(ui.ctx(), Drag::Row(i));
                    }
                    if picked && view.reveal {
                        r.scroll_to_me(None);
                        view.reveal = false;
                    }
                }
                edge_scroll(ui);
                match action {
                    Some((i, true)) => remove(menu, view, i),
                    Some((i, false)) => view.picked = Some(i),
                    None => {}
                }
                rects
            })
    });
    let rects = list.inner.inner;
    let list_rect = list.response.rect;
    crate::widgets::register_geometry(ui.ctx(), "photoMenu:list", list_rect);
    drop_into(ui, t, menu, commands, view, list_rect, &rects);
}

/// While a row or an item is dragged near the top or bottom of the list, the list scrolls that
/// way, so a row can be taken anywhere in a menu taller than the list.
fn edge_scroll(ui: &egui::Ui) {
    const EDGE: f32 = 28.0;
    const STEP: f32 = 8.0;
    if !egui::DragAndDrop::has_payload_of_type::<Drag>(ui.ctx()) {
        return;
    }
    let Some(p) = ui.ctx().pointer_latest_pos() else { return };
    let clip = ui.clip_rect();
    if !clip.x_range().contains(p.x) {
        return;
    }
    let dy = if (clip.top() - EDGE..clip.top() + EDGE).contains(&p.y) {
        STEP
    } else if (clip.bottom() - EDGE..clip.bottom() + EDGE).contains(&p.y) {
        -STEP
    } else {
        return;
    };
    ui.scroll_with_delta(vec2(0.0, dy));
    ui.ctx().request_repaint();
}

/// While something is dragged over the list: a line where it would go, and the move or insert
/// when it is let go there. A drag let go anywhere else changes nothing.
fn drop_into(
    ui: &mut egui::Ui,
    t: &Tokens,
    menu: &mut Vec<Slot>,
    commands: &[(crate::menus::menubar::MenuNode, String)],
    view: &mut View,
    list_rect: Rect,
    rects: &[Rect],
) {
    let ctx = ui.ctx().clone();
    let Some(drag) = egui::DragAndDrop::payload::<Drag>(&ctx) else { return };
    let Some(pointer) = ctx.pointer_latest_pos() else { return };
    // the thing dragged follows the pointer
    let label = match drag.as_ref() {
        Drag::Row(i) => menu.get(*i).map(|s| if *s == Slot::Separator { crate::i18n::tr("Separator").to_string() } else { row_label(s, commands) }),
        Drag::Add(key) => {
            Slot::parse(key).map(|s| if s == Slot::Separator { crate::i18n::tr("Separator").to_string() } else { row_label(&s, commands) })
        }
    };
    if let Some(label) = label {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("photo-menu-drag"));
        let p = ctx.layer_painter(layer);
        let g = p.layout_no_wrap(label, t.font(12.5), t.text);
        let r = Rect::from_min_size(pointer + vec2(12.0, -ROW_H / 2.0), vec2(g.size().x + 20.0, ROW_H));
        p.rect(r, 4.0, t.chrome, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        p.galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, t.text);
    }
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let released = ctx.input(|i| i.pointer.any_released());
    if !list_rect.contains(pointer) {
        if released {
            egui::DragAndDrop::clear_payload(&ctx);
        }
        return;
    }
    // the gap nearest the pointer: before the first row whose middle is below it
    let at = rects.iter().position(|r| pointer.y < r.center().y).unwrap_or(rects.len());
    let y = match rects.get(at) {
        Some(r) => r.top(),
        None => rects.last().map_or(list_rect.top() + 4.0, |r| r.bottom()),
    };
    let y = y.clamp(list_rect.top() + 2.0, list_rect.bottom() - 2.0);
    ui.painter()
        .with_clip_rect(list_rect)
        .line_segment([pos2(list_rect.left() + 6.0, y), pos2(list_rect.right() - 6.0, y)], Stroke::new(2.0, t.accent));
    if released {
        match egui::DragAndDrop::take_payload::<Drag>(&ctx).as_deref() {
            Some(Drag::Row(from)) => move_row(menu, view, *from, at),
            Some(Drag::Add(key)) => {
                if let Some(slot) = Slot::parse(key) {
                    menu.insert(at.min(menu.len()), slot);
                    view.picked = Some(at.min(menu.len() - 1));
                }
            }
            None => {}
        }
    }
}

fn add_list(
    ui: &mut egui::Ui,
    t: &Tokens,
    menu: &mut Vec<Slot>,
    commands: &[(crate::menus::menubar::MenuNode, String)],
    view: &mut View,
    width: f32,
) {
    crate::widgets::text_field::TextField::singleline("field:photoMenuSearch", &mut view.query)
        .hint(crate::i18n::tr("Search items and commands"))
        .width(width)
        .show(ui);
    let offered = available(commands, menu, &view.query);
    let frame = egui::Frame::new().fill(t.inset).stroke(Stroke::new(1.0, t.divider)).corner_radius(6.0).inner_margin(4.0);
    let mut add: Option<Slot> = None;
    frame.show(ui, |ui| {
        ui.set_width(width - 10.0);
        // (before the scroll area: show_rows spaces its rows by the spacing it is given)
        ui.spacing_mut().item_spacing.y = 0.0;
        let h = view.list_h - ROW_H - 2.0;
        egui::ScrollArea::vertical().id_salt("photo-menu-add").max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show_rows(
            ui,
            ROW_H + 8.0,
            offered.len(),
            |ui, range| {
                if offered.is_empty() {
                    ui.label(egui::RichText::new(crate::i18n::tr("Nothing matches")).color(t.text_dim));
                }
                for a in offered.get(range).unwrap_or_default() {
                    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H + 8.0), Sense::click_and_drag());
                    let key = a.slot.key();
                    register(ui, format!("photoMenuAdd:{key}"), rect);
                    let p = ui.painter();
                    if r.hovered() && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
                        p.rect_filled(rect, 4.0, t.hover);
                    }
                    let plus = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(18.0, 18.0));
                    let max = (plus.left() - rect.left() - 12.0).max(10.0);
                    let line = |text: &str, font: egui::FontId, color: Color32| {
                        let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(font, color));
                        job.wrap = egui::text::TextWrapping::truncate_at_width(max);
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let g1 = line(&a.label, t.font(12.5), t.text);
                    let g2 = line(&a.path, t.font(10.5), t.text_dim);
                    p.galley(pos2(rect.left() + 6.0, rect.top() + 3.0), g1, t.text);
                    p.galley(pos2(rect.left() + 6.0, rect.top() + 19.0), g2, t.text_dim);
                    let pr = ui.interact(plus, ui.scope_id().with(("photoMenuPlus", &key)), Sense::click());
                    register(ui, format!("button:photoMenuAdd:{key}"), plus);
                    if pr.hovered() {
                        ui.painter().rect_filled(plus, 4.0, t.pressed);
                    }
                    paint(ui.painter(), plus.shrink(3.0), Icon::Plus, if pr.hovered() || r.hovered() { t.text } else { t.text_dim });
                    crate::widgets::announce(&pr, crate::i18n::tr_format!("Add {label}", label = a.label));
                    crate::widgets::announce(&r, a.label.clone());
                    if pr.on_hover_text(crate::i18n::tr("Add to the menu")).clicked() || r.double_clicked() {
                        add = Some(a.slot.clone());
                    }
                    if r.drag_started() {
                        egui::DragAndDrop::set_payload(ui.ctx(), Drag::Add(key));
                    }
                }
            },
        );
    });
    if let Some(slot) = add {
        insert(menu, view, slot);
    }
}

/// ↑ / ↓ pick a row, ⌥↑ / ⌥↓ move it, Delete / Backspace take it out: only while no text box has
/// the keyboard.
fn keys(ui: &egui::Ui, menu: &mut Vec<Slot>, view: &mut View) {
    if ui.ctx().memory(|m| m.focused().is_some()) || menu.is_empty() {
        return;
    }
    let Some(p) = view.picked.filter(|p| *p < menu.len()) else { return };
    ui.ctx().input_mut(|i| {
        if i.consume_key(Modifiers::ALT, Key::ArrowUp) && p > 0 {
            menu.swap(p, p - 1);
            view.picked = Some(p - 1);
            view.reveal = true;
        } else if i.consume_key(Modifiers::ALT, Key::ArrowDown) && p + 1 < menu.len() {
            menu.swap(p, p + 1);
            view.picked = Some(p + 1);
            view.reveal = true;
        } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            view.picked = Some(p.saturating_sub(1));
            view.reveal = true;
        } else if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            view.picked = Some((p + 1).min(menu.len() - 1));
            view.reveal = true;
        } else if i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace) {
            remove(menu, view, p);
        }
    });
}
