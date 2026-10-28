//! Open as Layers (Photo ▸ Edit In ▸ Open as Layers in PhotoCraft…): several photos, rendered with
//! their edits, become the layers of one new PhotoCraft document (double and multiple exposures).
//!
//! The engine half renders the layer files (the same file as Edit in PhotoCraft: format, colour
//! space and bit depth from its settings) into a folder of their own next to the top photo, each
//! named after its original photo (PhotoCraft names the layer after the file), and writes the
//! `.pcstack` file that tells PhotoCraft which files are the layers, top first (version 1 of the
//! interface in the PhotoCraft fork's `FORK.md`). The host opens the `.pcstack`. The files are not
//! added to the library: the layered document is what the user wants, and 2 to 256 scratch copies
//! would crowd the grid.

use std::path::{Path, PathBuf};

use lightcraft_catalog::{PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::export::{Destination, ExportOptions, PreparedExport, prepare_batch, run_batch};

/// Most layers one document takes (PhotoCraft's own limit).
pub const MAX_LAYERS: usize = 256;
/// The extension of the file that lists the layers.
pub const STACK_EXTENSION: &str = "pcstack";
/// What a stopped run says (the host words its own toast).
pub const CANCELLED: &str = "Open as Layers cancelled";
/// `kind` and `version` of the `.pcstack` file.
const KIND: &str = "photocraft-open-as-layers";
const VERSION: u32 = 1;

/// The new document's name: the top photo's name, and how many photos lie under it
/// (`DSC01234 + 2`).
pub fn doc_name(top_file_name: &str, count: usize) -> String {
    let stem = Path::new(top_file_name).file_stem().map(|s| s.to_string_lossy().trim().to_string()).filter(|s| !s.is_empty());
    let stem = stem.unwrap_or_else(|| "Layers".into());
    match count.saturating_sub(1) {
        0 => stem,
        n => format!("{stem} + {n}"),
    }
}

/// The `.pcstack` file's text: `files` are absolute paths, top layer first, 1 to [`MAX_LAYERS`].
pub fn pcstack_json(name: &str, files: &[String]) -> Result<String, String> {
    if files.is_empty() {
        return Err("no layer files".into());
    }
    if files.len() > MAX_LAYERS {
        return Err(format!("{} layers: a document takes at most {MAX_LAYERS}", files.len()));
    }
    if let Some(f) = files.iter().find(|f| !Path::new(f).is_absolute()) {
        return Err(format!("{f}: a layer file needs an absolute path"));
    }
    serde_json::to_string_pretty(&json!({"kind": KIND, "version": VERSION, "name": name, "files": files})).map_err(|e| e.to_string())
}

/// Whether an application's `Info.plist` (XML form) lists `pcstack` among its
/// `CFBundleDocumentTypes`: the PhotoCraft fork that opens a layer list. The official PhotoCraft
/// doesn't, and is given the files one by one instead.
pub fn plist_lists_pcstack(plist: &str) -> bool {
    const KEY: &str = "<key>CFBundleDocumentTypes</key>";
    let Some(rest) = plist.find(KEY).and_then(|i| plist.get(i + KEY.len()..)) else { return false };
    // the key's value is the array that follows it
    let body = array_body(rest);
    body.split("<string>").skip(1).filter_map(|s| s.split("</string>").next()).any(|s| s.trim().eq_ignore_ascii_case(STACK_EXTENSION))
}

/// The text of the first `<array>…</array>` in `text` (nested arrays included), or empty.
fn array_body(text: &str) -> &str {
    let Some(start) = text.find("<array>") else { return "" };
    let mut depth = 0usize;
    let mut i = start;
    while let Some(rel) = text.get(i..).and_then(|t| t.find('<')) {
        let tag_at = i + rel;
        let rest = text.get(tag_at..).unwrap_or_default();
        if rest.starts_with("<array>") {
            depth += 1;
        } else if rest.starts_with("</array>") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return text.get(start..tag_at).unwrap_or_default();
            }
        }
        i = tag_at + 1;
    }
    ""
}

/// The photos of an Open as Layers call, top layer first: an explicit `ids` in the order given;
/// otherwise the selection with the active photo on top and the rest in selection order. Repeats
/// are dropped.
pub fn ordered_targets(s: &Session, p: &Value) -> Result<Vec<PhotoId>, String> {
    let explicit = ["ids", "id"].iter().any(|k| p.get(*k).is_some_and(|v| !v.is_null()));
    let mut seen = std::collections::HashSet::new();
    let mut ids: Vec<PhotoId> = s.targets(p).map_err(|e| e.to_string())?.into_iter().filter(|id| seen.insert(*id)).collect();
    if !explicit && let Some(active) = s.selection.active.filter(|a| ids.contains(a)) {
        ids.retain(|id| *id != active);
        ids.insert(0, active);
    }
    Ok(ids)
}

