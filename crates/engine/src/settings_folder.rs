//! The sync folder of Settings sync (`fork/settings-sync.md`): Save to Folder and Load from Folder
//! keep every setting in `lightcraft.lcsettings` in a folder the user chose (one in iCloud Drive,
//! say), and Keep in Sync saves there on quit and offers a newer file from another Mac at start.
//!
//! This module knows files and folders only; what a settings file holds, and how it is read and
//! applied, is the app's (`lightcraft_ui_egui::settings_sync`). The file is the settings file plus a
//! top-level `savedBy` object, `{"computer": "<the Mac's name>", "at": "<RFC 3339 time>"}`, a note
//! about the file that is never a setting: a missing or unreadable one is ignored.
//!
//! The same contract as PhotoCraft's (`photocraft.pcsettings`), so both apps share one folder:
//! other ArtCraft apps keep their own file there and only [`FILE_NAME`] is ever read or written
//! here. The chosen folder is also kept in `~/Library/Application Support/ArtCraft/sync-folder.json`
//! (`{"path": "<folder>"}`), shared by the ArtCraft apps: an app with no folder yet offers it.
//!
//! A file is written to a hidden temporary name in the same folder and renamed over the old one, so
//! a sync service never sees half a file, and a failed write leaves the old file alone.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, Local, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The file this app keeps in the sync folder.
pub const FILE_NAME: &str = "lightcraft.lcsettings";
/// The extension of a settings file.
pub const EXTENSION: &str = "lcsettings";
/// The biggest settings file read (user presets can make one a few megabytes); the app's own reading
/// of a file's text uses the same limit (`lightcraft_ui_egui::settings_sync::MAX_BYTES`).
pub const MAX_BYTES: usize = 32_000_000;
/// The ArtCraft folder's file that names the chosen sync folder.
pub const SHARED_NAME: &str = "sync-folder.json";
/// The most of the shared file that is read.
const SHARED_MAX_BYTES: usize = 64 * 1024;
/// The longest Mac name kept.
const NAME_MAX: usize = 100;

/// Who saved a file and when: the file's `savedBy`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBy {
    /// The Mac's name, as macOS shows it in Sharing.
    pub computer: String,
    /// When, as RFC 3339 with the Mac's offset (`2026-11-02T14:30:05+01:00`).
    pub at: String,
}

impl SavedBy {
    /// [`SavedBy::at`] as a point in time; `None` when it isn't RFC 3339.
    pub fn when(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.at).ok()
    }

    /// [`SavedBy::at`] for a person, in this Mac's time zone ("November 2, 2026 at 14:30"); the
    /// text as written when it isn't RFC 3339.
    pub fn when_text(&self) -> String {
        self.when().map_or_else(|| self.at.clone(), |t| t.with_timezone(&Local).format("%B %-d, %Y at %H:%M").to_string())
    }
}

/// Where this Mac's name and the shared file come from. The default is the real Mac; tests set
/// both so nothing real is touched.
#[derive(Clone, Debug, Default)]
pub struct SyncEnv {
    /// The ArtCraft `sync-folder.json` to use in place of the real one.
    pub shared_file: Option<PathBuf>,
    /// This Mac's name in place of the real one.
    pub computer: Option<String>,
}

impl SyncEnv {
    /// This Mac's name.
    pub fn computer(&self) -> String {
        self.computer.clone().unwrap_or_else(computer_name)
    }

    /// The shared file's path.
    pub fn shared_file(&self) -> Option<PathBuf> {
        self.shared_file.clone().or_else(default_shared_file)
    }

    /// This Mac's name and the time now.
    pub fn saved_by(&self) -> SavedBy {
        SavedBy { computer: self.computer(), at: now_stamp() }
    }
}

/// The Mac's name as macOS shows it in Sharing (System Settings › General › Sharing), else the
/// host name, else "This Mac".
pub fn computer_name() -> String {
    let from = |program: &str, args: &[&str]| {
        let out = std::process::Command::new(program).args(args).output().ok().filter(|o| o.status.success())?;
        clean_name(&String::from_utf8_lossy(&out.stdout))
    };
    (cfg!(target_os = "macos").then(|| from("scutil", &["--get", "ComputerName"])).flatten())
        .or_else(|| ["HOSTNAME", "COMPUTERNAME"].iter().find_map(|k| std::env::var(k).ok().and_then(|v| clean_name(&v))))
        .or_else(|| from("hostname", &[]))
        .unwrap_or_else(|| "This Mac".into())
}

