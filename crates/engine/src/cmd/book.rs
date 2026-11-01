//! `book.*`: the Book module as commands.
//!
//! Books live in [`Session::books`] and are saved to `books.json` in the library folder after each
//! change. Nothing here touches the catalog, so none of it is journaled or undoable. Page and cell
//! numbers are zero-based; `book` is a book id (the current book when left out) and photos are
//! `ids` (a call that names none uses the selection, else the photos in view).
//!
//! * `book.list`, `book.get`, `book.create`, `book.open`, `book.delete`
//! * `book.set`, `book.layout`, `book.setTemplate`, `book.movePhoto`
//! * `book.exportPdf`, `book.exportJpeg`
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, index_req, names_photos, str_param};
use crate::activity::Cancel;
use crate::book::edit::Preset;
use crate::book::geometry::cells_for;
use crate::book::model::{Book, BookSettings, Slot, Template, clean_name, size_names, size_of};
use crate::book::{MAX_PHOTOS, render};
use crate::{Result, Session};

/// Rejects a parameter the command does not know, so a typo is never silently ignored.
fn only(p: &Value, known: &[&str], id: &str) -> Result<()> {
    let Some(o) = p.as_object() else { return if p.is_null() { Ok(()) } else { Err(bad(id, "params must be an object")) } };
    match o.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => Err(bad(id, format!("unknown parameter `{k}` (one of {})", known.join(", ")))),
        None => Ok(()),
    }
}

/// The `book` parameter: a book id, or `None` for the current book.
fn book_id(p: &Value, id: &str) -> Result<Option<u32>> {
    match p.get("book") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().and_then(|n| u32::try_from(n).ok()).map(Some).ok_or_else(|| bad(id, "`book` must be a book id (see book.list)")),
    }
}

/// The photos a book is made of: `ids`, else `source` (`selection` or `view`), else the selection,
/// else the photos in view (the current album, folder or collection).
fn photos(s: &mut Session, p: &Value, id: &str) -> Result<Vec<u64>> {
    let ids = match str_param(p, "source") {
        _ if names_photos(p) => s.targets(p)?,
        Some("view") => s.visible_cloned(),
        Some("selection") => s.selected(),
        Some(other) => return Err(bad(id, format!("`source` must be selection or view, not `{other}`"))),
        None => {
            let sel = s.selected();
            if sel.is_empty() { s.visible_cloned() } else { sel }
        }
    };
    if ids.is_empty() {
        return Err(bad(id, "no photos: select some, or show an album"));
    }
    if ids.len() > MAX_PHOTOS {
        return Err(bad(id, format!("a book holds at most {MAX_PHOTOS} photos")));
    }
    if let Some(missing) = ids.iter().find(|i| s.catalog.photo(**i).is_none()) {
        return Err(bad(id, format!("no photo {}", missing.0)));
    }
    Ok(ids.into_iter().map(|i| i.0).collect())
}

/// `Untitled Book`, `Untitled Book 2`… the first name no book has.
fn free_name(s: &Session, base: &str) -> String {
    let taken = |n: &str| s.books.books.iter().any(|b| b.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} {n}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

fn rect_json(r: &crate::print::layout::Rect) -> Value {
    json!({"x": r.x, "y": r.y, "w": r.w, "h": r.h})
}

/// A book as a front end draws it: settings, page size, every page's template, photos and cell
/// rectangles (inches, page space), the spreads and the tray of unplaced photos.
fn book_json(s: &Session, b: &Book) -> Value {
    let (w, h) = b.settings.page_size().unwrap_or((0.0, 0.0));
    let pages: Vec<Value> = b
        .pages
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let cells = cells_for(p.template, &b.settings).unwrap_or_default();
            json!({"index": i, "template": p.template, "slots": p.slots, "cells": cells.iter().map(rect_json).collect::<Vec<_>>()})
        })
        .collect();
    let all = b.photos();
    json!({
        "id": b.id, "name": b.name, "current": s.books.current == Some(b.id),
        "settings": b.settings, "cover": b.cover, "pageWidth": w, "pageHeight": h,
        "pages": pages, "spreads": b.spreads(), "unplaced": b.unplaced,
        "photos": all.len(),
        "missing": all.iter().filter(|i| s.catalog.photo(lightcraft_catalog::PhotoId(**i)).is_none()).collect::<Vec<_>>(),
        "templates": Template::ALL.map(|t| json!({"name": t.name(), "cells": t.cells()})),
        "sizes": size_names().filter_map(|n| size_of(n).map(|(w, h)| json!({"name": n, "width": w, "height": h}))).collect::<Vec<_>>(),
    })
}

