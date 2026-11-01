//! The Book view (File ▸ Book…, or a book in the side bar): a large spread in the middle and the
//! page strip below it in the main window like the other views, and the book's photos in the right
//! panel (`panels/right.rs` hands it to [`column`] while this view is on).
//!
//! Every change goes through a `book.*` command ([`lightcraft_engine::cmd`]) and is one undo step;
//! this module draws the session's books ([`lightcraft_engine::Session::books`]) and turns input into
//! commands. Widgets register under `book:*`, `button:book*` ids for the control channel and the
//! headless tests.
//!
//! * [`spread`]: the open spread: drag a photo from the pool or another frame onto a frame, drag
//!   inside a frame to move the photo, scroll or pinch to zoom it (the crop is saved per cell).
//! * [`strip`]: every page as a thumbnail, only the ones on screen drawn: click, drag to reorder, +.
//! * [`pool`]: the right panel: the book's photos, searchable, unplaced first; and the book's settings.
//! * [`menu`]: Add to Book (the photo menu, a drag onto a book in the side bar) and its prompts.
//! * [`thumbs`]: pages and photos from the grid's thumbnails.
//!
//! The side bar's Books section is `panels/left/books.rs`.
use egui::{Rect, Ui, vec2};
use lightcraft_engine::book::model::{Book, Crop, Slot};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

pub mod menu;
mod pool;
mod settings;
mod spread;
mod strip;
mod thumbs;

pub use menu::{Prompt, add_photos_to_book, show_prompt};

/// Height of the page strip.
const STRIP_H: f32 = 112.0;
/// Height of the bar above the spread.
const BAR_H: f32 = 40.0;
/// Seconds a zoom waits for the next scroll step before it is saved as one undo step.
const ZOOM_SETTLE: f64 = 0.45;

/// What the right column shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Photos,
    Settings,
}

/// A photo or page being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Drag {
    /// A photo taken from the pool or a frame, to be dropped on a frame (or the pool).
    Photo(Slot),
    /// A press inside a frame: it moves the photo within the frame while the pointer stays there,
    /// and becomes a photo drag (`swap`) once it leaves.
    Cell { page: usize, cell: usize, swap: bool },
    /// A page of the strip, to be placed elsewhere.
    Page(usize),
}

/// A place a dragged photo can land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Hit {
    Cell {
        page: usize,
        cell: usize,
    },
    /// The pool: a photo dropped there comes off its page.
    Pool,
}

/// A crop being changed by hand and not saved yet (a drag, a run of scroll steps).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pending {
    pub book: u32,
    pub page: usize,
    pub cell: usize,
    pub crop: Crop,
    /// The last change (egui time).
    pub at: f64,
}

/// What the Book view remembers between frames (not saved).
#[derive(Clone, Debug, Default)]
pub struct BookUi {
    /// The page on show (selected), zero-based.
    pub page: usize,
    pub tab: Tab,
    /// The pool's search.
    pub search: String,
    /// Auto Layout's template choice (an index into [`settings::LAYOUTS`]).
    pub layout: usize,
    pub(crate) drag: Option<Drag>,
    pub(crate) pending: Option<Pending>,
    /// The book being renamed in the side bar, and the text so far.
    pub rename: Option<(u32, String)>,
    /// A question to answer before something is done.
    pub prompt: Option<Prompt>,
    /// Where a dragged photo may land in the spread, as drawn this frame.
    pub(crate) hits: Vec<(Hit, Rect)>,
    /// The pool's panel as drawn this frame (it is drawn before the spread): a photo dropped there
    /// comes off its page.
    pub(crate) pool: Option<Rect>,
    /// The strip scrolls to the page on show on the next frame.
    pub(crate) reveal_page: bool,
    /// Milliseconds the last frame of the view took to draw (for the frame-time check).
    pub frame_ms: f32,
}

