//! The Book preview: the open book full screen, page by page like the printed book. The cover
//! alone, then two-page spreads, on a dark ground, each page drawn from the same layout, crops and
//! photos as the export (see [`cache`]), sized to fit the window.
//!
//! * `book.preview {book?, page?, close?}` opens it (Preview in the Book view's bar, ⌘↩); `close`
//!   closes it; with it open and no `close`, the reply says what it shows.
//! * `book.previewPage {page}` or `{to: next | previous | first | last}` turns to a spread.
//! * Keys: ← → Page Up / Down and Space turn pages (Shift+Space back), Home / End go to the cover
//!   and the last spread, a click on the right or left half of the screen turns, Esc or the close
//!   button returns to the Book view on the same page. The counter and the thumbnail strip show
//!   while the pointer moves ([`CONTROLS_SECS`]).
//! * A turn slides the old spread out and the new one in ([`SLIDE_SECS`]); with the system's
//!   reduce-motion setting on, the new spread just appears ([`motion`]).
//!
//! It fills the window the way the full-screen photo preview does (`UiState::fullscreen`, drawn in
//! place of the panels by `app/frame.rs`); the state is [`Preview`] in [`BookUi::preview`].
use egui::{Context, Sense, Ui};
use serde_json::{Value, json};

use super::flush;
use crate::LightcraftApp;
use crate::widgets::register;

mod cache;
mod draw;
mod motion;

pub use cache::Cache;
use draw::Action;

/// Seconds the controls stay after the pointer last moved.
pub const CONTROLS_SECS: f64 = 2.5;
/// Seconds a page turn takes.
pub const SLIDE_SECS: f64 = 0.28;
/// Seconds a page is drawn from thumbnails before the "Rendering…" label says so (no blink for a quick render).
const RENDERING_AFTER_SECS: f64 = 0.3;
/// Seconds the controls take to fade out.
const FADE_SECS: f64 = 0.35;

/// A page turn in progress: spread `from` slides out, spread `to` in.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Slide {
    from: usize,
    to: usize,
    started: f64,
}

/// How long the frames of the page turns took (for the frame-time check).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frames {
    /// The last frame, milliseconds.
    pub last_ms: f32,
    /// Frames drawn while a page turn slid since the preview opened, and the sum and the longest of their times.
    pub count: u32,
    pub sum_ms: f32,
    pub max_ms: f32,
    /// The longest texture upload of those frames.
    pub upload_ms: f32,
    /// The whole UI pass of the frame before each of those frames (the app's own measure: every
    /// panel, not only the preview), summed and longest.
    pub update_sum_ms: f32,
    pub update_max_ms: f32,
    /// Frames over the 16 ms budget (the app's UI pass), and the longest set-up of page renders.
    pub over_budget: u32,
    pub setup_max_ms: f32,
}

/// The open preview.
#[derive(Clone, Debug)]
pub struct Preview {
    pub book: u32,
    /// The page on show: the page asked for, or the first page of the spread turned to.
    pub page: usize,
    slide: Option<Slide>,
    pub reduce_motion: bool,
    /// The controls show until this time (egui time).
    controls_until: f64,
    /// Where the pointer was last frame (the controls appear when it is somewhere else).
    pointer: Option<egui::Pos2>,
    cache: Cache,
    pub frames: Frames,
    /// Failures already said (a failed page is said once).
    said: usize,
    /// Since when the spread on show is drawn from thumbnails (egui time), for the "Rendering…" label.
    standin_since: Option<f64>,
    /// The book as of the last frame, so a frame clones it only when it changed.
    copy: Option<lightcraft_engine::book::Book>,
}

/// Where a page turn goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Page(usize),
    Next,
    Previous,
    First,
    Last,
}

