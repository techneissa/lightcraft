//! `book.*`: the Book module as commands.
//!
//! Books live in [`Session::books`] and are saved to `books.json` in the library folder after each
//! change. Nothing here touches the catalog, so none of it is journaled; every change is one step of
//! the app's undo (see [`crate::book::undo`]). Page and cell numbers are zero-based; `book` is a book id (the current book when left out) and photos are
//! `ids` (a call that names none uses the selection, else the photos in view).
//!
//! * `book.list`, `book.get`, `book.create`, `book.open`, `book.delete`, `book.rename`, `book.duplicate`
//! * `book.set`, `book.layout`, `book.setCover`, `book.setTemplate`, `book.movePhoto`
//! * `book.addPhotos`, `book.addPage`, `book.deletePage`, `book.duplicatePage`, `book.movePage`,
//!   `book.lockPage`, `book.setCellCrop`
//! * `book.exportPdf`, `book.exportJpeg`
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, index_req, names_photos, str_param};
use crate::activity::Cancel;
use crate::book::edit::Preset;
use crate::book::geometry::cells_for;
use crate::book::model::{Book, BookSettings, Crop, Slot, Template, clean_name, size_names, size_of};
use crate::book::pages::Duplicates;
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

/// `settings`: an object, or absent (an empty one). Anything else, `null` included, is an error.
fn settings_param(p: &Value, id: &str) -> Result<Value> {
    match p.get("settings") {
        None => Ok(json!({})),
        Some(v) if v.is_object() => Ok(v.clone()),
        Some(_) => Err(bad(id, "`settings` must be an object")),
    }
}

/// `name`: text, or absent. Anything else is an error, never dropped.
fn name_param(p: &Value, id: &str) -> Result<Option<String>> {
    match p.get("name") {
        None => Ok(None),
        Some(Value::String(n)) => clean_name(n).map(Some).map_err(|e| bad(id, e)),
        Some(_) => Err(bad(id, "`name` must be text")),
    }
}

/// A name no other book has (compared without regard to case).
fn unique_name(s: &Session, name: &str, except: Option<u32>, id: &str) -> Result<()> {
    if s.books.books.iter().any(|b| Some(b.id) != except && b.name.eq_ignore_ascii_case(name)) {
        return Err(bad(id, format!("there is a book named `{name}` already")));
    }
    Ok(())
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
            json!({"index": i, "template": p.template, "slots": p.slots, "crops": p.crops, "locked": p.locked, "cells": cells.iter().map(rect_json).collect::<Vec<_>>()})
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
    only(p, &["name", "ids", "id", "source", "preset", "cover", "settings", "empty"], ID)?;
    let empty = flag(p, "empty", false, ID)?;
    if empty && (names_photos(p) || p.get("source").is_some()) {
        return Err(bad(ID, "`empty` makes a book with no photos: leave out `ids` and `source`, or leave out `empty`"));
    }
    let ids = if empty { Vec::new() } else { photos(s, p, ID)? };
    let name = match name_param(p, ID)? {
        Some(n) => {
            unique_name(s, &n, None, ID)?;
            n
        }
        None => free_name(s, "Untitled Book"),
    };
    let settings = merged(&BookSettings::default(), &settings_param(p, ID)?, ID)?;
    let cover = flag(p, "cover", true, ID)?;
    let mut book = Book { name, settings, ..Default::default() };
    if !empty {
        book.relayout(&ids, cover, preset(p, ID)?).map_err(|e| bad(ID, e))?;
    }
    book.validate().map_err(|e| bad(ID, e))?;
    let before = s.books.clone();
    let new = s.books.add(book).map_err(|e| bad(ID, e))?;
    let step = book_step(&before, new);
    s.save_books(before)?;
    s.push_book_step("Create Book", step);
    state(s, Some(new), ID)
}

