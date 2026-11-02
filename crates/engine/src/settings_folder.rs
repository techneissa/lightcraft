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
    /// A random id made once per Mac (LightCraft only; PhotoCraft's files have none and readers
    /// ignore keys they don't know). Two Macs with the same name are told apart by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
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

    /// This Mac's name and the time now, and `id`, the id of this Mac (empty: none).
    pub fn saved_by(&self, id: &str) -> SavedBy {
        SavedBy { computer: self.computer(), at: now_stamp(), id: Some(id.to_string()).filter(|i| !i.is_empty()) }
    }

    /// This Mac as [`inspect`] needs it; `id` is the Mac's own id (empty: none made yet).
    pub fn me(&self, id: &str) -> Me {
        Me { computer: self.computer(), id: id.to_string() }
    }
}

/// This Mac: its name and its own id (empty before one is made).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    /// The Mac's name.
    pub computer: String,
    /// The Mac's own id.
    pub id: String,
}

/// A random id for a Mac: 128 bits from the system's random seeds, as hex.
pub fn new_mac_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let word = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", word(), word())
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
    let id =
        by.get("id").and_then(Value::as_str).map(|i| i.chars().filter(|c| !c.is_control()).take(64).collect::<String>()).filter(|i| !i.is_empty());
    Some(SavedBy { computer, at, id })
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

/// How the file in the folder stands to this Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// This Mac saved it.
    Own,
    /// Another Mac saved it, before this Mac last saved or loaded.
    Older,
    /// Another Mac saved it after this Mac last saved or loaded (any, when this Mac has neither).
    Newer,
    /// The saver or the time can't be read: not told apart from a newer one by anything that
    /// would overwrite it.
    Unknown,
}

impl Standing {
    /// Saving over the file would replace something this Mac hasn't seen (or can't tell).
    pub fn would_lose_settings(self) -> bool {
        matches!(self, Standing::Newer | Standing::Unknown)
    }
}

/// Did `me` save the file stamped `file`? By id when the file has one and this Mac has one, else
/// by the computer's name.
pub fn is_same_mac(file: &SavedBy, me: &Me) -> bool {
    match &file.id {
        Some(id) if !me.id.is_empty() => *id == me.id,
        _ => file.computer == me.computer,
    }
}

/// How a file saved by `file` (`None`: it doesn't say, or not readably) stands to `me`, who last
/// saved or loaded the file stamped `last`. A file that is not `me`'s own and whose time can't be
/// read is [`Standing::Unknown`], like one whose saver can't be read.
pub fn standing(file: Option<&SavedBy>, me: &Me, last: &str) -> Standing {
    let Some(file) = file else { return Standing::Unknown };
    if is_same_mac(file, me) {
        return Standing::Own;
    }
    match (file.when(), DateTime::parse_from_rfc3339(last).ok()) {
        (Some(file), Some(last)) if file <= last => Standing::Older,
        (Some(_), _) => Standing::Newer,
        (None, _) => Standing::Unknown,
    }
}

/// The folder's file and how it stands to `me`; `Ok(None)` when no folder is chosen or it has no
/// file yet. An error when the folder or the file can't be read: nothing may be saved over it then.
pub fn inspect(folder: &str, me: &Me, last: &str) -> Result<Option<(FolderFile, Standing)>, String> {
    if folder.trim().is_empty() {
        return Ok(None);
    }
    let dir = check_folder(folder)?;
    if !has_file(&dir) {
        return Ok(None);
    }
    let file = read(folder)?;
    let stand = standing(file.by.as_ref(), me, last);
    Ok(Some((file, stand)))
}

/// Keep in Sync at start: the folder's file when it is [`Standing::Newer`] (a file that doesn't say
/// who saved it is never offered, or it would be offered at every start).
pub fn newer_file(folder: &str, me: &Me, last: &str) -> Result<Option<FolderFile>, String> {
    Ok(inspect(folder, me, last)?.filter(|(_, s)| *s == Standing::Newer).map(|(f, _)| f))
}

/// Remove temporary files an interrupted save of this app left in `dir`: only names of the form
/// `.lightcraft.lcsettings.<pid>-<n>.tmp`, and only when a minute old (another Mac may be saving
/// right now). Returns how many went.
pub fn remove_leftovers(dir: &Path) -> usize {
    let prefix = format!(".{FILE_NAME}.");
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(middle) = name.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".tmp")) else { continue };
        let ours = middle
            .split_once('-')
            .is_some_and(|(pid, n)| !pid.is_empty() && !n.is_empty() && pid.bytes().chain(n.bytes()).all(|b| b.is_ascii_digit()));
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= std::time::Duration::from_secs(60));
        if ours && old && entry.file_type().is_ok_and(|t| t.is_file()) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
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
