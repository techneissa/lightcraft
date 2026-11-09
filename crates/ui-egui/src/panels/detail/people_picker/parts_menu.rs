//! The parts dropdown of the people picker: one button naming the parts picked ("Entire Person", "Hair + Lips",
//! "3 Parts"), and the menu it opens below the card, its rows in groups under small headers, a check mark on each
//! part picked. A click picks one part and closes the menu; ⌘-click adds or removes one and keeps it open. ↑/↓
//! move a highlight, ↩ picks (⌘↩ adds), Esc closes; while it is open the keyboard is the menu's.
//!
//! Each row is the widget `<id>:part:<key>`. While the menu is closed, `ui.clickWidget` on a row opens it first
//! ([`crate::widgets::register_opener`]), so scripts click a part in one request.

use egui::{Align2, Color32, Key, Rect, Sense, pos2, vec2};

use super::{PartItem, Picker, select};
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

const ROW_H: f32 = 22.0;
const HEAD_H: f32 = 18.0;
const PAD: f32 = 4.0;
const MIN_W: f32 = 170.0;

/// Where the menu's open state is kept, per picker.
pub(super) fn open_id(pk: &Picker<'_>) -> egui::Id {
    egui::Id::unique((pk.id, "people-picker-parts-open"))
}

fn highlight_id(pk: &Picker<'_>) -> egui::Id {
    egui::Id::unique((pk.id, "people-picker-parts-highlight"))
}

pub(super) fn is_open(ctx: &egui::Context, pk: &Picker<'_>) -> bool {
    ctx.data(|d| d.get_temp::<bool>(open_id(pk))).unwrap_or(false)
}

pub(super) fn set_open(ctx: &egui::Context, pk: &Picker<'_>, open: bool) {
    ctx.data_mut(|d| {
        if open {
            d.insert_temp(open_id(pk), true);
        } else {
            d.remove::<bool>(open_id(pk));
            d.remove::<usize>(highlight_id(pk));
        }
    });
}

/// The button's text for the parts picked (`selected`, indexes of `parts`).
pub(super) fn summary(parts: &[PartItem], selected: &[usize]) -> String {
    let labels: Vec<&str> = selected.iter().filter_map(|i| parts.get(*i)).map(|p| p.label.as_str()).collect();
    match labels.as_slice() {
        [] => crate::i18n::tr("Parts").to_string(),
        [one] => crate::i18n::tr(one).to_string(),
        [a, b] => format!("{} + {}", crate::i18n::tr(a), crate::i18n::tr(b)),
        more => format!("{} {}", more.len(), crate::i18n::tr("Parts")),
    }
}

/// The dropdown button's width for `text` in `font`.
pub(super) fn button_width(p: &egui::Painter, text: &str, font: egui::FontId, max: f32) -> f32 {
    (p.layout_no_wrap(text.to_string(), font, Color32::WHITE).size().x + 30.0).min(max)
}

