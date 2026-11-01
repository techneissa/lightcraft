//! Rename Photos and Rename Folder in three steps, so the window keeps drawing while the disk works:
//!
//! 1. [`Session::begin_rename`] (on the session): checks the request and holds the automatic XMP sidecar
//!    writes. It touches no photo file and no catalog record.
//! 2. [`RenameJob::run`] (anywhere, a worker thread): works out the new names (a disk check per photo),
//!    waits for the sidecar writes already queued, and moves the files, all or nothing.
//! 3. [`Session::finish_rename`] (on the session): commits the catalog change as one undo step and lets the
//!    held sidecar writes go, at the paths the files now have. When a move failed, the files that moved are
//!    back (or the catalog follows those that could not go back); when the catalog does not take the change,
//!    the files are moved back. Either way the error says why, and the catalog is as it was.
//!
//! `photo.rename` and `folder.rename` run the same steps in a row, on the calling thread.

use std::path::Path;
use std::sync::Arc;

use lightcraft_catalog::Photo;
use serde_json::{Value, json};

use crate::cmd::browse::{check_folder_target, commit_folder_move, prepare_folder_move, rename_folder_on_disk, rename_folder_target};
use crate::cmd::{bad, manage::rename_args};
use crate::rename::{
    BatchMoveError, MoveFs, RealFs, RenamePlan, move_all, move_all_with_progress, plan_moves, plan_rename_photos_with, rename_photos,
};
use crate::{EngineError, Result, Session};

/// A rename ready for its disk work.
pub struct RenameJob {
    command: &'static str,
    what: What,
    /// Waits for the sidecar writes queued before the files move.
    wait: Option<Arc<dyn Fn() + Send + Sync>>,
}

enum What {
    Photos { photos: Vec<Arc<Photo>>, template: String, start: usize },
    Folder { from: String, to: String },
}

/// How the disk work of a [`RenameJob`] went.
pub struct Moved {
    plans: Vec<RenamePlan>,
    result: std::result::Result<(), Failure>,
}

enum Failure {
    Photos(BatchMoveError),
    Folder(String),
}

impl RenameJob {
    /// The command this job stands for: `photo.rename` or `folder.rename`.
    pub fn command(&self) -> &'static str {
        self.command
    }

    /// How many photos with a file (or folders) the disk work is about: 0 when there is nothing to move.
    pub fn total(&self) -> usize {
        match &self.what {
            What::Photos { photos, .. } => photos.iter().filter(|p| matches!(p.source, lightcraft_catalog::Source::File { .. })).count(),
            What::Folder { .. } => 1,
        }
    }

    /// The disk work, on the real file system. Needs no session: run it on a worker.
    pub fn run(&self) -> Moved {
        self.run_on(&RealFs, &|_, _| {})
    }

    /// [`RenameJob::run`], telling `progress(moved, total)` after each file (`total` 0 until the names are worked out).
    pub fn run_with_progress(&self, progress: &dyn Fn(usize, usize)) -> Moved {
        self.run_on(&RealFs, progress)
    }

    pub(crate) fn run_on(&self, fs: &dyn MoveFs, progress: &dyn Fn(usize, usize)) -> Moved {
        progress(0, 0);
        match &self.what {
            What::Photos { photos, template, start } => {
                let plans = plan_rename_photos_with(photos, template, *start, fs);
                let moves = plan_moves(&plans);
                let total = moves.len();
                progress(0, total);
                if let Some(wait) = &self.wait {
                    wait();
                }
                let result = move_all_with_progress(fs, &moves, &|done| progress(done, total)).map_err(Failure::Photos);
                Moved { plans, result }
            }
            What::Folder { from, to } => {
                progress(0, 1);
                if let Some(wait) = &self.wait {
                    wait();
                }
                let result = rename_folder_on_disk(from, to).map_err(Failure::Folder);
                progress(1, 1);
                Moved { plans: Vec::new(), result }
            }
        }
    }

    /// Whether every file or folder the catalog points at is still where it was: what a worker that died
    /// before it answered may leave unknown.
    pub fn files_in_place(&self) -> bool {
        match &self.what {
            What::Photos { photos, .. } => photos.iter().all(|p| match &p.source {
                lightcraft_catalog::Source::File { path } => Path::new(path).exists(),
                _ => true,
            }),
            What::Folder { from, to } => Path::new(from).is_dir() && (from.eq_ignore_ascii_case(to) || !Path::new(to).exists()),
        }
    }
}