impl Turn {
    fn parse(v: &Value) -> Result<Turn, String> {
        if let Some(page) = v.get("page").filter(|p| !p.is_null()) {
            return page.as_u64().and_then(|p| usize::try_from(p).ok()).map(Turn::Page).ok_or_else(|| "`page` is a whole number from 0".to_string());
        }
        match v.get("to").and_then(Value::as_str) {
            Some("next") => Ok(Turn::Next),
            Some("previous") => Ok(Turn::Previous),
            Some("first") => Ok(Turn::First),
            Some("last") => Ok(Turn::Last),
            Some(other) => Err(format!("`to` is next, previous, first or last, not `{other}`")),
            None => Err("book.previewPage needs `page` (0 is the cover or the first page) or `to` (next, previous, first, last)".into()),
        }
    }
}

/// Is the preview on screen?
pub(crate) fn active(app: &LightcraftApp) -> bool {
    app.ui.book.preview.is_some() && app.ui.fullscreen && app.ui.view == crate::state::ViewMode::Book
}

/// Drops a preview that something else ended (the full-screen preview toggled off, another view).
pub(crate) fn sync(app: &mut LightcraftApp) {
    if app.ui.book.preview.is_some() && !active(app) {
        close(app);
    }
}

fn spreads_of(app: &LightcraftApp, id: u32) -> Option<Vec<Vec<usize>>> {
    app.session.books.find(Some(id)).ok().map(|b| b.spreads())
}

fn spread_at(spreads: &[Vec<usize>], page: usize) -> usize {
    spreads.iter().position(|s| s.contains(&page)).unwrap_or(0)
}

/// What the preview shows, for the replies.
pub fn state(app: &LightcraftApp, now: f64) -> Value {
    let Some(pv) = app.ui.book.preview.as_ref().filter(|_| active(app)) else { return json!({"open": false}) };
    let spreads = spreads_of(app, pv.book).unwrap_or_default();
    json!({
        "open": true,
        "book": pv.book,
        "page": pv.page,
        "spread": spread_at(&spreads, pv.page),
        "spreads": spreads.len(),
        "sliding": pv.slide.is_some_and(|s| now - s.started < SLIDE_SECS),
        "reduceMotion": pv.reduce_motion,
        "pagesReady": pv.cache.ready(),
        "pagesRendering": pv.cache.pending(),
        "rendersRunning": pv.cache.in_flight(),
        "frames": {"last": pv.frames.last_ms, "turnFrames": pv.frames.count,
            "turnAvg": if pv.frames.count > 0 { pv.frames.sum_ms / pv.frames.count as f32 } else { 0.0 },
            "turnMax": pv.frames.max_ms, "uploadMax": pv.frames.upload_ms,
            "updateAvg": if pv.frames.count > 0 { pv.frames.update_sum_ms / pv.frames.count as f32 } else { 0.0 },
            "updateMax": pv.frames.update_max_ms, "overBudget": pv.frames.over_budget, "setupMax": pv.frames.setup_max_ms},
    })
}

/// `book.preview`: opens the preview (of `book`, at `page`; else the book and page on show), or with
/// `close` closes it. Already open: says what it shows.
pub(crate) fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    let now = ctx.input(|i| i.time);
    Some(match id {
        "book.preview" if p.get("close").and_then(Value::as_bool) == Some(true) => {
            close(app);
            Ok(state(app, now))
        }
        "book.preview" if active(app) && p.get("book").is_none() && p.get("page").is_none() => Ok(state(app, now)),
        "book.preview" => open(app, &ctx, p).map(|_| state(app, now)),
        "book.previewPage" => Turn::parse(p).and_then(|t| go(app, &ctx, t)).map(|_| state(app, now)),
        _ => return None,
    })
}