/// Draw the button at `rect` and, when open, the menu below `card`. The new parts selection, when it changed.
pub(super) fn show(ui: &mut egui::Ui, pk: &Picker<'_>, rect: Rect, card: Rect, text: &str) -> Option<Vec<usize>> {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let font = t.font(11.5);
    let p = ui.painter().clone();
    let resp = ui.interact(rect, egui::Id::unique((pk.id, "people-picker-parts")), Sense::click());
    register(ui, format!("{}:parts", pk.id), rect);
    let mut open = is_open(&ctx, pk);
    let fill = if open { t.accent } else { Color32::from_gray(if resp.hovered() { 85 } else { 60 }) };
    p.rect_filled(rect, rect.height() / 2.0, fill);
    let g = crate::widgets::one_line(&p, text, font.clone(), Color32::WHITE, rect.width() - 30.0);
    p.galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, Color32::WHITE);
    paint(&p, Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(14.0, 14.0)), Icon::ChevronDown, Color32::WHITE);
    if resp.on_hover_text(text).clicked() {
        open = !open;
        set_open(&ctx, pk, open);
    }
    if !open {
        crate::widgets::register_opener(&ctx, format!("{}:part:", pk.id), open_id(pk));
        return None;
    }
    crate::widgets::dropdown_search::mark_open(&ctx);
    let add = ui.input(|i| i.modifiers.command);
    let mut out = None;

    // the keyboard
    let n = pk.parts.len();
    let mut hi: Option<usize> = ctx.data(|d| d.get_temp::<usize>(highlight_id(pk)));
    let (down, up, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, Key::Enter) || i.consume_key(egui::Modifiers::COMMAND, Key::Enter),
            i.consume_key(egui::Modifiers::NONE, Key::Escape),
        )
    });
    if n > 0 {
        if down {
            hi = Some(hi.map_or(0, |h| (h + 1) % n));
        }
        if up {
            hi = Some(hi.map_or(n - 1, |h| (h + n - 1) % n));
        }
    }
    if let Some(h) = hi {
        ctx.data_mut(|d| d.insert_temp(highlight_id(pk), h));
    }
    if enter && let Some(h) = hi.filter(|h| *h < n) {
        out = Some(select(pk.selected_parts, h, add));
        if !add {
            set_open(&ctx, pk, false);
            return out;
        }
    }
    if esc {
        set_open(&ctx, pk, false);
        return None;
    }

    // the menu: as wide as its widest row, its right edge on the card's
    let widest =
        pk.parts.iter().map(|it| p.layout_no_wrap(crate::i18n::tr(&it.label).to_string(), font.clone(), Color32::WHITE).size().x).fold(0.0, f32::max);
    let w = (widest + 44.0).max(MIN_W).max(rect.width());
    let at = pos2(card.right() - w, card.bottom() + 4.0);
    let area = egui::Area::new(egui::Id::unique((pk.id, "people-picker-parts-menu"))).order(egui::Order::Foreground).fixed_pos(at).show(&ctx, |ui| {
        let frame = egui::Frame::new().fill(Color32::from_rgba_unmultiplied(28, 28, 28, 240)).corner_radius(6.0).inner_margin(PAD);
        frame.show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let row_w = w - 2.0 * PAD;
            for (i, it) in pk.parts.iter().enumerate() {
                if let Some(head) = it.group {
                    if i > 0 {
                        let (r, _) = ui.allocate_exact_size(vec2(row_w, 7.0), Sense::hover());
                        ui.painter().hline(r.x_range().shrink(6.0), r.center().y, egui::Stroke::new(1.0, Color32::from_gray(70)));
                    }
                    if !head.is_empty() {
                        let (r, _) = ui.allocate_exact_size(vec2(row_w, HEAD_H), Sense::hover());
                        ui.painter().text(
                            pos2(r.left() + 8.0, r.center().y),
                            Align2::LEFT_CENTER,
                            crate::i18n::tr(head),
                            t.semibold(10.5),
                            Color32::from_gray(150),
                        );
                    }
                }
                let (r, row) = ui.allocate_exact_size(vec2(row_w, ROW_H), Sense::click());
                register(ui, format!("{}:part:{}", pk.id, it.key), r);
                let on = pk.selected_parts.contains(&i);
                crate::widgets::announce_selected(&row, crate::i18n::tr(&it.label).to_string(), on);
                if row.hovered() || hi == Some(i) {
                    ui.painter().rect_filled(r, 4.0, Color32::from_white_alpha(28));
                }
                if on {
                    paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 13.0, r.center().y), vec2(14.0, 14.0)), Icon::Check, t.accent);
                }
                ui.painter().text(pos2(r.left() + 26.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::tr(&it.label), font.clone(), Color32::WHITE);
                if row.on_hover_text(crate::i18n::tr("⌘-click to select several")).clicked() {
                    out = Some(select(pk.selected_parts, i, add));
                    if !add {
                        set_open(&ctx, pk, false);
                    }
                }
            }
        });
    });
    register(ui, format!("{}:partsMenu", pk.id), area.response.rect);
    // a click outside the menu and its button closes it
    let outside =
        ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !rect.contains(p)));
    if outside {
        set_open(&ctx, pk, false);
    }
    out
}