/// Runs a `book.*` command, saving a crop in progress first; a refusal is a toast.
pub(crate) fn run(app: &mut LightcraftApp, ctx: &egui::Context, bu: &mut BookUi, id: &str, params: Value) -> Option<Value> {
    if id != "book.setCellCrop" {
        flush(app, ctx, bu);
    }
    match app.run(id, params) {
        Ok(v) => Some(v),
        Err(e) => {
            app.toast(ctx, e);
            None
        }
    }
}

/// Saves the crop in progress as one undo step (nothing when its book or cell is gone).
pub(crate) fn flush(app: &mut LightcraftApp, ctx: &egui::Context, bu: &mut BookUi) {
    let Some(p) = bu.pending.take() else { return };
    let holds = app
        .session
        .books
        .find(Some(p.book))
        .ok()
        .and_then(|b| b.pages.get(p.page))
        .is_some_and(|pg| pg.slots.get(p.cell).copied().flatten().is_some());
    if !holds {
        return;
    }
    // a photo at the frame's size and centred is the book's own setting, not a crop: a frame that shows
    // the whole photo stays so (zoomed out again, or nothing moved)
    if p.crop.zoom <= 1.0 && (p.crop.x - 0.5).abs() < 1e-3 && (p.crop.y - 0.5).abs() < 1e-3 {
        let stored = app.session.books.find(Some(p.book)).ok().and_then(|b| b.pages.get(p.page)).and_then(|pg| pg.crop(p.cell)).is_some();
        if stored && let Err(e) = app.run("book.setCellCrop", json!({"book": p.book, "page": p.page, "cell": p.cell, "reset": true})) {
            app.toast(ctx, e);
        }
        return;
    }
    if let Err(e) =
        app.run("book.setCellCrop", json!({"book": p.book, "page": p.page, "cell": p.cell, "zoom": p.crop.zoom, "x": p.crop.x, "y": p.crop.y}))
    {
        app.toast(ctx, e);
    }
}

/// The view, in the main window's centre panel.
pub fn show(app: &mut LightcraftApp, ui: &mut Ui) {
    let started = std::time::Instant::now();
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let mut bu = std::mem::take(&mut app.ui.book);
    bu.hits.clear();
    let book = app.session.books.find(None).ok().cloned();
    match &book {
        Some(b) => {
            bu.page = bu.page.min(b.pages.len().saturating_sub(1));
            layout(app, ui, &t, &mut bu, b);
            spread::resolve_drag(app, &ctx, &mut bu, b);
        }
        None => empty(app, ui, &t, &mut bu),
    }
    // a zoom that has not been touched for a moment is saved
    if let Some(p) = bu.pending {
        let now = ctx.input(|i| i.time);
        if now - p.at >= ZOOM_SETTLE && !ctx.input(|i| i.pointer.any_down()) {
            flush(app, &ctx, &mut bu);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    bu.pool = None;
    bu.frame_ms = started.elapsed().as_secs_f32() * 1000.0;
    app.ui.book = bu;
}

/// The right panel while the Book view is on: the book's photos and settings.
pub fn column(app: &mut LightcraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let mut bu = std::mem::take(&mut app.ui.book);
    if let Ok(book) = app.session.books.find(None).cloned() {
        let room = ui.max_rect();
        pool::show(app, ui, &t, &mut bu, &book, room);
        bu.pool = Some(room);
    } else {
        ui.painter().rect_filled(ui.max_rect(), 0.0, t.chrome);
    }
    app.ui.book = bu;
}

fn empty(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi) {
    let ctx = ui.ctx().clone();
    let selected = app.session.selected().len();
    ui.add_space(ui.available_height() / 3.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("No Book Yet")).font(t.semibold(16.0)).color(t.text));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Make a book from the selected photos, or start an empty one.")).color(t.text_dim));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() / 2.0 - 150.0).max(0.0));
            ui.add_enabled_ui(selected > 0, |ui| {
                if text_button(ui, "bookNewSelected", "New from Selected", false).clicked() {
                    run(app, &ctx, bu, "book.create", json!({"source": "selection"}));
                }
            });
            if text_button(ui, "bookNewEmpty", "New Empty Book", false).clicked() {
                run(app, &ctx, bu, "book.create", json!({"empty": true}));
            }
        });
    });
}