/// Opens the preview (see [`command`]).
fn open(app: &mut LightcraftApp, ctx: &Context, p: &Value) -> Result<(), String> {
    let which = match p.get("book") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("`book` must be a book id")?),
    };
    if let Some(id) = which {
        app.run("book.open", json!({"book": id}))?;
    }
    let book = app.session.books.find(None)?;
    let (id, count) = (book.id, book.pages.len());
    if count == 0 {
        return Err("the book has no pages to preview; add photos or a page first".into());
    }
    let page = match p.get("page") {
        None | Some(Value::Null) => app.ui.book.page.min(count - 1),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n < count)
            .ok_or_else(|| format!("`page` is 0 to {} in this book", count - 1))?,
    };
    // the crop being changed is saved before the pages are drawn from the book
    let mut bu = std::mem::take(&mut app.ui.book);
    flush(app, ctx, &mut bu);
    app.ui.book = bu;
    if app.ui.view != crate::state::ViewMode::Book {
        super::command(app, "view.book", &json!({})).transpose()?;
    }
    let now = ctx.input(|i| i.time);
    app.ui.book.page = page;
    app.ui.book.preview = Some(Preview {
        book: id,
        page,
        slide: None,
        reduce_motion: motion::reduced(),
        controls_until: now + CONTROLS_SECS,
        pointer: None,
        cache: Cache::default(),
        frames: Frames::default(),
        said: 0,
        standin_since: None,
        copy: None,
    });
    app.ui.fullscreen = true;
    ctx.request_repaint();
    Ok(())
}

/// Closes the preview: back in the Book view on the page that was on show.
pub(crate) fn close(app: &mut LightcraftApp) {
    let Some(pv) = app.ui.book.preview.take() else { return };
    let count = app.session.books.find(Some(pv.book)).map_or(0, |b| b.pages.len());
    app.ui.book.page = pv.page.min(count.saturating_sub(1));
    app.ui.book.reveal_page = true;
    app.ui.fullscreen = false;
}

/// Turns to `to`; a turn to the spread already on show only moves the page.
fn go(app: &mut LightcraftApp, ctx: &Context, to: Turn) -> Result<(), String> {
    if !active(app) {
        return Err("the preview is not open; run book.preview first".into());
    }
    let id = app.ui.book.preview.as_ref().map(|p| p.book).ok_or("the preview is not open; run book.preview first")?;
    let book = app.session.books.find(Some(id)).map_err(|e| format!("the book is gone: {e}"))?;
    let spreads = book.spreads();
    let count = book.pages.len();
    let now = ctx.input(|i| i.time);
    let Some(pv) = app.ui.book.preview.as_mut() else { return Ok(()) };
    let from = spread_at(&spreads, pv.page);
    let first_of = |s: usize| spreads.get(s).and_then(|p| p.first().copied());
    let page = match to {
        Turn::Page(p) if p < count => p,
        Turn::Page(p) => return Err(format!("the book has {count} pages; `page` is 0 to {}, not {p}", count.saturating_sub(1))),
        Turn::Next => first_of(from + 1).ok_or("this is the last spread")?,
        Turn::Previous => first_of(from.checked_sub(1).ok_or("this is the cover")?).ok_or("this is the cover")?,
        Turn::First => first_of(0).ok_or("the book has no pages")?,
        Turn::Last => first_of(spreads.len().saturating_sub(1)).ok_or("the book has no pages")?,
    };
    let target = spread_at(&spreads, page);
    pv.page = page;
    if target != from {
        // (a turn begun while another slides ends that one at once)
        pv.slide = (!pv.reduce_motion).then_some(Slide { from, to: target, started: now });
    }
    pv.controls_until = pv.controls_until.max(now + CONTROLS_SECS / 2.0);
    ctx.request_repaint();
    Ok(())
}

