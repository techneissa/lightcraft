//! Delete Permanently with the files: which files of the photos going leave the disk, and how they
//! reach the system Trash.
//!
//! `photo.deletePermanently` and `library.emptyRecentlyDeleted` take `files: "keep" | "trash"`
//! (default `keep`, so an agent never removes a file by accident). `trash` moves each photo's file
//! and its XMP sidecars to the Trash, so they can be got back from Finder. `keep` still moves the
//! files of Local photos (browsed from a folder, never added to the library): they have no library
//! to be removed from, and the folder would show the file again. A file that a photo
//! staying in the library still uses is never moved: a virtual copy shares its master's file, and
//! the two halves of a raw + JPEG pair may share the stem sidecar (`IMG_0001.xmp`).
//!
//! The Trash is reached through [`Trasher`], so tests use a fake and never touch the real Trash.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::{Catalog, Photo, PhotoId, Source};
use serde_json::{Value, json};

use crate::sidecar::{SidecarNaming, sidecar_candidates};
use crate::{Result, Session};

/// Moves one file to the system Trash.
pub trait Trasher: Send + Sync {
    /// Move `path` to the Trash; the error is a sentence for the person.
    fn trash(&self, path: &Path) -> std::result::Result<(), String>;
}

/// The real Trash (the `trash` crate: `NSFileManager` on macOS, the Recycle Bin on Windows, the
/// freedesktop trash elsewhere). In the browser there is none.
pub struct SystemTrash;

impl Trasher for SystemTrash {
    #[cfg(target_arch = "wasm32")]
    fn trash(&self, _path: &Path) -> std::result::Result<(), String> {
        Err("moving files to the Trash isn't available in the browser".to_string())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn trash(&self, path: &Path) -> std::result::Result<(), String> {
        let ctx = ::trash::TrashContext::default();
        // (the Finder method asks for Automation permission and plays the Finder's sound)
        #[cfg(target_os = "macos")]
        {
            use ::trash::macos::{DeleteMethod, TrashContextExtMacos};
            let mut ctx = ctx;
            ctx.set_delete_method(DeleteMethod::NsFileManager);
            ctx.delete(path).map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "macos"))]
        ctx.delete(path).map_err(|e| e.to_string())
    }
}

/// What `files` asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesChoice {
    Keep,
    Trash,
}

impl FilesChoice {
    /// `files` in a command's params: absent or `null` is `keep`.
    pub fn from_params(p: &Value, command: &str) -> Result<Self> {
        match p.get("files") {
            None | Some(Value::Null) => Ok(Self::Keep),
            Some(Value::String(s)) if s == "keep" => Ok(Self::Keep),
            Some(Value::String(s)) if s == "trash" => Ok(Self::Trash),
            Some(_) => Err(crate::cmd::bad(command, "`files` must be \"keep\" or \"trash\"")),
        }
    }
}

fn path_of(p: &Photo) -> Option<&str> {
    match &p.source {
        Source::File { path } => Some(path),
        Source::Demo { .. } => None,
    }
}

fn lower(p: impl AsRef<Path>) -> String {
    p.as_ref().to_string_lossy().to_lowercase()
}

/// A path without its extension, in lower case: files that can share a sidecar share it.
fn base_key(path: &str) -> String {
    lower(Path::new(path).with_extension(""))
}

