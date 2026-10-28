//! Photo ▸ Edit In from the window: the copies are rendered on a worker thread
//! ([`lightcraft_engine::edit_copies`]), one row in the activity stack with ✕ to stop, so the
//! window keeps drawing; when they are written they join the library, stacked on their originals,
//! and open in the editor. Needs [`crate::Services::write_shared`] (a thread-safe writer); without
//! it, or with `wait: true`, Edit In runs `photo.editExternal` in place as before.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::edit_copies::{CANCELLED, EditPlan, Rendered};
use serde_json::{Value, json};

use crate::LightcraftApp;

/// Edit In copies being rendered in the background.
pub struct EditInTask {
    pub total: usize,
    /// Photos started so far and the file in progress.
    progress: Arc<Mutex<(usize, String)>>,
    pub cancel: Arc<AtomicBool>,
    rx: Receiver<Rendered>,
    /// The row in the activity stack (`activity.cancel` sets `cancel`).
    guard: TaskGuard,
    /// The editor the copies go to.
    editor: String,
}

/// Start rendering `plan` in the background for `editor`.
pub fn start(app: &mut LightcraftApp, plan: EditPlan, editor: String) -> Result<Value, String> {
    if app.edit_in.is_some() {
        return Err("Edit In is already preparing copies".into());
    }
    let write = app.services.write_shared.clone().ok_or("no background writer")?;
    let total = plan.total();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new((0usize, String::new())));
    let guard = app.session.activity.start_batch("editIn", "Preparing copies to edit", Cancel::Flag(cancel.clone()));
    // the toast when the editor opens says how it went
    guard.reports_itself();
    guard.progress(0, total as u64);
    let (c, pr) = (cancel.clone(), progress.clone());
    let (tx, rx) = channel();
    let work = move || {
        let r = plan.run(&mut |path, bytes| write(path, bytes), &mut |done, name| {
            if let Ok(mut g) = pr.lock() {
                *g = (done, name.to_string());
            }
            !c.load(Ordering::Relaxed)
        });
        let _ = tx.send(r);
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name("edit-in".into()).spawn(work).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    work();
    app.edit_in = Some(EditInTask { total, progress, cancel, rx, guard, editor });
    Ok(json!({"background": true, "total": total}))
}

/// Per frame: keep the activity row up to date; when the copies are written, add them to the
/// library and open them.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(task) = &app.edit_in else { return };
    match task.rx.try_recv() {
        Ok(r) => {
            let editor = task.editor.clone();
            // dropping the task ends its row
            app.edit_in = None;
            finish(app, ctx, r, &editor);
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            let (done, current) = task.progress.lock().map(|g| g.clone()).unwrap_or_default();
            task.guard.progress(done as u64, task.total as u64);
            task.guard.detail(&current);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            app.edit_in = None;
            app.toast(ctx, crate::i18n::tr("Edit In stopped unexpectedly"));
        }
    }
}

fn finish(app: &mut LightcraftApp, ctx: &egui::Context, r: Rendered, editor: &str) {
    let cancelled = r.error.as_deref() == Some(CANCELLED);
    if cancelled {
        // a stop means no copies: remove the ones already written (none is in the library yet)
        for (_, path) in &r.copies {
            let _ = std::fs::remove_file(path);
        }
        app.toast(ctx, crate::i18n::tr("Edit In cancelled"));
        return;
    }
    // one command around the imports: one undo step, the durable save, sidecar auto-write
    let added = app
        .session
        .execute_fn("photo.editExternal", |s| lightcraft_engine::edit_copies::finish(s, r).map_err(lightcraft_engine::EngineError::Other))
        .map_err(|e| e.to_string());
    match added {
        Ok(r) => {
            let _ = crate::menus::files::open_copies(app, ctx, r, editor);
        }
        Err(e) => app.toast(ctx, crate::i18n::tr_format!("Couldn't open the editor: {e}", e = e)),
    }
}

#[cfg(test)]
mod tests;