fn state(s: &Session, id: Option<u32>, cmd: &str) -> Result<Value> {
    let b = s.books.find(id).map_err(|e| bad(cmd, e))?;
    Ok(book_json(s, b))
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    let books: Vec<Value> = s
        .books
        .books
        .iter()
        .map(|b| json!({"id": b.id, "name": b.name, "pages": b.pages.len(), "photos": b.photos().len(), "size": b.settings.size, "current": s.books.current == Some(b.id)}))
        .collect();
    Ok(json!({"books": books, "current": s.books.current}))
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    only(p, &["book"], "book.get")?;
    state(s, book_id(p, "book.get")?, "book.get")
}

/// Settings given as a JSON object merge over `base`; unknown or mistyped keys are errors.
fn merged(base: &BookSettings, patch: &Value, id: &str) -> Result<BookSettings> {
    let mut value = serde_json::to_value(base).map_err(|e| bad(id, e.to_string()))?;
    if let (Some(v), Some(pa)) = (value.as_object_mut(), patch.as_object()) {
        for (k, x) in pa {
            v.insert(k.clone(), x.clone());
        }
    }
    serde_json::from_value(value).map_err(|e| bad(id, e.to_string()))
}

fn preset(p: &Value, id: &str) -> Result<Preset> {
    match p.get("preset") {
        None | Some(Value::Null) => Ok(Preset::Mixed),
        Some(v) => Preset::parse(v.as_str().ok_or_else(|| bad(id, "`preset` must be a name"))?).map_err(|e| bad(id, e)),
    }
}

fn flag(p: &Value, key: &str, d: bool, id: &str) -> Result<bool> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(d),
        Some(v) => v.as_bool().ok_or_else(|| bad(id, format!("`{key}` must be true or false"))),
    }
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.create";
    only(p, &["name", "ids", "id", "source", "preset", "cover", "settings"], ID)?;
    let ids = photos(s, p, ID)?;
    let name = match str_param(p, "name") {
        Some(n) => clean_name(n).map_err(|e| bad(ID, e))?,
        None => free_name(s, "Untitled Book"),
    };
    let settings = merged(&BookSettings::default(), p.get("settings").unwrap_or(&Value::Null), ID)?;
    let cover = flag(p, "cover", true, ID)?;
    let mut book = Book { name, settings, ..Default::default() };
    book.relayout(&ids, cover, preset(p, ID)?).map_err(|e| bad(ID, e))?;
    book.validate().map_err(|e| bad(ID, e))?;
    let before = s.books.clone();
    let new = s.books.add(book).map_err(|e| bad(ID, e))?;
    s.save_books(before)?;
    state(s, Some(new), ID)
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.open";
    only(p, &["book"], ID)?;
    let id = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.id;
    let before = s.books.clone();
    s.books.current = Some(id);
    s.save_books(before)?;
    state(s, Some(id), ID)
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.delete";
    only(p, &["book"], ID)?;
    let id = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.id;
    let before = s.books.clone();
    s.books.remove(id).map_err(|e| bad(ID, e))?;
    s.save_books(before)?;
    list(s, &Value::Null)
}