/// The preview's keys, before the general shortcuts. While it is open every key is its own: `true`
/// (the native menu keeps ⌘Q, which does not pass through here).
pub(crate) fn handle_key(app: &mut LightcraftApp, ctx: &Context) -> bool {
    if !active(app) || app.ui.dialog.is_some() || app.ui.book.prompt.is_some() {
        return false;
    }
    let (esc, toggle) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.modifiers.command && i.key_pressed(egui::Key::Enter)));
    if esc || toggle {
        close(app);
        return true;
    }
    let key = |k: egui::Key| ctx.input(|i| i.modifiers.is_none() && i.key_pressed(k));
    let to = if key(egui::Key::ArrowRight) || key(egui::Key::PageDown) || key(egui::Key::Space) {
        Turn::Next
    } else if key(egui::Key::ArrowLeft) || key(egui::Key::PageUp) || ctx.input(|i| i.modifiers.shift_only() && i.key_pressed(egui::Key::Space)) {
        Turn::Previous
    } else if key(egui::Key::Home) {
        Turn::First
    } else if key(egui::Key::End) {
        Turn::Last
    } else {
        // every other key is swallowed: the library's shortcuts (ratings, flags, delete) would
        // reach the selected photo under the preview
        return true;
    };
    // (at the first or the last spread the key does nothing)
    let _ = go(app, ctx, to);
    true
}

