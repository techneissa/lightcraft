//! Photo ▸ Edit In ▸ Open as Layers in PhotoCraft…: the host half. The layer files are rendered on
//! a worker thread ([`lightcraft_engine::layers`]), one row in the activity stack with ✕ to stop;
//! when they are written the `.pcstack` layer list is opened in PhotoCraft (one document, one layer
//! per photo). A PhotoCraft that doesn't know the list ([`crate::Services::layers_support`]: the
//! official one) is given the layer files one by one instead, and the toast says why.
//!
//! `wait: true` does it all in place (agents and tests): the result says what was written and
//! opened.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::layers::{Layers, LayersPlan};
use serde_json::{Value, json};

use crate::LightcraftApp;

/// Layers being rendered in the background.
pub struct LayersTask {
    pub total: usize,
    /// Photos started so far and the file in progress.
    progress: Arc<Mutex<(usize, String)>>,
    pub cancel: Arc<AtomicBool>,
    rx: Receiver<Result<Layers, String>>,
    /// The row in the activity stack (`activity.cancel` sets `cancel`).
    guard: TaskGuard,
    /// The editor the result goes to, and whether it takes the layer list.
    editor: String,
    stacked: bool,
}

impl LayersTask {
    /// `{total, done, current}` for `ui.inspect`.
    pub fn status(&self) -> Value {
        let (done, current) = self.progress.lock().map(|g| g.clone()).unwrap_or_default();
        json!({"total": self.total, "done": done, "current": current})
    }
}

/// The command `photo.openAsLayersInPhotocraft` (`ids?`, `app?`, `wait?`, `format?`, `colorSpace?`,
/// `bitDepth?`, `dir?` for generated demo photos). Every way in says why nothing opened.
pub fn open_as_layers(app: &mut LightcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    start(app, ctx, p).inspect_err(|e| app.toast(ctx, crate::i18n::tr_format!("Couldn't open the layers: {e}", e = e)))
}

fn start(app: &mut LightcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    if app.layers.is_some() {
        return Err("layers are already being prepared".into());
    }
    if app.services.open_with.is_none() {
        return Err("opening another application isn't available here".into());
    }
    let settings = &app.ui.settings;
    let configured = match settings.photocraft_path.trim() {
        "" => crate::state::PHOTOCRAFT.to_string(),
        path => path.to_string(),
    };
    let editor = p.get("app").and_then(Value::as_str).map_or(configured, str::to_string);
    // a missing PhotoCraft leaves no files behind
    if let Some(check) = app.services.check_editor.as_ref() {
        check(&editor)?;
    }
    let stacked = app.services.layers_support.as_ref().is_some_and(|supports| supports(&editor));
    let mut params = settings.photocraft_file.params();
    if let (Some(o), Some(given)) = (params.as_object_mut(), p.as_object()) {
        for (k, v) in given.iter().filter(|(k, _)| !matches!(k.as_str(), "app" | "wait")) {
            o.insert(k.clone(), v.clone());
        }
    }
    let plan = lightcraft_engine::layers::prepare(&mut app.session, &params)?;
    let total = plan.total();
    let write = app.services.write_shared.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new((0usize, String::new())));
    let guard = app.session.activity.start_batch("layers", "Rendering layers", Cancel::Flag(cancel.clone()));
    // the toast below says how it went
    guard.reports_itself();
    guard.progress(0, total as u64);
    let (c, pr) = (cancel.clone(), progress.clone());
    let work = move || run(plan, stacked, write, &c, &pr);
    if p.get("wait").and_then(Value::as_bool) == Some(true) {
        let r = work();
        drop(guard);
        return finish(app, ctx, r, &editor, stacked);
    }
    let (tx, rx) = channel();
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new()
        .name("layers".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    let _ = tx.send(work());
    app.layers = Some(LayersTask { total, progress, cancel, rx, guard, editor, stacked });
    Ok(json!({"background": true, "total": total, "stack": stacked}))
}

/// Render and write the layers (on the worker).
fn run(
    plan: LayersPlan,
    stacked: bool,
    write: Option<crate::services::SharedWrite>,
    cancel: &AtomicBool,
    progress: &Mutex<(usize, String)>,
) -> Result<Layers, String> {
    let mut w = |path: &str, bytes: &[u8]| match &write {
        Some(write) => write(path, bytes),
        None => lightcraft_engine::export::write_file_durable(path, bytes),
    };
    plan.run(stacked, &mut w, &mut |done, name| {
        if let Ok(mut g) = progress.lock() {
            *g = (done, name.to_string());
        }
        !cancel.load(Ordering::Relaxed)
    })
}

/// The layers are written: open them in `editor` (the layer list, or the files one by one), and
/// say how it went. Files the editor refuses are reported in `failed` and a toast, not as an error:
/// the files are written and a retry would make another set.
fn finish(app: &mut LightcraftApp, ctx: &egui::Context, r: Result<Layers, String>, editor: &str, stacked: bool) -> Result<Value, String> {
    let layers = r?;
    let n = layers.files.len();
    let mut failed = None;
    let mut opened = Vec::new();
    {
        let open = app.services.open_with.as_mut().ok_or("opening another application isn't available here")?;
        let handed: Vec<Result<String, String>> = match (&layers.stack, stacked) {
            (Some(stack), true) => vec![lightcraft_engine::open_path::openable_stack(stack)],
            _ => layers.files.iter().map(|f| lightcraft_engine::open_path::openable(f, "")).collect(),
        };
        for path in handed {
            match path {
                Err(e) => failed = Some(e),
                Ok(path) => match open(&path, editor) {
                    Ok(()) => opened.push(path),
                    Err(e) => failed = Some(e),
                },
            }
        }
    }
    let result = json!({
        "name": layers.name, "dir": layers.dir, "files": layers.files, "stack": layers.stack,
        "opened": opened, "oneByOne": !stacked, "failed": failed,
    });
    match (&failed, stacked) {
        (Some(e), _) => app.toast(ctx, crate::i18n::tr_format!("Couldn't open the editor: {e}", e = e)),
        (None, true) => app.toast(ctx, crate::i18n::tr_format!("{name} opened in PhotoCraft with {n} layers", name = layers.name, n = n)),
        (None, false) => {
            app.toast_for(ctx, crate::i18n::tr_format!("This PhotoCraft can't open photos as layers; the {n} photos opened one by one", n = n), 8.0)
        }
    }
    Ok(result)
}

/// Per frame: keep the activity row up to date; when the layers are written, open them.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(task) = &app.layers else { return };
    match task.rx.try_recv() {
        Ok(r) => {
            let (editor, stacked) = (task.editor.clone(), task.stacked);
            // dropping the task ends its row
            app.layers = None;
            if let Err(e) = finish(app, ctx, r, &editor, stacked) {
                match e.as_str() {
                    lightcraft_engine::layers::CANCELLED => app.toast(ctx, crate::i18n::tr("Open as Layers cancelled")),
                    _ => app.toast(ctx, crate::i18n::tr_format!("Couldn't open the layers: {e}", e = e)),
                }
            }
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            let (done, current) = task.progress.lock().map(|g| g.clone()).unwrap_or_default();
            task.guard.progress(done as u64, task.total as u64);
            task.guard.detail(&current);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            app.layers = None;
            app.toast(ctx, crate::i18n::tr("Open as Layers stopped unexpectedly"));
        }
    }
}

#[cfg(test)]
mod tests;
