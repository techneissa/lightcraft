//! The open spread: two pages side by side as printed (the cover alone), as large as the room
//! allows. Drag a photo from the pool or another frame onto a frame; press inside a frame and drag to
//! move the photo within it; scroll or pinch over a frame to zoom the photo; drop a photo on a
//! frame that holds one to swap them. A double click puts the photo back to the book's setting.
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::book::model::{Book, Crop, MAX_ZOOM, Slot};
use serde_json::json;

use super::thumbs::{self, Show};
use super::{BookUi, Drag, Hit, Pending, flush, run};
use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// Gap between the two pages of a spread, points.
const GAP: f32 = 3.0;
/// How far outside its frame the pointer may go while a press still moves the photo within it.
const STAY: f32 = 6.0;

/// The crop a pending change or the saved book holds for a cell.
fn crop_now(book: &Book, bu: &BookUi, page: usize, cell: usize) -> Option<Crop> {
    match bu.pending {
        Some(p) if p.book == book.id && (p.page, p.cell) == (page, cell) => Some(p.crop),
        _ => book.pages.get(page).and_then(|p| p.crop(cell)),
    }
}

pub(super) fn show(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book, room: Rect) {
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(room, 0.0, t.canvas);
    register(ui, "book:spread", room);
    let Ok((pw, ph)) = book.settings.page_size() else { return };
    let spreads = book.spreads();
    let Some(spread) = spreads.iter().find(|s| s.contains(&bu.page)).cloned() else {
        ui.painter().text(
            room.center(),
            egui::Align2::CENTER_CENTER,
            crate::i18n::tr("No pages yet. Add photos, then Auto Layout, or add a page in the strip."),
            t.font(13.0),
            t.text_dim,
        );
        return;
    };
    let n = spread.len() as f64;
    let inner = room.shrink2(vec2(24.0, 22.0));
    let scale = ((f64::from(inner.width()) - f64::from(GAP) * (n - 1.0)) / (pw * n)).min(f64::from(inner.height()) / ph).max(1.0);
    let (w, h) = ((pw * scale) as f32, (ph * scale) as f32);
    let total = vec2(w * spread.len() as f32 + GAP * (spread.len() as f32 - 1.0), h);
    let origin = room.center() - total / 2.0;
    let pending = bu.pending.filter(|p| p.book == book.id).map(|p| (p.page, p.cell, p.crop));
    let dropping = matches!(bu.drag, Some(Drag::Photo(_)) | Some(Drag::Cell { swap: true, .. }));
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    for (k, &index) in spread.iter().enumerate() {
        let rect = Rect::from_min_size(origin + vec2((w + GAP) * k as f32, 0.0), vec2(w, h));
        let painter = ui.painter_at(room);
        thumbs::paint_page(app, &painter, book, index, rect, pending, true);
        register(ui, format!("book:page:{index}"), rect);
        if index == bu.page {
            painter.rect_stroke(rect.expand(1.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Outside);
        }
        if book.pages.get(index).is_some_and(|p| p.locked) {
            padlock(&painter, rect.right_top() + vec2(-14.0, 6.0), Color32::from_gray(60));
        }
        let page_resp = ui.interact(rect, egui::Id::unique(("book-page", index)), Sense::click());
        if page_resp.clicked() {
            bu.page = index;
        }
        let Some(page) = book.pages.get(index) else { continue };
        for (c, frame) in thumbs::page_cells(book, index, rect).into_iter().enumerate() {
            let slot = Slot::Cell { page: index, cell: c };
            let photo = page.slots.get(c).copied().flatten();
            bu.hits.push((Hit::Cell { page: index, cell: c }, frame));
            register(ui, format!("book:cell:{index}:{c}"), frame);
            let resp = ui.interact(frame, egui::Id::unique(("book-cell", index, c)), Sense::click_and_drag());
            if dropping
                && pointer.is_some_and(|p| frame.contains(p))
                && !matches!(bu.drag, Some(Drag::Cell { page: pg, cell, .. }) if (pg, cell) == (index, c))
            {
                painter.rect_stroke(frame, 0.0, Stroke::new(2.5, t.accent), StrokeKind::Inside);
            }
            let Some(photo) = photo else { continue };
            frame_input(app, &ctx, bu, book, &resp, (index, c), frame, PhotoId(photo));
            let _ = slot;
        }
    }
}

/// A padlock, drawn: a page that Auto Layout leaves alone.
pub(super) fn padlock(painter: &egui::Painter, at: egui::Pos2, color: Color32) {
    let body = Rect::from_min_size(at + vec2(0.0, 5.0), vec2(9.0, 7.0));
    painter.rect_filled(body, 1.5, color);
    painter.rect_stroke(Rect::from_min_size(at + vec2(1.5, 0.0), vec2(6.0, 8.0)), 3.0, Stroke::new(1.5, color), StrokeKind::Middle);
}

/// What a press, drag, scroll, pinch or double click on a frame holding a photo does.
#[allow(clippy::too_many_arguments)]
fn frame_input(
    app: &mut LightcraftApp,
    ctx: &egui::Context,
    bu: &mut BookUi,
    book: &Book,
    resp: &egui::Response,
    (page, cell): (usize, usize),
    frame: Rect,
    photo: PhotoId,
) {
    let Some(aspect) = thumbs::aspect(app, photo) else { return };
    let now = ctx.input(|i| i.time);
    if resp.clicked() {
        bu.page = page;
    }
    if resp.double_clicked() {
        bu.pending = None;
        run(app, ctx, bu, "book.setCellCrop", json!({"page": page, "cell": cell, "reset": true}));
        return;
    }
    if resp.drag_started() {
        flush(app, ctx, bu);
        bu.page = page;
        bu.drag = Some(Drag::Cell { page, cell, swap: false });
    }
    if resp.dragged()
        && let Some(Drag::Cell { page: p, cell: c, swap }) = bu.drag
        && (p, c) == (page, cell)
    {
        let inside = resp.interact_pointer_pos().is_some_and(|pos| frame.expand(STAY).contains(pos));
        if swap || !inside {
            // it left the frame: a photo being carried (the move within the frame is undone)
            if !swap {
                bu.pending = None;
            }
            bu.drag = Some(Drag::Cell { page, cell, swap: true });
        } else {
            let next = thumbs::pan(crop_now(book, bu, page, cell), frame, resp.drag_delta(), aspect);
            bu.pending = Some(Pending { book: book.id, page, cell, crop: next, at: now });
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
    if resp.drag_stopped() && matches!(bu.drag, Some(Drag::Cell { swap: false, .. })) {
        bu.drag = None;
        flush(app, ctx, bu);
    }
    if resp.hovered() && bu.drag.is_none() {
        let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = f64::from(pinch) * (f64::from(scroll) * 0.004).exp();
        if (factor - 1.0).abs() > 1e-6 {
            let cur = crop_now(book, bu, page, cell).unwrap_or(Crop::FILL);
            let crop = Crop { zoom: (cur.zoom * factor).clamp(1.0, MAX_ZOOM), ..cur };
            // a crop that did not change (already at the limit) is no new step
            if crop != cur || bu.pending.is_none() && book.pages.get(page).and_then(|p| p.crop(cell)).is_none() {
                bu.pending = Some(Pending { book: book.id, page, cell, crop, at: now });
                ctx.request_repaint();
            }
        }
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
}

/// A photo that follows the pointer while it is carried, and what its release does: over a frame it
/// moves there (swapping with the photo that is there), over the pool it comes off its page.
pub(super) fn resolve_drag(app: &mut LightcraftApp, ctx: &egui::Context, bu: &mut BookUi, book: &Book) {
    let from = match bu.drag {
        Some(Drag::Photo(from)) => from,
        Some(Drag::Cell { page, cell, swap: true }) => Slot::Cell { page, cell },
        _ => return,
    };
    let pos = ctx.input(|i| i.pointer.latest_pos());
    let photo = match from {
        Slot::Cell { page, cell } => book.pages.get(page).and_then(|p| p.slots.get(cell).copied().flatten()),
        Slot::Unplaced { unplaced } => book.unplaced.get(unplaced).copied(),
    };
    if let (Some(pos), Some(photo)) = (pos, photo) {
        let ghost = Rect::from_center_size(pos + vec2(0.0, 0.0), vec2(58.0, 58.0));
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("book-ghost")));
        painter.rect_filled(ghost.expand(2.0), 3.0, Color32::from_black_alpha(130));
        thumbs::paint_photo(app, &painter, ghost, PhotoId(photo), Show::Fit);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    ctx.request_repaint();
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    bu.drag = None;
    let target = pos.and_then(|p| bu.hits.iter().rev().find(|(_, r)| r.contains(p)).map(|(h, _)| *h));
    let to = match target {
        Some(Hit::Cell { page, cell }) => Slot::Cell { page, cell },
        Some(Hit::Pool) if matches!(from, Slot::Cell { .. }) => Slot::Unplaced { unplaced: book.unplaced.len() },
        _ => return,
    };
    if to == from {
        return;
    }
    let slot = |s: Slot| match s {
        Slot::Cell { page, cell } => json!({"page": page, "cell": cell}),
        Slot::Unplaced { unplaced } => json!({"unplaced": unplaced}),
    };
    run(app, ctx, bu, "book.movePhoto", json!({"from": slot(from), "to": slot(to)}));
    if let Slot::Cell { page, .. } = to {
        bu.page = page;
    }
    let _ = pos2(0.0, 0.0);
}