/// `name` without control characters and spaces around, at most [`NAME_MAX`] characters; `None`
/// when nothing is left.
fn clean_name(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The time now as RFC 3339 in the Mac's own offset, to the second.
pub fn now_stamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// `<home>/Library/Application Support/ArtCraft/sync-folder.json`.
pub fn shared_file_in(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join("ArtCraft").join(SHARED_NAME)
}

/// The shared file of this Mac's user (macOS only; `None` elsewhere or without a home folder).
pub fn default_shared_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    cfg!(target_os = "macos").then(|| shared_file_in(Path::new(&home)))
}

// ------------------------------------------------------------------ the file

/// `doc` with `savedBy` set to `by`, as the file's text.
pub fn stamped_text(mut doc: Value, by: &SavedBy) -> String {
    if let Some(m) = doc.as_object_mut() {
        m.insert("savedBy".into(), json!(by));
    }
    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    text.push('\n');
    text
}

/// The `savedBy` of a settings file's text. A missing or unreadable one is `None`: it is a note
/// about the file, not a setting, so it never refuses a file.
pub fn saved_by(text: &str) -> Option<SavedBy> {
    let doc: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let by = doc.get("savedBy")?.as_object()?;
    let computer = clean_name(by.get("computer")?.as_str()?)?;
    let at: String = by.get("at")?.as_str()?.chars().filter(|c| !c.is_control()).take(64).collect();
    Some(SavedBy { computer, at })
}

/// A message for an I/O failure on `path` that tells the user what to do.
fn io_message(path: &Path, e: &std::io::Error, writing: bool) -> String {
    use std::io::ErrorKind;
    let shown = path.display();
    match e.kind() {
        ErrorKind::NotFound => format!("{shown} isn't there. Choose the sync folder again."),
        ErrorKind::PermissionDenied if writing => {
            format!("{shown} can't be written to: it is read-only, or you have no permission. Choose another folder.")
        }
        ErrorKind::PermissionDenied => format!("{shown} can't be read: you have no permission. Choose another folder."),
        ErrorKind::StorageFull => format!("The disk is full, so nothing was saved to {shown}"),
        _ => format!("{shown}: {e}"),
    }
}

/// The folder at `folder`, checked: chosen, there, and a folder.
pub fn check_folder(folder: &str) -> Result<PathBuf, String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Err("No sync folder is chosen. Choose one in Settings ▸ Sync first.".into());
    }
    let path = PathBuf::from(folder);
    match std::fs::metadata(&path) {
        Ok(m) if m.is_dir() => Ok(path),
        Ok(_) => Err(format!("{} is not a folder. Choose the sync folder again.", path.display())),
        Err(e) => Err(io_message(&path, &e, false)),
    }
}

/// The rename that puts a staged file over its target; a test replaces it to fail it on purpose.
pub type RenameFn<'a> = &'a dyn Fn(&Path, &Path) -> std::io::Result<()>;

/// Write `text` to `target` through a hidden temporary file beside it that is renamed over it
/// with `rename`; the temporary file goes when anything fails, and `target` stays as it was.
pub fn write_atomic_with(target: &Path, text: &str, rename: RenameFn) -> Result<(), String> {
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut staged = None;
    for n in 0..1000u32 {
        let tmp = dir.join(format!(".{name}.{}-{n}.tmp", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => {
                staged = Some((tmp, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_message(dir, &e, true)),
        }
    }
    let Some((tmp, mut file)) = staged else { return Err(format!("{}: no free temporary name", dir.display())) };
    let written = file.write_all(text.as_bytes()).and_then(|()| file.sync_all());
    drop(file);
    let done = written.and_then(|()| rename(&tmp, target));
    if let Err(e) = done {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_message(dir, &e, true));
    }
    Ok(())
}

/// [`write_atomic_with`] with the real rename.
pub fn write_atomic(target: &Path, text: &str) -> Result<(), String> {
    write_atomic_with(target, text, &|from, to| std::fs::rename(from, to))
}

/// What a save wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The file.
    pub path: PathBuf,
    /// The `savedBy` written into it.
    pub by: SavedBy,
}

/// Save `doc` (a settings file's JSON object) in `folder` as [`FILE_NAME`], with `by` as `savedBy`,
/// through `rename`.
pub fn save_with(folder: &str, doc: Value, by: SavedBy, rename: RenameFn) -> Result<Saved, String> {
    let path = check_folder(folder)?.join(FILE_NAME);
    write_atomic_with(&path, &stamped_text(doc, &by), rename)?;
    Ok(Saved { path, by })
}

/// [`save_with`] with the real rename.
pub fn save(folder: &str, doc: Value, by: SavedBy) -> Result<Saved, String> {
    save_with(folder, doc, by, &|from, to| std::fs::rename(from, to))
}

/// The file read from the sync folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderFile {
    /// Where it is.
    pub path: PathBuf,
    /// Its text.
    pub text: String,
    /// Who saved it and when, when it says.
    pub by: Option<SavedBy>,
}

