//! Rename Photos and Rename Folder from the window: the files move on a worker thread
//! ([`lightcraft_engine::rename_job`]), one row in the activity stack, so the window keeps drawing;
//! when they have moved, the catalog follows in one undo step. A failure leaves the catalog as it was
//! and a toast says why (a name taken, no permission, a disk that went away). Only one rename runs at
//! a time, and none while an export is reading the files.
//!
//! A command with `background: true` (what the Rename Photos and Rename Folder windows send) starts
//! it; without, `photo.rename` and `folder.rename` run in place as before.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::rename_job::{Moved, RenameJob};
use serde_json::{Value, json};

use crate::LightcraftApp;

/// A rename whose files are moving.
pub struct RenameTask {
    job: Arc<RenameJob>,
    rx: Receiver<Moved>,
    /// The row in the activity stack; dropping the task ends it.
    _guard: TaskGuard,
}

/// Start `command` (`photo.rename` or `folder.rename`) with `params` in the background. Returns
/// `{background: true, total}` at once; the end is a toast.
pub fn start(app: &mut LightcraftApp, command: &str, params: &Value) -> Result<Value, String> {
    if app.rename_task.is_some() {
        return Err(crate::i18n::tr("Another rename is still running").to_string());
    }
    if app.session.activity.list().iter().any(|t| t.kind == "export") {
        return Err(crate::i18n::tr("Wait for the export to finish: it is reading the files").to_string());
    }
    let job = Arc::new(app.session.begin_rename(command, params).map_err(|e| e.to_string())?);
    let total = job.total();
    let label = if command == "folder.rename" { "Renaming folder" } else { "Renaming photos" };
    let guard = app.session.activity.start("rename", label, Cancel::No);
    guard.progress(0, total as u64);
    let (tx, rx) = channel();
    let work = {
        let job = job.clone();
        move || {
            let _ = tx.send(job.run());
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name("rename".into()).spawn(work).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    work();
    app.rename_task = Some(RenameTask { job, rx, _guard: guard });
    Ok(json!({"background": true, "total": total}))
}

/// Per frame: when the files have moved, commit the catalog and say how it went.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(task) = &app.rename_task else { return };
    match task.rx.try_recv() {
        Ok(moved) => {
            let job = task.job.clone();
            // dropping the task ends its row
            app.rename_task = None;
            let result = app.session.finish_rename(&job, moved).map_err(|e| e.to_string());
            done(app, ctx, job.command(), result);
        }
        Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        Err(TryRecvError::Disconnected) => {
            app.rename_task = None;
            app.toast(ctx, crate::i18n::tr("The rename stopped unexpectedly; the library is unchanged"));
        }
    }
}

fn done(app: &mut LightcraftApp, ctx: &egui::Context, command: &str, result: Result<Value, String>) {
    match result {
        Ok(_) if command == "folder.rename" => app.toast(ctx, crate::i18n::tr("Folder renamed")),
        Ok(r) => {
            let n = r.get("renamed").and_then(Value::as_u64).unwrap_or(0);
            app.toast(ctx, crate::i18n::tr_format!("Renamed {n} photo{}", if n == 1 { "" } else { "s" }, n = n));
        }
        Err(why) => app.toast_error(ctx, crate::i18n::tr_format!("Not renamed: {why}", why = why)),
    }
}
