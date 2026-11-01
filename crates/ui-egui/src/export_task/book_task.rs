//! The Book view's exports: a PDF or a folder of JPEGs, rendered on a worker thread with a row
//! in the activity stack that cancels it (the same task as the other exports, see [`ExportTask`]).
//!
//! `view.book` (and `dialog.book`) open the Book view ([`crate::panels::book`]); `app.bookPdf` and
//! `app.bookJpeg` ask for a destination (when `path` / `dir` is not given) and start the export.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use lightcraft_engine::activity::Cancel;
use lightcraft_engine::book::render;
use serde_json::{Value, json};

use super::ExportTask;
use crate::LightcraftApp;
use crate::pick::{PickRequest, Picked};

/// JPEG quality of the pages.
const JPEG_QUALITY: u8 = 90;

/// How many steps a finished export did: a book's one summary entry stands for all its renders, so
/// it is `total`; any other export has one entry per photo.
pub fn finished_done(files: &[Value], total: usize) -> usize {
    if files.first().is_some_and(|f| f["book"] == true) { total } else { files.len() }
}

/// Starts exporting the current book (or `params.book`): a PDF at `params.path`, or with `jpeg` a
/// folder of JPEGs at `params.dir`.
pub fn start(app: &mut LightcraftApp, params: &Value, jpeg: bool) -> Result<Value, String> {
    if app.export.is_some() {
        return Err("an export is already running".into());
    }
    let key = if jpeg { "dir" } else { "path" };
    let target =
        params.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("missing {key}"))?.to_string();
    let which = match params.get("book") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("`book` must be a book id")?),
    };
    let book = app.session.books.find(which)?.clone();
    let files = if jpeg { render::jpeg_targets(&target, &book.name, book.pages.len()) } else { vec![target.clone()] };
    for f in &files {
        app.session.check_write_target(f)?;
    }
    let guard = app.session.original_guard();
    let job = render::prepare(&mut app.session, &book)?;
    let total = job.steps();
    let write = app.services.write_shared.clone();
    let progress = Arc::new(Mutex::new((0, String::new())));
    let cancel = Arc::new(AtomicBool::new(false));
    let (p, c) = (progress.clone(), cancel.clone());
    let (tx, rx) = channel();
    let photos = book.photos().len();
    let pages = book.pages.len();
    let work = move || {
        let mut progress = |done: usize, name: &str| {
            if let Ok(mut g) = p.lock() {
                *g = (done, name.to_string());
            }
            !c.load(Ordering::Relaxed)
        };
        let result =
            (if jpeg { job.run_jpegs(JPEG_QUALITY, &mut progress) } else { job.run_pdf(&mut progress).map(|pdf| vec![pdf]) }).and_then(|parts| {
                let mut bytes = 0usize;
                for (path, part) in files.iter().zip(&parts) {
                    if c.load(Ordering::Relaxed) {
                        return Err(render::CANCELLED.to_string());
                    }
                    guard.check(std::path::Path::new(path))?;
                    match &write {
                        Some(write) => write(path, part)?,
                        None => lightcraft_engine::export::write_file(path, part)?,
                    }
                    bytes = bytes.saturating_add(part.len());
                }
                Ok(vec![json!({"path": target, "pages": pages, "photos": photos, "bytes": bytes, "book": true, "jpeg": jpeg})])
            });
        let _ = tx.send(result.map(|files| (files, None, c.load(Ordering::Relaxed))));
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name("book".into()).spawn(work).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    work();
    let row =
        app.session.activity.start("export", if jpeg { "Exporting book to JPEG" } else { "Exporting book to PDF" }, Cancel::Flag(cancel.clone()));
    row.progress(0, total as u64);
    app.export = Some(ExportTask { total, progress, cancel, rx, guard: row, show_in_finder: false });
    Ok(json!({"background": true, "total": total}))
}

/// The UI commands of the Book module; `None` for any other id.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if let Some(r) = crate::panels::book::command(app, id, p) {
        return Some(r);
    }
    let jpeg = match id {
        "app.bookPdf" => false,
        "app.bookJpeg" => true,
        _ => return None,
    };
    let key = if jpeg { "dir" } else { "path" };
    let mut params = p.clone();
    if p.get(key).is_none() {
        let name = app.session.books.find(None).map(|b| b.name.clone()).unwrap_or_else(|_| "Book".into());
        let request = if jpeg {
            PickRequest::folder("Export Book as JPEGs")
        } else {
            PickRequest::save("Export Book as PDF", "PDF", &["pdf"], format!("{}.pdf", render::file_stem(&name)))
        };
        let paths = match crate::pick::ask(app, id, p, key, request, |_| None) {
            Picked::Now(paths) => paths,
            Picked::Later => return Some(Ok(Value::Null)),
            Picked::Unavailable => return Some(Err(format!("no file dialog on this platform; supply {key}"))),
        };
        let Some(path) = paths.first() else { return Some(Ok(Value::Null)) };
        params[key] = json!(path);
    }
    Some(start(app, &params, jpeg))
}
