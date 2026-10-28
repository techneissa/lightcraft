//! Apply macOS Colour to All Photos from This Camera… from the window, off the UI thread: the count before the
//! question and the move after Apply run on a worker ([`lightcraft_engine::macos_colour_job`]) with a row in the
//! activity stack, so a large library doesn't freeze the window. The move lands as one undo step.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::macos_colour_job::Done;
use serde_json::{Value, json};

use crate::LightcraftApp;

/// A count or a move at work.
pub struct MacosJob {
    rx: Receiver<Done>,
    /// The row in the activity stack.
    _guard: TaskGuard,
    /// The photo whose camera it is for, and whether it is the move.
    photo: u64,
    apply: bool,
}

/// Start counting (`apply` false: the question follows) or moving the photos of `photo`'s camera.
pub(crate) fn start(app: &mut LightcraftApp, photo: u64, apply: bool) -> Result<Value, String> {
    if app.macos_job.is_some() {
        return Err(crate::i18n::tr("macOS Colour is still being applied. Try again in a moment.").to_string());
    }
    let p = json!({"photo": photo});
    let job = match apply {
        true => lightcraft_engine::macos_colour_job::apply(&app.session, &p)?,
        false => lightcraft_engine::macos_colour_job::count(&app.session, &p)?,
    };
    let label = if apply { "Applying macOS Colour" } else { "Counting photos" };
    let guard = app.session.activity.start("macosColour", label, Cancel::No);
    guard.progress(0, job.total() as u64);
    let (tx, rx) = channel();
    let work = move || {
        let _ = tx.send(job.run());
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name("macos-colour".into()).spawn(work).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    work();
    app.macos_job = Some(MacosJob { rx, _guard: guard, photo, apply });
    Ok(json!({"background": true}))
}

/// Per frame: when the job is done, ask the question (a count) or say how many photos moved.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(job) = &app.macos_job else { return };
    let done = match job.rx.try_recv() {
        Ok(done) => done,
        Err(TryRecvError::Empty) => return ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        Err(TryRecvError::Disconnected) => {
            app.macos_job = None;
            return app.toast(ctx, crate::i18n::tr("macOS Colour stopped unexpectedly"));
        }
    };
    let (photo, apply) = (job.photo, job.apply);
    // dropping the job ends its row
    app.macos_job = None;
    if !apply {
        match lightcraft_engine::macos_colour_job::finish(&mut app.session, done) {
            Ok(plan) => super::picker::asked(app, ctx, photo, &plan),
            Err(e) => app.toast(ctx, e),
        }
        return;
    }
    // one command around the commit: the durable save and sidecar auto-write, as the command has
    let r = app.session.execute_fn("profiles.macosForCamera", |s| {
        lightcraft_engine::macos_colour_job::finish(s, done).map_err(lightcraft_engine::EngineError::Other)
    });
    match r {
        Ok(r) => {
            let n = r["changed"].as_u64().unwrap_or(0);
            app.toast(ctx, crate::i18n::tr_format!("{n} photos now use macOS Colour", n = n));
        }
        Err(e) => {
            let e = e.to_string();
            let checking = e.contains(lightcraft_engine::cmd::macos_colour::CHECKING);
            app.toast(ctx, if checking { crate::i18n::tr(lightcraft_engine::cmd::macos_colour::CHECKING).to_string() } else { e });
        }
    }
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};

    use crate::LightcraftApp;

    fn app() -> LightcraftApp {
        let mut s = lightcraft_engine::Session::new();
        for i in 1..=3 {
            let mut p = Photo::new(PhotoId(i), Source::File { path: format!("/scratch/{i}.arw") }, "a.arw", "ARW", 7008, 4672, "2026-10-28");
            p.kind = MediaKind::Raw;
            p.meta.camera = "SONY ILCE-7M5".into();
            s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        LightcraftApp::new(s, crate::Services::default())
    }

    fn wait(app: &mut LightcraftApp, ctx: &egui::Context) {
        let t0 = std::time::Instant::now();
        while app.macos_job.is_some() {
            super::poll(app, ctx);
            assert!(t0.elapsed() < std::time::Duration::from_secs(30), "the job ends");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The menu item and Apply return at once with a row in the activity stack: the count and the move run on a
    /// worker, and the frame after says what came of them (the question, or why there is none; a toast after a move).
    #[test]
    fn the_count_and_the_move_leave_the_frame_free() {
        let mut app = app();
        let ctx = egui::Context::default();
        let r = super::start(&mut app, 1, false).unwrap();
        assert_eq!(r["background"], true);
        assert!(app.macos_job.is_some(), "the click doesn't wait for the count");
        assert!(app.session.activity.list().iter().any(|t| t.kind == "macosColour"), "{:?}", app.session.activity.list());
        assert!(super::start(&mut app, 1, true).is_err(), "one at a time");
        wait(&mut app, &ctx);
        assert!(app.ui.dialog.is_some() || app.ui.toast.is_some(), "the question, or why not");
        assert!(!app.session.activity.list().iter().any(|t| t.kind == "macosColour"), "the row goes with the job");
        app.ui.toast = None;
        super::start(&mut app, 1, true).unwrap();
        assert!(app.macos_job.is_some(), "Apply doesn't wait for the move");
        wait(&mut app, &ctx);
        assert!(app.ui.toast.is_some(), "the move says how it went");
    }
}
