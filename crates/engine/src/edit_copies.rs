//! Edit In's copies in three steps, so the slow one can leave the UI thread: [`prepare`] reads the
//! session (cheap), [`EditPlan::run`] renders and writes every copy without it (another thread),
//! and [`finish`] adds the copies to the library, stacked on their originals. `photo.editExternal`
//! runs all three in place; the desktop app runs `run` on a worker with a row in the activity
//! stack.

use std::path::Path;

use lightcraft_catalog::{PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::export::{Destination, ExportOptions, PreparedExport, run_batch};

/// Most photos one Edit In renders (each becomes a full-size file on disk and a library photo).
pub const MAX_EDIT_COPIES: usize = 50;

/// What [`EditPlan::run`] returns when its progress callback stopped it.
pub const CANCELLED: &str = "Edit In cancelled";

/// Edit In set up from the session; [`EditPlan::run`] needs no session.
pub struct EditPlan {
    opts: ExportOptions,
    /// Each photo's export and the folder its copy goes in (beside its original).
    items: Vec<(PreparedExport, String)>,
}

/// What [`EditPlan::run`] wrote: `(original, copy)` per photo done, in order, and why it stopped
/// early (a failed render or write; [`CANCELLED`]), if it did.
#[derive(Debug, Default)]
pub struct Rendered {
    pub copies: Vec<(PhotoId, String)>,
    pub error: Option<String>,
}

/// Set up Edit In for the photos `p` names (`ids?`; `format`, `colorSpace`, `bitDepth`; `dir` for
/// a generated demo photo, which has no folder). Refuses none and more than [`MAX_EDIT_COPIES`].
pub fn prepare(s: &mut Session, p: &Value) -> Result<EditPlan, String> {
    let mut ids = s.targets(p).map_err(|e| e.to_string())?;
    ids.dedup();
    if ids.is_empty() {
        return Err("no active photo".into());
    }
    if ids.len() > MAX_EDIT_COPIES {
        return Err(format!("{} photos selected: edit at most {MAX_EDIT_COPIES} at a time", ids.len()));
    }
    let opts = crate::cmd::convert::edit_options(p, "{name}-Edit").map_err(|e| e.to_string())?;
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let ph = s.catalog.photo(id).ok_or_else(|| format!("no photo {}", id.0))?;
        let dir = match &ph.source {
            Source::File { path } => Path::new(path).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
            Source::Demo { .. } => match crate::cmd::str_param(p, "dir") {
                Some(d) => d.to_string(),
                None => return Err("a generated demo photo has no folder: give `dir`".into()),
            },
        };
        items.push((crate::export::prepare_export(s, id, &opts, 1)?, dir));
    }
    Ok(EditPlan { opts, items })
}

impl EditPlan {
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// Render and write each copy in turn (a free name beside its original, never over a file).
    /// `progress(done, next file)` returns false to stop; a stop removes nothing already written
    /// but is reported as [`CANCELLED`].
    pub fn run(self, write: &mut dyn FnMut(&str, &[u8]) -> Result<(), String>, progress: &mut dyn FnMut(usize, &str) -> bool) -> Rendered {
        let mut out = Rendered::default();
        let ext = format!(".{}", self.opts.format.extension());
        for (done, (item, dir)) in self.items.into_iter().enumerate() {
            if !progress(done, &item.file_name) {
                out.error = Some(CANCELLED.into());
                break;
            }
            let photo = item.photo;
            let mut written = Vec::new();
            let mut w = |path: &str, bytes: &[u8]| {
                write(path, bytes)?;
                written.push(path.to_string());
                Ok(())
            };
            let to = Destination { dir, exact: None };
            if let Err(e) = run_batch(vec![item], &self.opts, &to, &mut w, &|f| Path::new(f).exists(), true, &mut |_, _| true) {
                out.error = Some(e);
                break;
            }
            match written.into_iter().find(|w| w.ends_with(&ext)) {
                Some(copy) => out.copies.push((photo, copy)),
                None => {
                    out.error = Some("nothing was written".into());
                    break;
                }
            }
        }
        out
    }
}

/// Add the copies to the library, each stacked on top of its original (expanded so both show),
/// and select them. The result is `photo.editExternal`'s: the first copy's `{path, id, original}`
/// and every one in `copies`. A run that stopped early is an error, after the copies it made are
/// in the library.
pub fn finish(s: &mut Session, r: Rendered) -> Result<Value, String> {
    let mut copies = Vec::with_capacity(r.copies.len());
    for (original, path) in r.copies {
        let imported = s.execute("library.import", &json!({"paths": [path]})).map_err(|e| e.to_string())?;
        let new = imported["imported"].get(0).and_then(Value::as_u64).ok_or("the edit copy could not be added")?;
        let _ = s.execute("stack.group", &json!({"ids": [new, original.0], "top": new, "collapsed": false}));
        copies.push(json!({"path": path, "id": new, "original": original.0}));
    }
    if let Some(e) = r.error {
        return Err(e);
    }
    let new: Vec<PhotoId> = copies.iter().filter_map(|c| c["id"].as_u64()).map(PhotoId).collect();
    s.selection = crate::Selection { active: new.first().copied(), ids: new };
    let first = copies.first().cloned().unwrap_or(Value::Null);
    Ok(json!({"path": first["path"], "id": first["id"], "original": first["original"], "copies": copies}))
}