/// What to put back to take book `id` away again: it was not there (a new book), with the books on show then.
fn book_step(before: &crate::book::Books, id: u32) -> crate::book::undo::BookStep {
    let mut step = before.snapshot(id);
    step.book = before.books.iter().find(|b| b.id == id).cloned();
    step
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.open";
    only(p, &["book"], ID)?;
    let id = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.id;
    // the book on show is saved when the file can take it; when it can't (books.json was left untouched
    // when the library opened) switching books still works, in memory
    let before = s.books.clone();
    s.books.current = Some(id);
    if s.save_books(before).is_err() {
        s.books.current = Some(id);
    }
    state(s, Some(id), ID)
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.delete";
    only(p, &["book"], ID)?;
    let id = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.id;
    let before = s.books.clone();
    let step = before.snapshot(id);
    s.books.remove(id).map_err(|e| bad(ID, e))?;
    s.save_books(before)?;
    s.push_book_step("Delete Book", step);
    list(s, &Value::Null)
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.rename";
    only(p, &["book", "name"], ID)?;
    let name = name_param(p, ID)?.ok_or_else(|| bad(ID, "missing `name`"))?;
    let which = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.id;
    unique_name(s, &name, Some(which), ID)?;
    change(s, p, ID, "Rename Book", |b| {
        b.name = name;
        Ok(())
    })
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.duplicate";
    only(p, &["book"], ID)?;
    let from = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.clone();
    let mut copy = from.clone();
    // room for " Copy" and a number of two digits (at most 50 books) inside the name limit
    copy.name = free_name(s, &format!("{} Copy", from.name.chars().take(crate::book::MAX_NAME.saturating_sub(8)).collect::<String>()));
    let before = s.books.clone();
    let new = s.books.add(copy).map_err(|e| bad(ID, e))?;
    let step = book_step(&before, new);
    s.save_books(before)?;
    s.push_book_step("Duplicate Book", step);
    state(s, Some(new), ID)
}

