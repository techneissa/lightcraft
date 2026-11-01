//! The Books section of the side bar: one row per book, like albums. Click a row to open the book in
//! the Book view; double-click, or Rename… in its menu, to rename it in the row (Return or a click
//! away saves, Esc cancels, an empty or taken name is refused); Duplicate and Delete… (which asks)
//! are in the menu too. Photos dragged from the grid onto a row are added to the book.
use super::albums::pointer_over_row;
use super::rows::{ROW_HEIGHT, Row};
use super::{Icon, LightcraftApp, Sense, Tokens, json, pos2, register, vec2};
use crate::panels::book::Prompt;

/// The Books block below the sections: a header with a + that makes an empty book, then a row per book.
pub(super) fn books_block(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(14.0);
    let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    register(ui, "bookHeader", header);
    ui.painter().text(
        pos2(header.left() + 18.0, header.center().y),
        egui::Align2::LEFT_CENTER,
        crate::i18n::tr("Books"),
        t.semibold(12.5),
        t.text_dim,
    );
    let edge = super::sections::visible_right(ui).min(header.right());
    let mut hdr = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(pos2(edge - 50.0, header.top()), pos2(edge, header.bottom())))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    if crate::widgets::icon_button(&mut hdr, "bookNew", Icon::Plus, vec2(26.0, 26.0), false, true, "Create Book").clicked() {
        let ctx = ui.ctx().clone();
        new_book(app, &ctx);
    }
    if app.session.books.books.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        register(ui, "bookEmpty", r);
        ui.painter().text(pos2(r.left() + 18.0, r.center().y), egui::Align2::LEFT_CENTER, crate::i18n::tr("No books yet"), t.font(13.0), t.text_dim);
    }
    books_rows(app, ui);
}

/// The rows of the books.
fn books_rows(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let books: Vec<(u32, String, usize)> = app.session.books.books.iter().map(|b| (b.id, b.name.clone(), b.photos().len())).collect();
    for (id, name, n) in books {
        if app.ui.book.rename.as_ref().is_some_and(|(r, _)| *r == id) {
            rename_row(app, ui, id, &name);
            continue;
        }
        let selected = app.ui.view == crate::state::ViewMode::Book && app.session.books.current == Some(id);
        let resp = Row::new(&format!("book:{id}"), Icon::Album, &name).count(Some(n)).selected(selected).sense(Sense::click()).show(app, ui);
        if resp.clicked() {
            let _ = app.run("view.book", json!({"book": id}));
        }
        if resp.double_clicked() {
            app.ui.book.rename = Some((id, name.clone()));
        }
        drop_target(app, ui, &resp, id);
        crate::widgets::context_menu(&resp, |ui| {
            if ui.button(crate::i18n::tr("Open Book")).clicked() {
                let _ = app.run("view.book", json!({"book": id}));
                ui.close();
            }
            if ui.button(crate::i18n::tr("Rename…")).clicked() {
                app.ui.book.rename = Some((id, name.clone()));
                ui.close();
            }
            if ui.button(crate::i18n::tr("Duplicate")).clicked() {
                if let Err(e) = app.run("book.duplicate", json!({"book": id})) {
                    app.toast(&ctx, e);
                }
                ui.close();
            }
            if ui.button(crate::i18n::tr("Delete…")).clicked() {
                app.ui.book.prompt = Some(Prompt::DeleteBook { book: id });
                ui.close();
            }
        });
    }
}

/// The row of the book being renamed: a text field in place of its name.
fn rename_row(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u32, name: &str) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    register(ui, format!("source:book:{id}"), r);
    crate::icons::paint(ui.painter(), egui::Rect::from_min_size(pos2(r.left() + 18.0, r.center().y - 8.0), vec2(16.0, 16.0)), Icon::Album, t.icon);
    let mut text = app.ui.book.rename.as_ref().map(|(_, t)| t.clone()).unwrap_or_else(|| name.to_string());
    let field = egui::Rect::from_min_max(pos2(r.left() + 42.0, r.top() + 2.0), pos2(r.right() - 10.0, r.bottom() - 2.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let resp = crate::text_field::TextField::singleline("field:bookRename", &mut text).width(field.width()).show(&mut child);
    if !resp.editing {
        ctx.memory_mut(|m| m.request_focus(crate::text_field::id("field:bookRename")));
    }
    if let Some((_, t)) = app.ui.book.rename.as_mut() {
        t.clone_from(&text);
    }
    if resp.cancelled() {
        app.ui.book.rename = None;
    } else if resp.committed() {
        app.ui.book.rename = None;
        if text.trim() != name
            && let Err(e) = app.run("book.rename", json!({"book": id, "name": text}))
        {
            app.toast(&ctx, e);
        }
    }
}

/// Photos dragged over a book's row: outlined, and added when released.
fn drop_target(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, book: u32) {
    let Some(ids) = app.ui.dragging_photos.clone() else { return };
    if !pointer_over_row(ui, resp.rect) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_stroke(resp.rect.shrink2(vec2(8.0, 1.0)), 4.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    if ui.input(|i| i.pointer.primary_released()) {
        app.ui.dragging_photos = None;
        crate::panels::book::add_photos_to_book(app, ui.ctx(), book, ids);
    }
}

/// The header's +: an empty book, named in its row.
fn new_book(app: &mut LightcraftApp, ctx: &egui::Context) {
    match app.run("book.create", json!({"empty": true})) {
        Ok(b) => {
            if let Some(id) = b["id"].as_u64().and_then(|i| u32::try_from(i).ok()) {
                let name = b["name"].as_str().unwrap_or_default().to_string();
                app.ui.book.rename = Some((id, name));
            }
            let _ = app.run("view.book", json!({}));
        }
        Err(e) => app.toast(ctx, e),
    }
}