/// Refuse a layer count a document can't take (before anything is rendered).
pub fn check_count(n: usize) -> Result<(), String> {
    match n {
        0 | 1 => Err("select at least two photos to open as layers".into()),
        n if n > MAX_LAYERS => Err(format!("{n} photos selected: a document takes at most {MAX_LAYERS} layers")),
        _ => Ok(()),
    }
}

/// An Open as Layers set up from the session ([`prepare`]); [`LayersPlan::run`] renders and
/// writes without the session, so it can run on another thread.
pub struct LayersPlan {
    /// The new document's name.
    pub name: String,
    /// The folder the layer files and the `.pcstack` go in.
    pub dir: String,
    /// The photos, top layer first.
    pub ids: Vec<PhotoId>,
    opts: ExportOptions,
    items: Vec<PreparedExport>,
}

/// What [`LayersPlan::run`] wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Layers {
    pub name: String,
    pub dir: String,
    /// The layer files, top first.
    pub files: Vec<String>,
    /// The `.pcstack` file (`None` when not asked for).
    pub stack: Option<String>,
}

/// Set up Open as Layers for the photos `p` names (see [`ordered_targets`]; `format`, `colorSpace`
/// and `bitDepth` as for `photo.editExternal`; `dir` for a generated demo photo, which has no
/// folder). Refuses fewer than two and more than [`MAX_LAYERS`] photos.
pub fn prepare(s: &mut Session, p: &Value) -> Result<LayersPlan, String> {
    let ids = ordered_targets(s, p)?;
    check_count(ids.len())?;
    let opts = crate::cmd::convert::edit_options(p, "{name}").map_err(|e| e.to_string())?;
    let top = ids.first().and_then(|id| s.catalog.photo(*id)).ok_or("no such photo")?;
    let name = doc_name(&top.file_name, ids.len());
    // beside the top photo; a generated photo has no folder, so the call gives one
    let from_photo = |id: &PhotoId| match s.catalog.photo(*id).map(|ph| &ph.source) {
        Some(Source::File { path }) => Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf),
        _ => None,
    };
    let base = ids
        .first()
        .and_then(from_photo)
        .or_else(|| super::cmd::str_param(p, "dir").map(PathBuf::from))
        .ok_or("a generated demo photo has no folder: give `dir`")?;
    let dir = free_folder(&base, &name)?.to_string_lossy().to_string();
    let items = prepare_batch(s, &ids, &opts)?;
    Ok(LayersPlan { name, dir, ids, opts, items })
}

/// `<name> Layers`, then `<name> Layers 2`… in `base`: a folder that doesn't exist yet.
fn free_folder(base: &Path, name: &str) -> Result<PathBuf, String> {
    (1u32..=9999)
        .map(|n| if n == 1 { base.join(format!("{name} Layers")) } else { base.join(format!("{name} Layers {n}")) })
        .find(|f| !f.exists())
        .ok_or_else(|| format!("{}: too many layer folders for {name}", base.display()))
}

impl LayersPlan {
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// Render every photo to its layer file, then (with `stack`) write the `.pcstack`. `write`
    /// puts a file on disk; `progress(done, next file)` returns false to cancel. Any failure or a
    /// cancel removes what was written (a layer missing from the document would go unnoticed),
    /// and says why.
    pub fn run(
        self,
        stack: bool,
        write: &mut dyn FnMut(&str, &[u8]) -> Result<(), String>,
        progress: &mut dyn FnMut(usize, &str) -> bool,
    ) -> Result<Layers, String> {
        let total = self.items.len();
        let to = Destination { dir: self.dir.clone(), exact: None };
        let mut written: Vec<String> = Vec::new();
        let mut cancelled = false;
        let result = {
            let mut w = |path: &str, bytes: &[u8]| {
                write(path, bytes)?;
                written.push(path.to_string());
                Ok(())
            };
            let mut p = |done: usize, name: &str| {
                let go = progress(done, name);
                cancelled |= !go;
                go
            };
            run_batch(self.items, &self.opts, &to, &mut w, &|f| Path::new(f).exists(), true, &mut p)
        };
        let outcome = result.and_then(|files| {
            let paths: Vec<String> = files.iter().filter_map(|f| f["path"].as_str().map(str::to_string)).collect();
            if cancelled || paths.len() < total {
                return Err(CANCELLED.to_string());
            }
            let stack = match stack {
                true => {
                    let file = Path::new(&self.dir).join(format!("{}.{STACK_EXTENSION}", self.name)).to_string_lossy().to_string();
                    write(&file, pcstack_json(&self.name, &paths)?.as_bytes())?;
                    written.push(file.clone());
                    Some(file)
                }
                false => None,
            };
            Ok(Layers { name: self.name.clone(), dir: self.dir.clone(), files: paths, stack })
        });
        if outcome.is_err() {
            for f in &written {
                let _ = std::fs::remove_file(f);
            }
            // only when it is empty: never a folder somebody else put things in
            let _ = std::fs::remove_dir(&self.dir);
        }
        outcome
    }
}

#[cfg(test)]
mod tests;