/// The bar, the spread, the strip and the right column.
fn layout(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let area = ui.max_rect();
    let main = area;
    let bar = Rect::from_min_size(main.min, vec2(main.width(), BAR_H));
    let strip = Rect::from_min_max(egui::pos2(main.left(), (main.bottom() - STRIP_H).max(bar.bottom())), main.max);
    let center = Rect::from_min_max(egui::pos2(main.left(), bar.bottom()), egui::pos2(main.right(), strip.top()));
    register(ui, "book:view", area);
    let child = |ui: &mut Ui, r: Rect, layout: egui::Layout| ui.new_child(egui::UiBuilder::new().max_rect(r).layout(layout));
    bar_ui(app, &mut child(ui, bar, egui::Layout::left_to_right(egui::Align::Center)), t, bu, book);
    spread::show(app, &mut child(ui, center, egui::Layout::top_down(egui::Align::Min)), t, bu, book, center);
    strip::show(app, &mut child(ui, strip, egui::Layout::top_down(egui::Align::Min)), t, bu, book, strip);
}

/// Above the spread: the page counter with its arrows, and what can be done to the page on show.
fn bar_ui(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, bu: &mut BookUi, book: &Book) {
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.x = 5.0;
    ui.add_space(12.0);
    ui.label(egui::RichText::new(&book.name).font(t.semibold(13.5)).color(t.text));
    ui.add_space(10.0);
    if text_button(ui, "bookPagePrev", "◀", false).clicked() && bu.page > 0 {
        flush(app, &ctx, bu);
        bu.page -= 1;
        bu.reveal_page = true;
    }
    let tag = match book.pages.len() {
        0 => crate::i18n::tr("No pages").to_string(),
        n if bu.page == 0 && book.cover => crate::i18n::tr_format!("Cover · {n} pages", n = n),
        n => crate::i18n::tr_format!("Page {i} of {n}", i = bu.page + 1, n = n),
    };
    let label = ui.label(egui::RichText::new(tag).color(t.text));
    register(ui, "book:pageLabel", label.rect);
    if text_button(ui, "bookPageNext", "▶", false).clicked() && bu.page + 1 < book.pages.len() {
        flush(app, &ctx, bu);
        bu.page += 1;
        bu.reveal_page = true;
    }
    ui.add_space(14.0);
    let has_page = !book.pages.is_empty();
    ui.add_enabled_ui(has_page, |ui| page_buttons(app, ui, bu, book));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(8.0);
        if text_button(ui, "bookExportJpeg", "Export JPEGs…", false).clicked() {
            run(app, &ctx, bu, "app.bookJpeg", json!({}));
        }
        if text_button(ui, "bookExportPdf", "Export PDF…", false).clicked() {
            run(app, &ctx, bu, "app.bookPdf", json!({}));
        }
    });
}