/// Changes a copy of a book, checks it, then stores and saves it as one undo step, `label`; the book
/// is untouched on error.
fn change(s: &mut Session, p: &Value, id: &str, label: &str, f: impl FnOnce(&mut Book) -> std::result::Result<(), String>) -> Result<Value> {
    let which = s.books.find(book_id(p, id)?).map_err(|e| bad(id, e))?.id;
    let mut next = s.books.find(Some(which)).map_err(|e| bad(id, e))?.clone();
    f(&mut next).map_err(|e| bad(id, e))?;
    next.validate().map_err(|e| bad(id, e))?;
    let before = s.books.clone();
    let step = before.snapshot(which);
    *s.books.find_mut(Some(which)).map_err(|e| bad(id, e))? = next;
    s.save_books(before)?;
    s.push_book_step(label, step);
    state(s, Some(which), id)
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.set";
    const KEYS: [&str; 7] = ["size", "orientation", "margin", "padding", "background", "fill", "ppi"];
    only(p, &["book", "name", "settings", "size", "orientation", "margin", "padding", "background", "fill", "ppi"], ID)?;
    // the settings may come nested (`settings`) or flat
    let mut patch = settings_param(p, ID)?;
    for k in KEYS {
        if let (Some(v), Some(o)) = (p.get(k), patch.as_object_mut()) {
            o.insert(k.to_string(), v.clone());
        }
    }
    let name = name_param(p, ID)?;
    if let Some(n) = &name {
        let which = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?.id;
        unique_name(s, n, Some(which), ID)?;
    }
    change(s, p, ID, "Change Book Settings", |b| {
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
    change(s, p, ID, "Auto Layout Book", |b| b.relayout(&ids, cover, preset))
}

fn set_cover(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.setCover";
    only(p, &["book", "cover"], ID)?;
    let on = p.get("cover").and_then(Value::as_bool).ok_or_else(|| bad(ID, "`cover` must be true or false"))?;
    change(s, p, ID, "Set Book Cover", |b| b.set_cover(on))
}

fn set_template(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.setTemplate";
    only(p, &["book", "page", "template"], ID)?;
    let page = index_req(p, "page", ID)?;
    let template = Template::parse(str_param(p, "template").ok_or_else(|| bad(ID, "missing `template`"))?).map_err(|e| bad(ID, e))?;
    change(s, p, ID, "Set Page Template", |b| b.set_template(page, template))
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
    change(s, p, ID, "Move Book Photo", |b| b.move_photo(from, to))
}

/// `photos` from the `ids` the call names, else none: adding needs photos named.
fn named_photos(s: &mut Session, p: &Value, id: &str) -> Result<Vec<u64>> {
    if !names_photos(p) && p.get("source").is_none() {
        return Err(bad(id, "name the photos: `ids`, or `source` (selection or view)"));
    }
    photos(s, p, id)
}

fn add_photos(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.addPhotos";
    only(p, &["book", "ids", "id", "source", "duplicates"], ID)?;
    let ids = named_photos(s, p, ID)?;
    let duplicates = match p.get("duplicates") {
        None | Some(Value::Null) => None,
        Some(v) => Some(Duplicates::parse(v.as_str().ok_or_else(|| bad(ID, "`duplicates` must be add or skip"))?).map_err(|e| bad(ID, e))?),
    };
    let book = s.books.find(book_id(p, ID)?).map_err(|e| bad(ID, e))?;
    let dups = book.duplicates_of(&ids);
    let Some(duplicates) = duplicates.or_else(|| dups.is_empty().then_some(Duplicates::Add)) else {
        return Err(bad(ID, format!("{} of these photos are in the book already ({:?}); say `duplicates`: add or skip", dups.len(), dups)));
    };
    let mut added = Default::default();
    let state = change(s, p, ID, "Add Photos to Book", |b| {
        added = b.add_photos(&ids, duplicates)?;
        Ok(())
    })?;
    let mut out = state;
    out["added"] = json!(added.added);
    out["skipped"] = json!(added.skipped);
    Ok(out)
}

fn add_page(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.addPage";
    only(p, &["book", "at", "template"], ID)?;
    let at = match p.get("at") {
        None | Some(Value::Null) => None,
        Some(_) => Some(index_req(p, "at", ID)?),
    };
    let template = match str_param(p, "template") {
        Some(t) => Template::parse(t).map_err(|e| bad(ID, e))?,
        None => Template::One,
    };
    change(s, p, ID, "Add Book Page", |b| b.add_page(at, template).map(|_| ()))
}

fn delete_page(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.deletePage";
    only(p, &["book", "page"], ID)?;
    let page = index_req(p, "page", ID)?;
    change(s, p, ID, "Delete Book Page", |b| b.delete_page(page))
}

fn duplicate_page(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.duplicatePage";
    only(p, &["book", "page"], ID)?;
    let page = index_req(p, "page", ID)?;
    change(s, p, ID, "Duplicate Book Page", |b| b.duplicate_page(page).map(|_| ()))
}

fn move_page(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.movePage";
    only(p, &["book", "from", "to"], ID)?;
    let (from, to) = (index_req(p, "from", ID)?, index_req(p, "to", ID)?);
    change(s, p, ID, "Move Book Page", |b| b.move_page(from, to))
}

fn lock_page(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.lockPage";
    only(p, &["book", "page", "locked"], ID)?;
    let page = index_req(p, "page", ID)?;
    let locked = flag(p, "locked", true, ID)?;
    change(s, p, ID, if locked { "Lock Book Page" } else { "Unlock Book Page" }, |b| b.lock_page(page, locked))
}

/// The number `key` of `p`: a finite number, else an error.
fn number(p: &Value, key: &str, id: &str) -> Result<f64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(id, format!("`{key}` must be a number")))
}

fn set_cell_crop(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "book.setCellCrop";
    only(p, &["book", "page", "cell", "zoom", "x", "y", "reset"], ID)?;
    let (page, cell) = (index_req(p, "page", ID)?, index_req(p, "cell", ID)?);
    let crop = if flag(p, "reset", false, ID)? {
        None
    } else {
        let zoom = number(p, "zoom", ID)?;
        let at = |key| match p.get(key) {
            None | Some(Value::Null) => Ok(0.5),
            Some(_) => number(p, key, ID),
        };
        Some(Crop { zoom, x: at("x")?, y: at("y")? })
    };
    change(s, p, ID, "Crop Book Photo", |b| b.set_cell_crop(page, cell, crop))
}

// ---- export ----------------------------------------------------------------------------------