/// The preview, in place of the window's panels.
pub fn show(app: &mut LightcraftApp, ui: &mut Ui) {
    let started = std::time::Instant::now();
    let ctx = ui.ctx().clone();
    let area = ui.max_rect();
    ui.painter().rect_filled(area, 0.0, draw::GROUND);
    register(ui, "book:preview", area);
    let Some(mut pv) = app.ui.book.preview.take() else { return };
    match app.session.books.find(Some(pv.book)) {
        Ok(live) => {
            if pv.copy.as_ref() != Some(live) {
                pv.copy = Some(live.clone());
            }
        }
        Err(_) => pv.copy = None,
    }
    let Some(book) = pv.copy.take() else {
        // the book was deleted behind the preview
        app.ui.book.preview = Some(pv);
        close(app);
        return;
    };
    let now = ctx.input(|i| i.time);
    let spreads = book.spreads();
    pv.page = pv.page.min(book.pages.len().saturating_sub(1));
    let current = spread_at(&spreads, pv.page);
    let Ok(page) = book.settings.page_size() else {
        pv.copy = Some(book);
        app.ui.book.preview = Some(pv);
        return;
    };
    let size = draw::page_size(area, page);
    let edge = draw::edge_of(&ctx, size);
    // the pages to have: the spread on show, the one sliding out, then the next and the one before
    let mut wanted: Vec<usize> = spreads.get(current).cloned().unwrap_or_default();
    let on_show = wanted.len();
    let sliding_from = pv.slide.map(|s| s.from);
    for s in [sliding_from.unwrap_or(usize::MAX), current + 1, current.wrapping_sub(1)] {
        for p in spreads.get(s).into_iter().flatten() {
            if !wanted.contains(p) {
                wanted.push(*p);
            }
        }
    }
    // (their thumbnails too: the stand-in of a page that isn't rendered yet)
    for &p in wanted.iter().skip(on_show) {
        for photo in book.pages.get(p).into_iter().flat_map(|pg| pg.slots.iter().flatten()) {
            crate::panels::grid::request_thumb(app, lightcraft_catalog::PhotoId(*photo), 512, 10);
        }
    }
    let (upload, setup) = pv.cache.update(app, &ctx, &book, &wanted, edge);
    for e in std::mem::take(&mut pv.cache.failures) {
        pv.said += 1;
        if pv.said == 1 {
            app.toast(&ctx, format!("Preview: {e}"));
        }
    }
    // pointer: moving it shows the controls
    let at = ctx.input(|i| i.pointer.latest_pos());
    let moved = at != pv.pointer || ctx.input(|i| i.pointer.any_pressed());
    pv.pointer = at;
    if moved {
        pv.controls_until = now + CONTROLS_SECS;
    }
    // the slide
    let slide = pv.slide.filter(|s| now - s.started < SLIDE_SECS);
    if pv.slide.is_some() && slide.is_none() {
        pv.slide = None;
    }
    let painter = ui.painter_at(area);
    let mut standin = false;
    match slide {
        Some(s) => {
            let t = ((now - s.started) / SLIDE_SECS).clamp(0.0, 1.0);
            let ease = (t * t * (3.0 - 2.0 * t)) as f32;
            let dir = if s.to > s.from { 1.0 } else { -1.0 };
            let travel = area.width();
            for (index, dx) in [(s.from, -dir * ease * travel), (s.to, dir * (1.0 - ease) * travel)] {
                let pages = spreads.get(index).cloned().unwrap_or_default();
                let rects = draw::spread_rects(area, size, pages.len(), dx);
                let late = draw::spread(app, &painter, &pv.cache, &book, &pages, &rects, edge);
                standin = late && index == s.to;
            }
            ctx.request_repaint();
        }
        None => {
            let pages = spreads.get(current).cloned().unwrap_or_default();
            if pages.is_empty() {
                painter.text(
                    area.center(),
                    egui::Align2::CENTER_CENTER,
                    crate::i18n::tr("No pages"),
                    crate::theme::Tokens::get(&ctx).font(14.0),
                    egui::Color32::GRAY,
                );
            }
            let rects = draw::spread_rects(area, size, pages.len(), 0.0);
            standin = draw::spread(app, &painter, &pv.cache, &book, &pages, &rects, edge);
        }
    }
    // the pixels of the spread on show aren't final: say so, as the loupe does (no activity row)
    if standin {
        let since = *pv.standin_since.get_or_insert(now);
        if now - since >= RENDERING_AFTER_SECS {
            if app.ui.settings.loupe_messages {
                let over = draw::spread_rects(area, size, spreads.get(current).map_or(0, Vec::len), 0.0)
                    .into_iter()
                    .reduce(|a, b| a.union(b))
                    .unwrap_or(area);
                crate::panels::detail::loupe_pill(ui, area, over, None, "book:previewRendering", crate::i18n::tr("Rendering…"), false);
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(RENDERING_AFTER_SECS - (now - since) + 0.01));
        }
    } else {
        pv.standin_since = None;
    }
    // a click on the left or right half turns; drawn first so the controls on top win
    let bg = ui.interact(area, egui::Id::unique("book-preview-area"), Sense::click());
    let mut action = bg.clicked().then(|| bg.interact_pointer_pos().map(|p| Action::Turn(draw::half(area, p)))).flatten();
    // the controls, while the pointer moved lately (or is held down)
    let left = pv.controls_until - now;
    let held = ui.input(|i| i.pointer.any_down());
    if left > 0.0 || held {
        let alpha = if held { 1.0 } else { (left / FADE_SECS).clamp(0.0, 1.0) as f32 };
        action = draw::controls(app, ui, &pv, &book, &spreads, area, alpha).or(action);
        if left < FADE_SECS {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left - FADE_SECS + 0.01));
        }
    }
    pv.copy = Some(book);
    app.ui.book.preview = Some(pv);
    match action {
        Some(Action::Close) => close(app),
        Some(Action::Page(p)) => {
            let _ = go(app, &ctx, Turn::Page(p));
        }
        Some(Action::Turn(d)) => {
            let _ = go(app, &ctx, if d < 0 { Turn::Previous } else { Turn::Next });
        }
        None => {}
    }
    let update = app.perf.update_ms as f32;
    if let Some(pv) = app.ui.book.preview.as_mut() {
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        pv.frames.last_ms = ms;
        pv.frames.upload_ms = pv.frames.upload_ms.max(upload);
        pv.frames.setup_max_ms = pv.frames.setup_max_ms.max(setup);
        if slide.is_some() {
            pv.frames.count += 1;
            pv.frames.sum_ms += ms + upload;
            pv.frames.max_ms = pv.frames.max_ms.max(ms + upload);
            pv.frames.update_sum_ms += update;
            pv.frames.update_max_ms = pv.frames.update_max_ms.max(update);
            pv.frames.over_budget += u32::from(update > 16.0);
        }
    }
}
