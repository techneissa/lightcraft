use super::selection::strs;
use super::{AlbumId, PhotoId, Result, Selection, Session, Value, bad, bool_or, json, str_param};

/// The auto-import folder's visible files with their sizes (the file-system half of
/// `library.autoImportScan`; the app lists on a worker thread and passes `listing`).
pub fn list_auto_import_folder(folder: &str) -> std::result::Result<Vec<(String, u64)>, String> {
    let rd = std::fs::read_dir(folder).map_err(|e| format!("{folder}: {e}"))?;
    Ok(rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let m = std::fs::metadata(e.path()).ok()?;
            m.is_file().then(|| (e.path().to_string_lossy().to_string(), m.len()))
        })
        .collect())
}

/// What one poll of the auto-import folder found.
pub struct AutoImportPoll {
    /// the folder's modification time when polled (`None` where the file system has none)
    pub stamp: Option<std::time::SystemTime>,
    /// its files, when they were listed: not when the folder's time is `last` and `force` is off
    pub listing: Option<Vec<(String, u64)>>,
}

/// Poll the auto-import folder: one `stat` of the folder, and its files only when its modification
/// time differs from `last` (a new file bumps it) or `force` is set (a file still being copied grows
/// without bumping it, and a subfolder's change doesn't bump its parent). Keeps an idle watch of a
/// big folder from listing it every few seconds. The time is read before the listing, so a file
/// added in between shows up in the next poll.
pub fn poll_auto_import_folder(folder: &str, last: Option<std::time::SystemTime>, force: bool) -> std::result::Result<AutoImportPoll, String> {
    let stamp = std::fs::metadata(folder).and_then(|m| m.modified()).ok();
    if !force && stamp.is_some() && stamp == last {
        return Ok(AutoImportPoll { stamp, listing: None });
    }
    Ok(AutoImportPoll { stamp, listing: Some(list_auto_import_folder(folder)?) })
}

/// `library.import`'s params, checked (no file-system calls).
pub struct ImportRequest {
    pub paths: Vec<String>,
    pub opts: crate::import::ImportOptions,
    pub album: Option<u64>,
    pub album_name: Option<String>,
}

/// Parse and check `library.import`'s params (the app's import task runs the import itself, on a
/// worker thread, with the same options).
pub fn import_params(s: &Session, p: &Value) -> Result<ImportRequest> {
    let paths = strs(p, "paths");
    let mode = match str_param(p, "mode").unwrap_or("add") {
        "add" => crate::import::ImportMode::Add,
        "copy" => crate::import::ImportMode::Copy,
        "move" => crate::import::ImportMode::Move,
        other => return Err(bad("library.import", format!("unknown mode `{other}` (add|copy|move)"))),
    };
    let preset = match str_param(p, "preset").filter(|x| !x.is_empty()) {
        Some(id) => Some(s.presets.iter().find(|x| x.id == id).cloned().ok_or_else(|| bad("library.import", format!("unknown preset `{id}`")))?),
        None => None,
    };
    let album = p.get("album").and_then(Value::as_u64);
    if let Some(a) = album
        && s.catalog.album(AlbumId(a)).is_none_or(|al| al.folder || al.is_smart())
    {
        return Err(bad("library.import", "album must be a regular album"));
    }
    let organize = match str_param(p, "organize") {
        Some(o) => crate::import::Organize::parse(o).ok_or_else(|| {
            bad("library.import", format!("unknown organize `{o}` (date|month|flat, or a folder template like {{date:%Y}}/{{date:%Y%m%d}})"))
        })?,
        None => Default::default(),
    };
    if let crate::import::Organize::Template(t) = &organize
        && let Some(e) = crate::rename::folder_template_error(t)
    {
        return Err(bad("library.import", e));
    }
    let metadata_preset = str_param(p, "metadataPreset").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    if let Some(n) = &metadata_preset
        && !s.metadata_presets.iter().any(|m| m.name.eq_ignore_ascii_case(n))
    {
        return Err(bad("library.import", format!("unknown metadata preset `{n}`")));
    }
    let on_deleted = match str_param(p, "onDeleted") {
        Some(v) => {
            crate::import::OnDeleted::parse(v).ok_or_else(|| bad("library.import", format!("unknown onDeleted `{v}` (restore|skip|fresh)")))?
        }
        None => Default::default(),
    };
    let opts = crate::import::ImportOptions {
        on_deleted,
        mode,
        preset,
        keywords: strs(p, "keywords"),
        destination: str_param(p, "destination").map(str::to_string),
        organize,
        rename: str_param(p, "rename").map(str::to_string),
        rename_start: p.get("renameStart").and_then(Value::as_u64).unwrap_or(1) as usize,
        metadata_preset,
        convert_dng: bool_or(p, "dng", false),
        local: bool_or(p, "local", false),
    };
    let album_name = str_param(p, "albumName").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    Ok(ImportRequest { paths, opts, album, album_name })
}

/// After a batch of an import was committed: the album (a new one named `album_name` when there
/// is no `album` and something was imported) gets the photos. Returns the report as JSON (with
/// `album`).
pub fn import_batch_done(s: &mut Session, report: crate::import::ImportReport, mut album: Option<u64>, album_name: Option<&str>) -> Result<Value> {
    let mut report = serde_json::to_value(report).unwrap_or_default();
    // photos an import restored from Recently Deleted take part like new ones (album, selection)
    let imported: Vec<u64> =
        ["imported", "restored"].iter().flat_map(|k| report[*k].as_array().into_iter().flatten().filter_map(Value::as_u64)).collect();
    if album.is_none()
        && let Some(name) = album_name
        && !imported.is_empty()
    {
        let r = s.execute("album.create", &json!({"name": name}))?;
        album = r["id"].as_u64();
    }
    if let Some(a) = album
        && !imported.is_empty()
    {
        // (album.addPhotos wants a selection: the new photos are about to be it)
        s.selection = Selection::single(PhotoId(imported[0]));
        s.execute("album.addPhotos", &json!({"id": a, "ids": imported}))?;
        report["album"] = json!(a);
    }
    Ok(report)
}