/// Lock, duplicate and delete the page on show, and its template.
fn page_buttons(app: &mut LightcraftApp, ui: &mut Ui, bu: &mut BookUi, book: &Book) {
    let ctx = ui.ctx().clone();
    let page = bu.page;
    let Some(p) = book.pages.get(page) else { return };
    let (c, chosen) = crate::widgets::dropdown_search::pick_with(
        ui,
        "bookTemplate",
        |c| c.selected_text(crate::i18n::tr(settings::template_label(p.template))),
        &lightcraft_engine::book::model::Template::ALL,
        |tpl| *tpl == p.template,
        |tpl| crate::i18n::tr(settings::template_label(*tpl)).to_string(),
        |ui, tpl, row| register(ui, format!("menu:bookTemplate-{}", tpl.name()), row.rect),
    );
    crate::widgets::name_combo(&c, crate::i18n::tr("Template"));
    register(ui, "combo:bookTemplate", c.rect);
    if let Some(tpl) = chosen.and_then(|i| lightcraft_engine::book::model::Template::ALL.get(i)).filter(|tpl| **tpl != p.template) {
        run(app, &ctx, bu, "book.setTemplate", json!({"page": page, "template": tpl.name()}));
    }
    if text_button(ui, "bookLockPage", if p.locked { "Unlock Page" } else { "Lock Page" }, p.locked).clicked() {
        run(app, &ctx, bu, "book.lockPage", json!({"page": page, "locked": !p.locked}));
    }
    if text_button(ui, "bookDuplicatePage", "Duplicate Page", false).clicked() {
        run(app, &ctx, bu, "book.duplicatePage", json!({"page": page}));
        bu.page += 1;
        bu.reveal_page = true;
    }
    if text_button(ui, "bookDeletePage", "Delete Page", false).clicked() {
        run(app, &ctx, bu, "book.deletePage", json!({"page": page}));
    }
}

/// ← → step through the pages, Page Up and Down through the spreads, while the Book view is on.
/// Called before the general shortcuts, which would move the photo selection. `true`: handled.
pub(crate) fn handle_key(app: &mut LightcraftApp, ctx: &egui::Context) -> bool {
    if app.ui.view != crate::state::ViewMode::Book || app.ui.dialog.is_some() || ctx.text_edit_focused() || app.ui.book.prompt.is_some() {
        return false;
    }
    let Ok(book) = app.session.books.find(None) else { return false };
    let spreads = book.spreads();
    let count = book.pages.len();
    let key = |k: egui::Key| ctx.input(|i| i.modifiers.is_none() && i.key_pressed(k));
    let at = spreads.iter().position(|s| s.contains(&app.ui.book.page)).unwrap_or(0);
    let to = if key(egui::Key::ArrowLeft) {
        app.ui.book.page.checked_sub(1)
    } else if key(egui::Key::ArrowRight) {
        Some(app.ui.book.page + 1).filter(|p| *p < count)
    } else if key(egui::Key::PageUp) {
        at.checked_sub(1).and_then(|s| spreads.get(s)).and_then(|s| s.first().copied())
    } else if key(egui::Key::PageDown) {
        spreads.get(at + 1).and_then(|s| s.first().copied())
    } else {
        return false;
    };
    if let Some(page) = to {
        let mut bu = std::mem::take(&mut app.ui.book);
        flush(app, ctx, &mut bu);
        bu.page = page;
        bu.reveal_page = true;
        app.ui.book = bu;
    }
    true
}

/// The Book commands that only the window can do: `view.book {book?}` opens the view (`dialog.book`
/// is the same, for File ▸ Book…); `None` for any other id. See [`crate::export_task::book_task`].
pub(crate) fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if !matches!(id, "view.book" | "dialog.book") {
        return None;
    }
    if let Some(book) = p.get("book").filter(|v| !v.is_null()) {
        let r = app.run("book.open", json!({"book": book}));
        if let Err(e) = r {
            return Some(Err(e));
        }
    }
    app.ui.view = crate::state::ViewMode::Book;
    app.ui.book.reveal_page = true;
    // the book's photos are in the right panel
    if app.ui.right == crate::state::RightPanel::None {
        app.ui.right = crate::state::RightPanel::Edit;
    }
    Some(Ok(json!({"books": app.session.books.books.len(), "current": app.session.books.current})))
}

/// Leaving the view for a library source or folder: the grid shows what was picked.
pub(crate) fn after_command(app: &mut LightcraftApp, id: &str) {
    if app.ui.view == crate::state::ViewMode::Book && matches!(id, "library.source" | "library.browse") {
        app.ui.view = crate::state::ViewMode::PhotoGrid;
    }
}
