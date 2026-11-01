//! Rename Photos and Rename Folder in three steps, so the window keeps drawing while the disk works:
//!
//! 1. [`Session::begin_rename`] (on the session): checks the request, settles the sidecars and works
//!    out the file moves and the catalog change. It touches no photo file and no catalog record.
//! 2. [`RenameJob::run`] (anywhere, a worker thread): moves the files on disk, all or nothing.
//! 3. [`Session::finish_rename`] (on the session): commits the catalog change as one undo step. When a
//!    move failed, the files that moved are back (or the catalog follows those that could not
//!    go back); when the catalog does not take the change, the files are moved back. Either way
//!    the error says why, and the catalog is as it was.
//!
//! `photo.rename` and `folder.rename` run the same steps in a row, on the calling thread.

use serde_json::{Value, json};

use crate::cmd::browse::{commit_folder_move, prepare_folder_move, rename_folder_on_disk, rename_folder_target};
use crate::cmd::{bad, manage::rename_args};
use crate::rename::{BatchMoveError, FilePrep, MoveFs, RealFs, move_all};
use crate::{EngineError, Result, Session};

/// A rename ready for its disk work.
pub struct RenameJob {
    command: &'static str,
    what: What,
}

enum What {
    Photos { prep: FilePrep },
    Folder { from: String, to: String },
}

/// How the disk work of a [`RenameJob`] went.
pub struct Moved(std::result::Result<(), Failure>);

enum Failure {
    Photos(BatchMoveError),
    Folder(String),
}

impl RenameJob {
    /// The command this job stands for: `photo.rename` or `folder.rename`.
    pub fn command(&self) -> &'static str {
        self.command
    }

    /// How many files (photos) or folders the disk work moves.
    pub fn total(&self) -> usize {
        match &self.what {
            What::Photos { prep } => prep.moves.len(),
            What::Folder { .. } => 1,
        }
    }

    /// The disk work, on the real file system. Needs no session: run it on a worker.
    pub fn run(&self) -> Moved {
        self.run_with(&RealFs)
    }

    pub(crate) fn run_with(&self, fs: &dyn MoveFs) -> Moved {
        Moved(match &self.what {
            What::Photos { prep } => move_all(fs, &prep.moves).map_err(Failure::Photos),
            What::Folder { from, to } => rename_folder_on_disk(from, to).map_err(Failure::Folder),
        })
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
                let plans = self.plan_rename_with(fs, &ids, &template, start);
                let prep = self.prepare_file_plans(fs, &plans)?;
                Ok(RenameJob { command: "photo.rename", what: What::Photos { prep } })
            }
            "folder.rename" => {
                let (from, to) = rename_folder_target(params)?;
                prepare_folder_move(self, &from).map_err(|e| bad("folder.rename", e))?;
                Ok(RenameJob { command: "folder.rename", what: What::Folder { from, to } })
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
        let RenameJob { command, what } = job;
        self.execute_fn(command, |s| match (what, moved.0) {
            (What::Photos { prep }, moved) => {
                let prep = prep.clone();
                let moved = moved.map_err(|f| match f {
                    Failure::Photos(e) => e,
                    Failure::Folder(message) => BatchMoveError { message, stuck: Vec::new(), pending_sidecars: Vec::new() },
                });
                let n = s.settle_file_plans(fs, prep, moved, &|n| format!("Rename {n} Photo{}", if n == 1 { "" } else { "s" }))?;
                Ok(json!({"renamed": n}))
            }
            (What::Folder { from, to }, moved) => {
                let (from, to) = (from.as_str(), to.as_str());
                if let Err(f) = moved {
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
        })
        .map_err(|e| match e {
            EngineError::BadParams { msg, .. } => EngineError::Other(msg),
            other => other,
        })
    }
}

#[cfg(test)]
mod tests;