/// Changes a copy of a book, checks it, then stores and saves it; the book is untouched on error.
fn change(s: &mut Session, p: &Value, id: &str, f: impl FnOnce(&mut Book) -> std::result::Result<(), String>) -> Result<Value> {
    let which = s.books.find(book_id(p, id)?).map_err(|e| bad(id, e))?.id;
    let mut next = s.books.find(Some(which)).map_err(|e| bad(id, e))?.clone();
    f(&mut next).map_err(|e| bad(id, e))?;
    next.validate().map_err(|e| bad(id, e))?;
    let before = s.books.clone();
    *s.books.find_mut(Some(which)).map_err(|e| bad(id, e))? = next;
    s.save_books(before)?;
    state(s, Some(which), id)
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.set";
    const KEYS: [&str; 7] = ["size", "orientation", "margin", "padding", "background", "fill", "ppi"];
    only(p, &["book", "name", "settings", "size", "orientation", "margin", "padding", "background", "fill", "ppi"], ID)?;
    // the settings may come nested (`settings`) or flat
    let mut patch = p.get("settings").cloned().unwrap_or_else(|| json!({}));
    for k in KEYS {
        if let (Some(v), Some(o)) = (p.get(k), patch.as_object_mut()) {
            o.insert(k.to_string(), v.clone());
        }
    }
    let name = str_param(p, "name").map(clean_name).transpose().map_err(|e| bad(ID, e))?;
    change(s, p, ID, |b| {
        b.settings = merged(&b.settings, &patch, ID).map_err(|e| e.to_string())?;
        if let Some(n) = name {
            b.name = n;
        }
        Ok(())
    })
}

fn layout(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.layout";
    only(p, &["book", "ids", "id", "source", "preset", "cover"], ID)?;
    let own = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?;
    let (cover, mine) = (own.cover, own.photos());
    let ids = if names_photos(p) || p.get("source").is_some() { photos(s, p, ID)? } else { mine };
    let cover = flag(p, "cover", cover, ID)?;
    let preset = preset(p, ID)?;
    change(s, p, ID, |b| b.relayout(&ids, cover, preset))
}

fn set_template(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.setTemplate";
    only(p, &["book", "page", "template"], ID)?;
    let page = index_req(p, "page", ID)?;
    let template = Template::parse(str_param(p, "template").ok_or_else(|| bad(ID, "missing `template`"))?).map_err(|e| bad(ID, e))?;
    change(s, p, ID, |b| b.set_template(page, template))
}

fn slot(p: &Value, key: &str, id: &str) -> Result<Slot> {
    let v = p.get(key).filter(|v| v.is_object()).ok_or_else(|| bad(id, format!("`{key}` must be {{page, cell}} or {{unplaced}}")))?;
    if v.get("unplaced").is_some() {
        return Ok(Slot::Unplaced { unplaced: index_req(v, "unplaced", id)? });
    }
    Ok(Slot::Cell { page: index_req(v, "page", id)?, cell: index_req(v, "cell", id)? })
}

fn move_photo(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.movePhoto";
    only(p, &["book", "from", "to"], ID)?;
    let (from, to) = (slot(p, "from", ID)?, slot(p, "to", ID)?);
    change(s, p, ID, |b| b.move_photo(from, to))
}

// ---- export ----------------------------------------------------------------------------------

/// Renders the book with a row in the activity stack that cancels it.
fn render_book(s: &mut Session, book: &Book, label: &str, id: &str) -> Result<render::Rendered> {
    let job = render::prepare(s, book).map_err(|e| bad(id, e))?;
    let total = job.steps() as u64;
    let row = s.activity.start("export", label, Cancel::Yes);
    row.progress(0, total);
    job.run(&mut |done, name| {
        row.progress(done as u64, total);
        row.detail(name);
        !row.is_cancelled()
    })
    .map_err(|e| bad(id, e))
}

fn export_pdf(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.exportPdf";
    only(p, &["book", "path"], ID)?;
    let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(ID, "missing path"))?;
    s.check_write_target(path).map_err(|e| bad(ID, e))?;
    let book = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.clone();
    let rendered = render_book(s, &book, "Exporting book to PDF", ID)?;
    let bytes = rendered.pdf().map_err(|e| bad(ID, e))?;
    s.check_write_target(path).map_err(|e| bad(ID, e))?;
    crate::export::write_file(path, &bytes).map_err(|e| bad(ID, e))?;
    Ok(json!({"path": path, "pages": rendered.pages.len(), "photos": book.photos().len(), "bytes": bytes.len()}))
}