/// The text of the file at `path`, at most `max` bytes, as UTF-8.
pub fn read_text(path: &Path, max: usize) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| io_message(path, &e, false))?;
    let mut bytes = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    file.take(limit).read_to_end(&mut bytes).map_err(|e| io_message(path, &e, false))?;
    if bytes.len() > max {
        return Err(format!("{} is too big to be a settings file", path.display()));
    }
    String::from_utf8(bytes).map_err(|_| format!("{} isn't text, so it isn't a settings file", path.display()))
}

/// Read [`FILE_NAME`] from `folder`. An iCloud file that isn't downloaded yet, a missing file and a
/// file that isn't readable each say what to do.
pub fn read(folder: &str) -> Result<FolderFile, String> {
    let dir = check_folder(folder)?;
    let path = dir.join(FILE_NAME);
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // iCloud Drive leaves `.name.icloud` in place of a file it hasn't downloaded.
            return Err(if dir.join(format!(".{FILE_NAME}.icloud")).exists() {
                format!("iCloud hasn't downloaded {FILE_NAME} yet. Open {} in Finder, wait for it, and try again.", dir.display())
            } else {
                format!(
                    "There is no {FILE_NAME} in {} yet. Use Save to Folder on the Mac that has the settings, or choose the folder that holds it.",
                    dir.display()
                )
            });
        }
        Err(e) => return Err(io_message(&path, &e, false)),
    }
    let text = read_text(&path, MAX_BYTES)?;
    let by = saved_by(&text);
    Ok(FolderFile { path, text, by })
}

/// Does the folder have the file, or iCloud's stand-in for it?
pub fn has_file(dir: &Path) -> bool {
    dir.join(FILE_NAME).exists() || dir.join(format!(".{FILE_NAME}.icloud")).exists()
}

// ------------------------------------------------------------------ newer or not

/// Is a file saved by `file` something to offer a Mac called `me` that last saved or loaded the
/// file stamped `last`? Yes when it comes from another Mac and is newer than `last` (any, when
/// `last` is empty or unreadable). A file whose time can't be read is never offered.
pub fn is_newer_from_another_mac(file: &SavedBy, me: &str, last: &str) -> bool {
    if file.computer == me {
        return false;
    }
    match (file.when(), DateTime::parse_from_rfc3339(last).ok()) {
        (Some(file), Some(last)) => file > last,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Keep in Sync at start: the folder's file when it comes from another Mac (not `me`) and is newer
/// than `last`. `Ok(None)` when there is no folder, no file yet, or nothing newer.
pub fn newer_file(folder: &str, me: &str, last: &str) -> Result<Option<FolderFile>, String> {
    if folder.trim().is_empty() {
        return Ok(None);
    }
    let dir = check_folder(folder)?;
    if !has_file(&dir) {
        return Ok(None);
    }
    let file = read(folder)?;
    let newer = file.by.as_ref().is_some_and(|by| is_newer_from_another_mac(by, me, last));
    Ok(newer.then_some(file))
}

// ------------------------------------------------------------------ the shared file

/// The folder `sync-folder.json` names. A missing file, a bad one, a missing or empty `path`, or a
/// path that isn't absolute is `None`, never an error; other keys are ignored.
pub fn read_shared(file: &Path) -> Option<String> {
    let text = read_text(file, SHARED_MAX_BYTES).ok()?;
    let doc: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let path = doc.get("path")?.as_str()?.trim();
    (!path.is_empty() && Path::new(path).is_absolute()).then(|| path.to_string())
}

/// Write `folder` as `path` in the shared file, atomically, keeping any other keys it holds; a
/// damaged file is replaced.
pub fn write_shared(file: &Path, folder: &str) -> Result<(), String> {
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| io_message(dir, &e, true))?;
    }
    let mut doc: Map<String, Value> = read_text(file, SHARED_MAX_BYTES).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    doc.insert("path".into(), json!(folder));
    let mut text = serde_json::to_string_pretty(&Value::Object(doc)).unwrap_or_default();
    text.push('\n');
    write_atomic(file, &text)
}

#[cfg(test)]
#[path = "settings_folder/tests.rs"]
mod tests;