impl Session {
    /// Step 1: `photo.rename {template, start?, ids?}` or `folder.rename {path, name}` as a job.
    pub fn begin_rename(&mut self, command: &str, params: &Value) -> Result<RenameJob> {
        self.begin_rename_with(&RealFs, command, params)
    }

    pub(crate) fn begin_rename_with(&mut self, fs: &dyn MoveFs, command: &str, params: &Value) -> Result<RenameJob> {
        match command {
            "photo.rename" => {
                let (ids, template, start) = rename_args(self, params, "photo.rename")?;
                let photos = rename_photos(&self.catalog, &ids);
                // a retained repair is finished before the files move, as `photo.rename` does
                self.repair_sidecars_with(fs)?;
                let wait = self.hold_sidecars_for_rename();
                Ok(RenameJob { command: "photo.rename", what: What::Photos { photos, template, start }, wait })
            }
            "folder.rename" => {
                let (from, to) = rename_folder_target(params)?;
                // refused here, before anything runs, what the disk would refuse anyway (a name taken)
                check_folder_target(&from, &to).map_err(|e| bad("folder.rename", e))?;
                prepare_folder_move(self, &from).map_err(|e| bad("folder.rename", e))?;
                let wait = self.hold_sidecars_for_rename();
                Ok(RenameJob { command: "folder.rename", what: What::Folder { from, to }, wait })
            }
            other => Err(bad(other, "not a rename that can run in the background")),
        }
    }

    /// Step 3: the result the command gives (`photo.rename` → `{renamed}`, `folder.rename` →
    /// `{path, relinked}`), or why nothing changed.
    pub fn finish_rename(&mut self, job: &RenameJob, moved: Moved) -> Result<Value> {
        self.finish_rename_with(&RealFs, job, moved)
    }

    pub(crate) fn finish_rename_with(&mut self, fs: &dyn MoveFs, job: &RenameJob, moved: Moved) -> Result<Value> {
        let command = job.command;
        let r = self.execute_fn(command, |s| match &job.what {
            What::Photos { .. } => {
                let Moved { plans, result } = moved;
                let result = result.map_err(|f| match f {
                    Failure::Photos(e) => e,
                    Failure::Folder(message) => BatchMoveError { message, stuck: Vec::new(), pending_sidecars: Vec::new() },
                });
                let moved_ok = result.is_ok();
                let prep = match s.prepare_file_plans(fs, &plans) {
                    Ok(prep) => prep,
                    Err(e) => {
                        // the files moved but the catalog can't follow: put them back
                        if moved_ok {
                            let back: Vec<(String, String)> = plan_moves(&plans).into_iter().rev().map(|(a, b)| (b, a)).collect();
                            if let Err(b) = move_all(fs, &back) {
                                return Err(EngineError::Other(format!("{e}; the renamed files could not all be moved back: {}", b.message)));
                            }
                        }
                        return Err(e);
                    }
                };
                let n = s.settle_file_plans(fs, prep, result, &|n| format!("Rename {n} Photo{}", if n == 1 { "" } else { "s" }))?;
                Ok(json!({"renamed": n}))
            }
            What::Folder { from, to } => {
                if let Err(f) = moved.result {
                    return Err(bad(
                        "folder.rename",
                        match f {
                            Failure::Folder(message) => message,
                            Failure::Photos(e) => e.message,
                        },
                    ));
                }
                let n = commit_folder_move(s, from, to).map_err(|e| bad("folder.rename", e))?;
                Ok(json!({"path": to, "relinked": n}))
            }
        });
        self.release_sidecars_after_rename();
        r.map_err(|e| match e {
            EngineError::BadParams { msg, .. } => EngineError::Other(msg),
            other => other,
        })
    }

    /// A job that never reached [`Session::finish_rename`] (its worker died): let the held sidecar writes go.
    pub fn abandon_rename(&mut self) {
        self.release_sidecars_after_rename();
    }
}

#[cfg(test)]
mod tests;