/// Sets the export up with a row in the activity stack that cancels it, and runs `make` on the job.
fn render_book<T>(
    s: &mut Session,
    book: &Book,
    label: &str,
    id: &str,
    make: impl FnOnce(render::BookJob, &mut dyn FnMut(usize, &str) -> bool) -> std::result::Result<T, String>,
) -> Result<T> {
    let job = render::prepare(s, book).map_err(|e| bad(id, e))?;
    let total = job.steps() as u64;
    let row = s.activity.start("export", label, Cancel::Yes);
    row.progress(0, total);
    make(job, &mut |done, name| {
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
    let bytes = render_book(s, &book, "Exporting book to PDF", ID, |job, progress| job.run_pdf(progress))?;
    s.check_write_target(path).map_err(|e| bad(ID, e))?;
    crate::export::write_file(path, &bytes).map_err(|e| bad(ID, e))?;
    Ok(json!({"path": path, "pages": book.pages.len(), "photos": book.photos().len(), "bytes": bytes.len()}))
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
    let jpegs = render_book(s, &book, "Exporting book to JPEG", ID, |job, progress| job.run_jpegs(quality, progress))?;
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
        cmd!(query "book.setCover", "Set Book Cover", [], None,
            "{book?, cover: bool} — adds a full-bleed cover (taking the first unplaced photo) or removes it (its photo goes to `unplaced`); every other page stays as it is → book.get",
            always, set_cover),
        cmd!(query "book.setTemplate", "Set Page Template", [], None,
            "{book?, page, template: one|two|three|grid4|fullBleed} — photos that no longer fit go to `unplaced`, and new cells take them → book.get",
            always, set_template),
        cmd!(query "book.movePhoto", "Move Book Photo", [], None,
            "{book?, from: {page, cell} | {unplaced}, to: {page, cell} | {unplaced}} — a cell that holds a photo swaps with it; `to: {unplaced: N}` takes the photo off its page and puts it at position N of the tray (0 to its length; N must be below its length when the photo is in the tray already) → book.get",
            always, move_photo),
        cmd!(query "book.rename", "Rename Book", [], None,
            "{book?, name} — a name 1 to 80 characters that no other book has → book.get",
            always, rename),
        cmd!(query "book.duplicate", "Duplicate Book", [], None, "{book?} — a copy named `<name> Copy`, which becomes the current book → book.get", always, duplicate),
        cmd!(query "book.addPhotos", "Add Photos to Book", [], None,
            "{book?, ids?, source?: selection|view, duplicates?: add|skip} → book.get + {added, skipped}; the photos join the tray (`unplaced`). Photos the book holds already need `duplicates`: add (they show twice) or skip; without it such a call is an error",
            always, add_photos),
        cmd!(query "book.addPage", "Add Book Page", [], None,
            "{book?, at?: page number the new page takes (default: the end), template?: one|two|three|grid4|fullBleed (one)} — an empty page; nothing goes before the cover → book.get",
            always, add_page),
        cmd!(query "book.deletePage", "Delete Book Page", [], None, "{book?, page} — its photos go to the end of the tray → book.get", always, delete_page),
        cmd!(query "book.duplicatePage", "Duplicate Book Page", [], None, "{book?, page} — a copy right after it, with its photos, crops and lock → book.get", always, duplicate_page),
        cmd!(query "book.movePage", "Move Book Page", [], None, "{book?, from, to} — the page becomes page `to`; the cover stays first → book.get", always, move_page),
        cmd!(query "book.lockPage", "Lock Book Page", [], None, "{book?, page, locked?: bool (true)} — Auto Layout leaves a locked page as it is → book.get", always, lock_page),
        cmd!(query "book.setCellCrop", "Crop Book Photo", [], None,
            "{book?, page, cell, zoom: 1..4, x?, y?: 0..1 (0.5), reset?: bool} — the photo fills the frame, magnified `zoom` times around the point (x, y) of the photo (fractions of it); `reset: true` goes back to the book's setting; the cell must hold a photo → book.get",
            always, set_cell_crop),
        cmd!(query "book.exportPdf", "Export Book as PDF", [], None,
            "{book?, path} → {path, pages, photos, bytes}; one PDF page per book page at the book's ppi; replaces an existing file, never an original; shown in the activity stack and cancellable",
            always, export_pdf),
        cmd!(query "book.exportJpeg", "Export Book as JPEGs", [], None,
            "{book?, dir, name?, quality?: 1..100 (90)} → {files, pages, bytes}; writes `<name>-01.jpg`… one per page (sRGB); shown in the activity stack and cancellable",
            always, export_jpeg),
    ]
}
