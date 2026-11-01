//! The right column: the book's photos (the pool) and its settings.
//!
//! The pool lists every photo of the book once for each place it has (a photo used twice shows
//! twice): the ones on no page first, then the ones on pages in page order, each placed one marked
//! with its page number. It is searched by file name, and only the rows on screen are drawn.
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::book::model::{Book, Slot};

use super::thumbs::{self, Show};
use super::{BookUi, Drag, Hit, Tab};
use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Cell size and gap in the pool's grid.
const CELL: f32 = 84.0;
const GAP: f32 = 6.0;

/// One place a photo has in the book.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Entry {
    pub slot: Slot,
    pub photo: u64,
}

/// The book's photos in pool order: the ones on no page, then the ones on pages. `keep` filters by photo.
pub(crate) fn entries(book: &Book, keep: &dyn Fn(u64) -> bool) -> Vec<Entry> {
    let tray = book.unplaced.iter().enumerate().map(|(i, p)| Entry { slot: Slot::Unplaced { unplaced: i }, photo: *p });
    let placed = book.pages.iter().enumerate().flat_map(|(page, pg)| {
        pg.slots.iter().enumerate().filter_map(move |(cell, p)| p.map(|p| Entry { slot: Slot::Cell { page, cell }, photo: p }))
    });
    tray.chain(placed).filter(|e| keep(e.photo)).collect()
}

pub(super) fn show(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book, room: Rect) {
    ui.painter().rect_filled(room, 0.0, t.chrome);
    ui.painter().line_segment([room.left_top(), room.left_bottom()], Stroke::new(1.0, t.divider));
    register(ui, "book:pool", room);
    bu.hits.push((Hit::Pool, room));
    let inner = room.shrink2(vec2(10.0, 8.0));
    let mut col = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    col.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (tab, id, label) in [(Tab::Photos, "bookTabPhotos", "Photos"), (Tab::Settings, "bookTabSettings", "Settings")] {
            if text_button(ui, id, label, bu.tab == tab).clicked() {
                bu.tab = tab;
            }
        }
    });
    col.add_space(6.0);
    match bu.tab {
        Tab::Photos => photos(app, &mut col, t, bu, book),
        Tab::Settings => super::settings::show(app, &mut col, t, bu, book),
    }
}

fn photos(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let placed = book.pages.iter().flat_map(|p| p.slots.iter().flatten()).count();
    ui.label(
        egui::RichText::new(crate::i18n::tr_format!("{n} photos · {m} on pages", n = book.photos().len(), m = placed))
            .font(t.font(11.5))
            .color(t.text_dim),
    );
    ui.add_space(3.0);
    crate::text_field::TextField::singleline("field:bookSearch", &mut bu.search)
        .hint(crate::i18n::tr("Search the book's photos"))
        .width(ui.available_width())
        .show(ui);
    ui.add_space(6.0);
    let needle = bu.search.trim().to_lowercase();
    let keep =
        |photo: u64| needle.is_empty() || app.session.catalog.photo(PhotoId(photo)).is_some_and(|p| p.file_name.to_lowercase().contains(&needle));
    let list = entries(book, &keep);
    if list.is_empty() {
        let text = if book.photos().is_empty() {
            crate::i18n::tr("No photos yet. Right-click photos in the library and choose Add to Book, or drag them onto this book.")
        } else {
            crate::i18n::tr("No photo matches.")
        };
        ui.label(egui::RichText::new(text).color(t.text_dim));
        return;
    }
    let per_row = (((ui.available_width() + GAP) / (CELL + GAP)).floor() as usize).max(1);
    let rows = list.len().div_ceil(per_row);
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().id_salt("book-pool").auto_shrink([false, false]).show_rows(ui, CELL + GAP, rows, |ui, range| {
        for row in range {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                for e in list.iter().skip(row * per_row).take(per_row) {
                    let (r, resp) = ui.allocate_exact_size(vec2(CELL, CELL), Sense::click_and_drag());
                    let painter = ui.painter_at(r);
                    painter.rect_filled(r, 3.0, t.canvas);
                    thumbs::paint_photo(app, &painter, r.shrink(3.0), PhotoId(e.photo), Show::Fit);
                    let (id, on) = match e.slot {
                        Slot::Unplaced { unplaced } => (format!("book:pool:unplaced:{unplaced}"), None),
                        Slot::Cell { page, cell } => (format!("book:pool:{page}:{cell}"), Some(page + 1)),
                    };
                    register(ui, id, r);
                    // a mark and the page number on a photo that is on a page
                    if let Some(page) = on {
                        let badge = Rect::from_min_size(r.right_bottom() - vec2(30.0, 17.0), vec2(28.0, 15.0));
                        painter.rect_filled(badge, 7.0, Color32::from_black_alpha(160));
                        painter.text(badge.center(), egui::Align2::CENTER_CENTER, format!("✓{page}"), t.font(10.5), Color32::WHITE);
                    } else {
                        painter.rect_stroke(r, 3.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
                    }
                    if resp.drag_started() {
                        bu.drag = Some(Drag::Photo(e.slot));
                    }
                    if resp.clicked()
                        && let Slot::Cell { page, .. } = e.slot
                    {
                        bu.page = page;
                        bu.reveal_page = true;
                    }
                    resp.on_hover_text(app.session.catalog.photo(PhotoId(e.photo)).map(|p| p.file_name.clone()).unwrap_or_default());
                }
            });
        }
    });
    let _ = (ctx, pos2(0.0, 0.0));
}
