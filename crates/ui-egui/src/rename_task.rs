//! Rename Photos and Rename Folder from the window: the files move on a worker thread
//! ([`lightcraft_engine::rename_job`]), one row in the activity stack (counting the files as they move), so the
//! window keeps drawing; when they have moved, the catalog follows in one undo step. A failure leaves the catalog
//! as it was and a toast says why (a name taken, no permission, a disk that went away). Only one rename runs at
//! a time, none while an export is running, and no export while one runs. Automatic XMP sidecar writes wait
//! meanwhile and go to the new names afterwards.
//!
//! A command with `background: true` (what the Rename Photos and Rename Folder windows send) starts
//! it; without, `photo.rename` and `folder.rename` run in place as before.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::rename_job::{Moved, RenameJob};
use serde_json::{Value, json};

use crate::LightcraftApp;

/// A rename whose files are moving.
pub struct RenameTask {
    job: Arc<RenameJob>,
    rx: Receiver<Moved>,
    /// Files moved and files to move, as the worker last said.
    progress: Arc<(AtomicUsize, AtomicUsize)>,
    /// The row in the activity stack; dropping the task ends it.
    guard: TaskGuard,
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
    if total == 0 {
        // nothing on disk to move (photos without files): no worker, no row
        let moved = job.run();
        return app.session.finish_rename(&job, moved).map_err(|e| e.to_string());
    }
    let label = if command == "folder.rename" { "Renaming folder" } else { "Renaming photos" };
    let guard = app.session.activity.start("rename", label, Cancel::No);
    guard.progress(0, total as u64);
    let progress = Arc::new((AtomicUsize::new(0), AtomicUsize::new(total)));
    let (tx, rx) = channel();
    let work = {
        let (job, progress) = (job.clone(), progress.clone());
        move || {
            let moved = job.run_with_progress(&|done, of| {
                progress.0.store(done, Ordering::Relaxed);
                progress.1.store(of, Ordering::Relaxed);
            });
            let _ = tx.send(moved);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(e) = std::thread::Builder::new().name("rename".into()).spawn(work) {
        app.session.abandon_rename();
        return Err(e.to_string());
    }
    #[cfg(target_arch = "wasm32")]
    work();
    app.rename_task = Some(RenameTask { job, rx, progress, guard });
    Ok(json!({"background": true, "total": total}))
}

/// Per frame: keep the row's count up to date; when the files have moved, commit the catalog and say how it went.
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
        Err(TryRecvError::Empty) => {
            let (moved, of) = (task.progress.0.load(Ordering::Relaxed), task.progress.1.load(Ordering::Relaxed));
            task.guard.progress(moved as u64, of as u64);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        Err(TryRecvError::Disconnected) => {
            let job = task.job.clone();
            app.rename_task = None;
            app.session.abandon_rename();
            // say "unchanged" only when the disk agrees: the files the library points at are all still there
            if job.files_in_place() {
                app.toast_error(ctx, crate::i18n::tr("The rename stopped unexpectedly; no file moved and the library is unchanged"));
            } else {
                app.toast_error(
                    ctx,
                    crate::i18n::tr("The rename stopped unexpectedly; some files may have moved. Check the folder, or use Find Missing Photos"),
                );
            }
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
