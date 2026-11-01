//! Photos into a book: the photo menu's Add to Book, a drag onto a book in the side bar, and the
//! questions on the way: the photos the book has already (add them again or skip them), a name for a
//! new book, and deleting a book. The questions are small windows drawn over the app by
//! [`show_prompt`], one at a time.
use egui::{Align2, Context, Ui};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::button::Button;

/// A question waiting for an answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    /// Some of `ids` are in the book already: add them again, or leave them out.
    Duplicates { book: u32, ids: Vec<u64>, held: usize },
    /// A name for a new book made from `ids`.
    NewBook { ids: Vec<u64>, name: String },
    /// Delete a book (its photos stay in the library).
    DeleteBook { book: u32 },
}

fn book_name(app: &LightcraftApp, id: u32) -> String {
    app.session.books.find(Some(id)).map(|b| b.name.clone()).unwrap_or_default()
}

/// Adds `ids` to `book`: at once when it holds none of them, else a question asks first.
pub fn add_photos_to_book(app: &mut LightcraftApp, ctx: &Context, book: u32, ids: Vec<u64>) {
    if ids.is_empty() {
        return;
    }
    let Ok(b) = app.session.books.find(Some(book)) else { return };
    let held = b.duplicates_of(&ids).len();
    if held == 0 {
        add(app, ctx, book, &ids, None);
    } else {
        app.ui.book.prompt = Some(Prompt::Duplicates { book, ids, held });
    }
}

/// `book.addPhotos`, then a toast saying how many went in.
fn add(app: &mut LightcraftApp, ctx: &Context, book: u32, ids: &[u64], duplicates: Option<&str>) {
    let mut params = json!({"book": book, "ids": ids});
    if let Some(d) = duplicates {
        params["duplicates"] = json!(d);
    }
    match app.run("book.addPhotos", params) {
        Ok(r) => {
            let (n, skipped) = (r["added"].as_u64().unwrap_or(0), r["skipped"].as_u64().unwrap_or(0));
            let name = book_name(app, book);
            let mut text = crate::i18n::tr_format!("Added {n} photo{} to “{}”", if n == 1 { "" } else { "s" }, name, n = n);
            if skipped > 0 {
                text.push_str(&crate::i18n::tr_format!(" · {k} already in the book", k = skipped));
            }
            app.toast(ctx, text);
        }
        Err(e) => app.toast(ctx, e),
    }
}

/// Add to Book in the photo menu: a submenu of the books and New Book….
pub fn add_to_book(app: &mut LightcraftApp, ui: &mut Ui, text: &str) {
    let ids: Vec<u64> = app.session.selected().iter().map(|p| p.0).collect();
    let books: Vec<(u32, String)> = app.session.books.books.iter().map(|b| (b.id, b.name.clone())).collect();
    let ctx = ui.ctx().clone();
    crate::widgets::menu_button(ui, text, |ui| {
        for (id, name) in &books {
            let r = ui.button(name);
            crate::widgets::register(ui, format!("menu:addToBook-{id}"), r.rect);
            if r.clicked() {
                add_photos_to_book(app, &ctx, *id, ids.clone());
                ui.close();
            }
        }
        if !books.is_empty() {
            ui.separator();
        }
        let r = ui.button(crate::i18n::tr("New Book…"));
        crate::widgets::register(ui, "menu:addToBook-new", r.rect);
        if r.clicked() {
            app.ui.book.prompt = Some(Prompt::NewBook { ids: ids.clone(), name: String::new() });
            ui.close();
        }
    });
}

/// Draws the question that is waiting, if one is; its buttons answer it.
pub fn show_prompt(app: &mut LightcraftApp, ctx: &Context) {
    let Some(prompt) = app.ui.book.prompt.clone() else { return };
    let (title, body): (&str, String) = match &prompt {
        Prompt::Duplicates { book, held, .. } => (
            "Photos Already in the Book",
            if *held == 1 {
                crate::i18n::tr_format!("1 of these photos is already in “{name}”.", name = book_name(app, *book))
            } else {
                crate::i18n::tr_format!("{n} of these photos are already in “{name}”.", n = held, name = book_name(app, *book))
            },
        ),
        Prompt::NewBook { ids, .. } => {
            ("New Book", crate::i18n::tr_format!("A book with {n} photo{}. Name it:", if ids.len() == 1 { "" } else { "s" }, n = ids.len()))
        }
        Prompt::DeleteBook { book } => (
            "Delete Book",
            crate::i18n::tr_format!("Delete “{name}”? Its photos stay in the library. Undo brings the book back.", name = book_name(app, *book)),
        ),
    };
    let mut answer: Option<&str> = None;
    let mut name = if let Prompt::NewBook { name, .. } = &prompt { name.clone() } else { String::new() };
    egui::Window::new(crate::i18n::tr(title))
        .id(egui::Id::unique("book-prompt"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            crate::widgets::register(ui, "book:prompt", ui.max_rect());
            ui.set_min_width(320.0);
            ui.label(body);
            if matches!(prompt, Prompt::NewBook { .. }) {
                let r =
                    crate::text_field::TextField::singleline("field:bookNewName", &mut name).hint(crate::i18n::tr("Book name")).width(300.0).show(ui);
                if !ctx.memory(|m| m.has_focus(crate::text_field::id("field:bookNewName"))) && !r.editing {
                    ctx.memory_mut(|m| m.request_focus(crate::text_field::id("field:bookNewName")));
                }
                if r.ending == Some(crate::text_field::Ending::Return) {
                    answer = Some("ok");
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (yes, extra): (&str, Option<&str>) = match prompt {
                    Prompt::Duplicates { .. } => ("Add Duplicates", Some("Skip Duplicates")),
                    Prompt::NewBook { .. } => ("Create", None),
                    Prompt::DeleteBook { .. } => ("Delete", None),
                };
                if Button::new(crate::i18n::tr(yes)).widget("button:bookPromptYes").min_width(96.0).show(ui).clicked() {
                    answer = Some("ok");
                }
                if let Some(extra) = extra
                    && Button::new(crate::i18n::tr(extra)).widget("button:bookPromptSkip").min_width(96.0).show(ui).clicked()
                {
                    answer = Some("skip");
                }
                if Button::new(crate::i18n::tr("Cancel")).widget("button:bookPromptCancel").min_width(72.0).show(ui).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    answer = Some("cancel");
                }
            });
        });
    if let Some(Prompt::NewBook { name: n, .. }) = app.ui.book.prompt.as_mut() {
        n.clone_from(&name);
    }
    let Some(answer) = answer else { return };
    app.ui.book.prompt = None;
    if answer == "cancel" {
        return;
    }
    match prompt {
        Prompt::Duplicates { book, ids, .. } => add(app, ctx, book, &ids, Some(if answer == "skip" { "skip" } else { "add" })),
        Prompt::NewBook { ids, .. } => {
            let mut params: Value = json!({"ids": ids});
            if !name.trim().is_empty() {
                params["name"] = json!(name);
            }
            match app.run("book.create", params) {
                Ok(_) => {
                    let _ = app.run("view.book", json!({}));
                }
                Err(e) => app.toast(ctx, e),
            }
        }
        Prompt::DeleteBook { book } => {
            if let Err(e) = app.run("book.delete", json!({"book": book})) {
                app.toast(ctx, e);
            }
        }
    }
}