fn export_jpeg(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.exportJpeg";
    only(p, &["book", "dir", "name", "quality"], ID)?;
    let dir = str_param(p, "dir").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(ID, "missing dir"))?;
    let quality = match p.get("quality") {
        None | Some(Value::Null) => 90,
        Some(v) => v.as_u64().filter(|q| (1..=100).contains(q)).ok_or_else(|| bad(ID, "`quality` must be 1 to 100"))? as u8,
    };
    let book = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.clone();
    let targets = render::jpeg_targets(dir, str_param(p, "name").unwrap_or(&book.name), book.pages.len());
    for t in &targets {
        s.check_write_target(t).map_err(|e| bad(ID, e))?;
    }
    let rendered = render_book(s, &book, "Exporting book to JPEG", ID)?;
    let jpegs = rendered.jpegs(quality).map_err(|e| bad(ID, e))?;
    let mut bytes = 0usize;
    for (t, j) in targets.iter().zip(&jpegs) {
        s.check_write_target(t).map_err(|e| bad(ID, e))?;
        crate::export::write_file(t, j).map_err(|e| bad(ID, e))?;
        bytes = bytes.saturating_add(j.len());
    }
    Ok(json!({"files": targets, "pages": jpegs.len(), "bytes": bytes}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "book.list", "Books", [], None, "{} → {books: [{id, name, pages, photos, size, current}], current}", always, list),
        cmd!(query "book.get", "Book", [], None,
            "{book?} → {id, name, settings, cover, pageWidth, pageHeight, pages: [{index, template, slots: [photo id | null], cells: [{x,y,w,h} inches]}], spreads, unplaced, photos, missing, templates, sizes}; page and cell numbers are zero-based",
            always, get),
        cmd!(query "book.create", "Create Book", [], None,
            "{name?, ids?, source?: selection|view, preset?: mixed|one|two|three|grid4|fullBleed, cover?: bool (default true), settings?: {size, orientation, margin, padding, background: [r,g,b], fill, ppi}} — lays the photos out (default: the selection, else the photos in view) and makes the book current → book.get",
            always, create),
        cmd!(query "book.open", "Open Book", [], None, "{book} — makes it the current book → book.get", always, open),
        cmd!(query "book.delete", "Delete Book", [], None, "{book?} → book.list", always, delete),
        cmd!(query "book.set", "Set Book Settings", [], None,
            "{book?, name?, size?: 5x7|6x9|7x7|8x8|8x10|10x10|11x14|12x12|a5|a4|letter, orientation?: portrait|landscape, margin?: inches 0..2, padding?: inches 0..1, background?: [r,g,b], fill?: bool, ppi?: 72..720} (also nested in `settings`) → book.get",
            always, set),
        cmd!(query "book.layout", "Auto Layout Book", [], None,
            "{book?, ids?, source?, preset?: mixed|one|two|three|grid4|fullBleed, cover?: bool} — lays the photos out again (default: the book's own photos in order) → book.get",
            always, layout),
        cmd!(query "book.setTemplate", "Set Page Template", [], None,
            "{book?, page, template: one|two|three|grid4|fullBleed} — photos that no longer fit go to `unplaced`, and new cells take them → book.get",
            always, set_template),
        cmd!(query "book.movePhoto", "Move Book Photo", [], None,
            "{book?, from: {page, cell} | {unplaced}, to: {page, cell} | {unplaced}} — a cell that holds a photo swaps with it; `to: {unplaced: 0}` takes the photo off its page → book.get",
            always, move_photo),
        cmd!(query "book.exportPdf", "Export Book as PDF", [], None,
            "{book?, path} → {path, pages, photos, bytes}; one PDF page per book page at the book's ppi; replaces an existing file, never an original; shown in the activity stack and cancellable",
            always, export_pdf),
        cmd!(query "book.exportJpeg", "Export Book as JPEGs", [], None,
            "{book?, dir, name?, quality?: 1..100 (90)} → {files, pages, bytes}; writes `<name>-01.jpg`… one per page (sRGB); shown in the activity stack and cancellable",
            always, export_jpeg),
    ]
}
