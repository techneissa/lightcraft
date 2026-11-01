//! The page strip: every page as a thumbnail along the bottom. Only the pages on screen are
//! drawn (the scroll area reports what is visible; the rest is empty space), so a book of 200 pages
//! draws what a book of 12 does. A click jumps to the page, a drag reorders, + adds a page.
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lightcraft_engine::book::model::Book;
use serde_json::json;

use super::thumbs;
use super::{BookUi, Drag, flush, run};
use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// Gap between pages, points; and the extra gap between spreads.
const GAP: f32 = 6.0;
const SPREAD_GAP: f32 = 10.0;
/// Room under the thumbnails for the page number.
const LABEL_H: f32 = 16.0;

/// Left edge of page `index` in the strip, and the strip's width up to its end.
fn x_of(index: usize, w: f32) -> f32 {
    // pages 0 | 1 2 | 3 4 …: a wider gap before each spread
    let spreads = (index + 1) / 2;
    index as f32 * (w + GAP) + spreads as f32 * SPREAD_GAP
}

pub(super) fn show(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book, room: Rect) {
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(room, 0.0, t.chrome);
    ui.painter().line_segment([room.left_top(), room.right_top()], Stroke::new(1.0, t.divider));
    register(ui, "book:strip", room);
    let Ok((pw, ph)) = book.settings.page_size() else { return };
    let thumb_h = room.height() - LABEL_H - 16.0;
    let thumb_w = (f64::from(thumb_h) * pw / ph) as f32;
    let n = book.pages.len();
    let plus_w = 44.0;
    let content_w = if n == 0 { 0.0 } else { x_of(n - 1, thumb_w) + thumb_w } + GAP + plus_w + 16.0;
    let mut area = egui::ScrollArea::horizontal().id_salt("book-strip").auto_shrink([false, false]);
    if std::mem::take(&mut bu.reveal_page) && n > 0 {
        // keep the page on show in view (the offset that centres it)
        let x = x_of(bu.page.min(n - 1), thumb_w) + thumb_w / 2.0 + 8.0;
        area = area.horizontal_scroll_offset((x - room.width() / 2.0).max(0.0));
    }
    let mut dropped: Option<(usize, usize)> = None;
    let mut add = false;
    area.show_viewport(&mut ui.new_child(egui::UiBuilder::new().max_rect(room)), |ui, view| {
        ui.set_width(content_w);
        ui.set_height(room.height() - 4.0);
        let top = ui.min_rect().top() + 8.0;
        let left = ui.min_rect().left() + 8.0;
        // the pages whose thumbnails are in view
        let first = (0..n).find(|i| x_of(*i, thumb_w) + thumb_w >= view.min.x - 8.0).unwrap_or(n);
        let last = (first..n).find(|i| x_of(*i, thumb_w) > view.max.x).unwrap_or(n);
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let mut insert_at: Option<f32> = None;
        for index in first..last {
            let rect = Rect::from_min_size(pos2(left + x_of(index, thumb_w), top), vec2(thumb_w, thumb_h));
            let painter = ui.painter_at(room);
            thumbs::paint_page(app, &painter, book, index, rect, None, true);
            register(ui, format!("book:thumb:{index}"), rect);
            if index == bu.page {
                painter.rect_stroke(rect.expand(1.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Outside);
            }
            if book.pages.get(index).is_some_and(|p| p.locked) {
                super::spread::padlock(&painter, rect.left_top() + vec2(3.0, 3.0), Color32::from_gray(60));
            }
            let tag = if index == 0 && book.cover { crate::i18n::tr("Cover").to_string() } else { (index + 1).to_string() };
            painter.text(
                pos2(rect.center().x, rect.bottom() + 9.0),
                egui::Align2::CENTER_CENTER,
                tag,
                t.font(10.5),
                if index == bu.page { t.accent } else { t.text_dim },
            );
            let resp = ui.interact(rect, egui::Id::unique(("book-thumb", index)), Sense::click_and_drag());
            if resp.clicked() {
                flush(app, &ctx, bu);
                bu.page = index;
            }
            // the cover stays first: it can't be dragged, and nothing is dropped before it
            let movable = !(index == 0 && book.cover);
            if resp.drag_started() && movable {
                bu.drag = Some(Drag::Page(index));
            }
            if let Some(Drag::Page(from)) = bu.drag
                && let Some(p) = pointer
                && rect.contains(p)
                && from != index
                && !(index == 0 && book.cover)
            {
                insert_at = Some(if index > from { rect.right() + GAP / 2.0 } else { rect.left() - GAP / 2.0 });
                if ctx.input(|i| i.pointer.any_released()) {
                    dropped = Some((from, index));
                }
            }
        }
        if let Some(x) = insert_at {
            ui.painter().line_segment([pos2(x, top), pos2(x, top + thumb_h)], Stroke::new(3.0, t.accent));
        }
        // +: a page after the one on show
        let plus =
            Rect::from_min_size(pos2(left + if n == 0 { 0.0 } else { x_of(n - 1, thumb_w) + thumb_w + GAP + 4.0 }, top), vec2(plus_w, thumb_h));
        let resp = ui.interact(plus, egui::Id::unique("book-add-page"), Sense::click());
        register(ui, "button:bookAddPage", plus);
        let fill = if resp.hovered() { t.hover } else { t.button };
        ui.painter().rect(plus, 4.0, fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        crate::icons::paint(ui.painter(), Rect::from_center_size(plus.center(), vec2(18.0, 18.0)), crate::icons::Icon::Plus, t.text);
        crate::widgets::announce(&resp, crate::i18n::tr("Add Page"));
        if resp.clicked() {
            add = true;
        }
    });
    if ctx.input(|i| i.pointer.any_released()) && matches!(bu.drag, Some(Drag::Page(_))) {
        bu.drag = None;
    }
    if let Some((from, to)) = dropped {
        run(app, &ctx, bu, "book.movePage", json!({"from": from, "to": to}));
        bu.page = to;
        bu.reveal_page = true;
    }
    if add {
        let at = if n == 0 { None } else { Some(bu.page + 1) };
        run(app, &ctx, bu, "book.addPage", at.map_or(json!({}), |a| json!({"at": a})));
        if n > 0 {
            bu.page += 1;
        }
        bu.reveal_page = true;
    }
}