/// The files to move for photos `ids`, each with the photos it belongs to, in order. A file (or
/// sidecar) a photo outside `ids` still uses is left out; so is one that is not there.
fn plan(cat: &Catalog, ids: &[PhotoId]) -> Vec<(PathBuf, Vec<PhotoId>)> {
    let gone: HashSet<PhotoId> = ids.iter().copied().collect();
    let bases: HashSet<String> = ids.iter().filter_map(|id| cat.photo(*id)).filter_map(|p| path_of(p)).map(base_key).collect();
    // the files and sidecar names of the photos staying (only those that could clash)
    let mut kept: HashSet<String> = HashSet::new();
    for p in cat.photos().filter(|p| !gone.contains(&p.id)) {
        let Some(path) = path_of(p) else { continue };
        if !bases.contains(&base_key(path)) {
            continue;
        }
        kept.insert(lower(path));
        kept.extend(sidecar_candidates(path, SidecarNaming::Stem).map(lower));
    }
    let mut out: Vec<(PathBuf, Vec<PhotoId>)> = Vec::new();
    let mut at: HashMap<PathBuf, usize> = HashMap::new();
    for id in ids {
        let Some(path) = cat.photo(*id).and_then(|p| path_of(p)) else { continue };
        let files = std::iter::once(PathBuf::from(path)).chain(sidecar_candidates(path, SidecarNaming::Stem));
        for f in files {
            if kept.contains(&lower(&f)) || std::fs::symlink_metadata(&f).is_err() {
                continue;
            }
            match at.get(&f) {
                Some(i) => {
                    if let Some((_, owners)) = out.get_mut(*i) {
                        owners.push(*id);
                    }
                }
                None => {
                    at.insert(f.clone(), out.len());
                    out.push((f, vec![*id]));
                }
            }
        }
    }
    out
}

/// What the Trash did, for the result of the command and the toast.
fn note(trashed: usize) -> String {
    if trashed == 1 {
        "Moved 1 file to the Trash. Undo brings back the photo, not the file: get the file back from the Trash.".to_string()
    } else {
        format!("Moved {trashed} files to the Trash. Undo brings back the photos, not the files: get the files back from the Trash.")
    }
}

impl Session {
    /// Use `trasher` for the Trash (tests pass a fake).
    pub fn set_trasher(&mut self, trasher: Arc<dyn Trasher>) {
        self.trasher = trasher;
    }

    /// Remove photos `ids` from the library for good (one undo step), and with
    /// [`FilesChoice::Trash`] move their files to the Trash first; Local photos' files always go
    /// (see the module docs). A photo whose file could not be
    /// moved stays in the library. `{deleted, trashed: [path], failed: [[path, error]], note?}`.
    pub(crate) fn delete_permanently_with(&mut self, ids: &[PhotoId], files: FilesChoice, label: &str) -> Result<Value> {
        let mut trashed: Vec<String> = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        let mut blocked: HashSet<PhotoId> = HashSet::new();
        // `trash`: every photo's files; `keep`: only the Local photos' (a library photo's files stay, so they count
        // as files of photos staying when `plan` looks for a shared one)
        let to_trash: Vec<PhotoId> = match files {
            FilesChoice::Trash => ids.to_vec(),
            FilesChoice::Keep => ids.iter().copied().filter(|id| self.catalog.photo(*id).is_some_and(|p| p.local)).collect(),
        };
        if !to_trash.is_empty() {
            for (file, owners) in plan(&self.catalog, &to_trash) {
                // (a name spelt another way for the same file, such as `.XMP` for `.xmp` on a
                // case-insensitive disk, went with the first)
                let meta = match std::fs::symlink_metadata(&file) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let outcome = if !file.is_absolute() {
                    Err("the path is not absolute".to_string())
                } else if meta.is_dir() {
                    Err("it is a folder, not a file".to_string())
                } else {
                    self.trasher.trash(&file)
                };
                match outcome {
                    Ok(()) => trashed.push(file.to_string_lossy().into_owned()),
                    Err(e) => {
                        failed.push((file.to_string_lossy().into_owned(), e));
                        blocked.extend(owners);
                    }
                }
            }
        }
        let remove: Vec<PhotoId> = ids.iter().copied().filter(|id| !blocked.contains(id)).collect();
        if !remove.is_empty() {
            let op = self.catalog.delete_photos_permanently_ops(&remove);
            self.commit(label, op)?;
        }
        let mut r = json!({"deleted": remove.len(), "trashed": trashed, "failed": failed});
        if !trashed.is_empty() {
            r["note"] = json!(note(trashed.len()));
        }
        Ok(r)
    }
}
