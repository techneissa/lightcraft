//! Handing files to an external editor without holding up the frame: on macOS each `open -a`
//! waits until LaunchServices has the file (a second or more while the editor starts). With
//! [`crate::Services::open_with_shared`] the files go to the editor on a worker thread, and the
//! next frame after it is done says how it went (the same toasts as before).

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use crate::LightcraftApp;

/// Files on their way to an editor.
pub struct Handoff {
    /// The first error the editor gave, once every file was handed over.
    rx: Receiver<Option<String>>,
    /// The toast when they all opened, and how long it stays.
    done: String,
    secs: f64,
}

/// Whether [`start`] can hand files over in the background here.
pub fn available(app: &LightcraftApp) -> bool {
    app.services.open_with_shared.is_some()
}

/// Hand `paths` to `editor` on a worker; `done` is the toast when they all opened (for `secs`).
/// False when this host has no thread-safe opener or the worker can't start (the caller opens
/// them itself).
pub fn start(app: &mut LightcraftApp, paths: Vec<String>, editor: &str, done: String, secs: f64) -> bool {
    let Some(open) = app.services.open_with_shared.clone() else { return false };
    let (tx, rx) = channel();
    let editor = editor.to_string();
    let work = move || {
        let mut failed = None;
        for path in &paths {
            if let Err(e) = open(path, &editor) {
                failed = Some(e);
            }
        }
        let _ = tx.send(failed);
    };
    if std::thread::Builder::new().name("editor-handoff".into()).spawn(work).is_err() {
        return false;
    }
    app.handoffs.push(Handoff { rx, done, secs });
    true
}

/// Per frame: say how each finished handoff went.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    if app.handoffs.is_empty() {
        return;
    }
    let mut toasts = Vec::new();
    app.handoffs.retain(|h| match h.rx.try_recv() {
        Ok(None) => {
            toasts.push((h.done.clone(), h.secs));
            false
        }
        Ok(Some(e)) => {
            toasts.push((crate::i18n::tr_format!("Couldn't open the editor: {e}", e = e), crate::TOAST_SECONDS));
            false
        }
        Err(TryRecvError::Empty) => true,
        Err(TryRecvError::Disconnected) => {
            toasts.push((crate::i18n::tr("Couldn't open the editor: it stopped unexpectedly").to_string(), crate::TOAST_SECONDS));
            false
        }
    });
    for (text, secs) in toasts {
        app.toast_for(ctx, text, secs);
    }
    if !app.handoffs.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::{LightcraftApp, Services};

    /// A slow editor start (an `open -a` that waits for LaunchServices) doesn't hold the frame:
    /// `start` returns at once, and the toast comes once the files are handed over, the error
    /// when the editor refused one.
    #[test]
    fn an_editor_handoff_runs_off_the_frame() {
        let release = Arc::new(AtomicBool::new(false));
        let opened: Arc<Mutex<Vec<String>>> = Default::default();
        let (rel, o) = (release.clone(), opened.clone());
        let services = Services {
            png: None,
            open_with_shared: Some(Arc::new(move |path: &str, _: &str| {
                while !rel.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                o.lock().unwrap().push(path.to_string());
                if path.ends_with("bad.tif") { Err("refused".into()) } else { Ok(()) }
            })),
            ..Default::default()
        };
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
        let ctx = egui::Context::default();
        assert!(super::start(&mut app, vec!["/tmp/a.tif".into(), "/tmp/b.tif".into()], "Editor", "opened".into(), 2.0));
        super::poll(&mut app, &ctx);
        assert!(opened.lock().unwrap().is_empty() && app.ui.toast.is_none(), "still waiting on the editor");
        release.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !app.handoffs.is_empty() {
            super::poll(&mut app, &ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(opened.lock().unwrap().len(), 2);
        assert_eq!(app.ui.toast.as_ref().map(|t| t.0.clone()).as_deref(), Some("opened"));
        assert!(super::start(&mut app, vec!["/tmp/bad.tif".into()], "Editor", "opened".into(), 2.0));
        while !app.handoffs.is_empty() {
            super::poll(&mut app, &ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        let toast = app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
        assert!(toast.contains("refused"), "{toast}");
    }
}
