//! File ▸ Export Animated GIF…: the dialog's choices go to `app.animatedGif`, which asks where to
//! write the .gif and starts the engine's `export.animatedGif` on the selected photos in the
//! background. The export's row in the activity stack shows its progress and ✕ (the engine owns
//! the row); [`poll`] tells the user how it ended.

use serde_json::{Value, json};

use super::frame::{choices, field};
use super::*;
use crate::LightcraftApp;
use crate::pick::{PickRequest, Picked};

/// `app.animatedGif {size?, seconds?, loop?, dither?, path?}`: without a path, a save dialog asks
/// for one first.
pub fn start(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    if app.session.animated_gif.as_ref().is_some_and(|r| r.running()) {
        return Err("an animated GIF export is already running".into());
    }
    let mut params = p.clone();
    if p.get("path").and_then(Value::as_str).is_none_or(|x| x.trim().is_empty()) {
        let req = PickRequest::save(crate::i18n::tr("Export Animated GIF"), "GIF", &["gif"], "Animation.gif");
        let paths = match crate::pick::ask(app, id, p, "path", req, |_| None) {
            Picked::Now(paths) => paths,
            Picked::Later => return Ok(Value::Null),
            Picked::Unavailable => return Err("no save dialog on this platform; supply path".into()),
        };
        let Some(path) = paths.into_iter().next() else { return Ok(Value::Null) };
        params["path"] = json!(path);
    }
    let r = app.run("export.animatedGif", params)?;
    app.animated_gif_reported = false;
    Ok(r)
}

/// Each frame: when the export has ended, say how (once).
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    if app.animated_gif_reported {
        return;
    }
    let Some(run) = app.session.animated_gif.clone() else { return };
    if run.running() {
        return;
    }
    app.animated_gif_reported = true;
    let msg = match run.result() {
        Some(Ok(r)) => {
            let mut m = crate::i18n::tr_format!(
                "Animated GIF exported: {n} frames, {mb} MB",
                n = r["frames"].as_u64().unwrap_or(0),
                mb = format!("{:.1}", r["bytes"].as_f64().unwrap_or(0.0) / 1e6)
            );
            if let Some(n) = r["skipped"].as_u64().filter(|n| *n > 0) {
                m += &crate::i18n::tr_format!(" · {n} skipped (could not be rendered)", n = n);
            }
            m
        }
        Some(Err(e)) => e,
        None => crate::i18n::tr("Export stopped unexpectedly").to_string(),
    };
    app.toast(ctx, msg);
}

/// File ▸ Export Animated GIF…: how big, how fast, looping and dithering; where comes after (a
/// save dialog).
pub(super) fn animated_gif_body(app: &mut LightcraftApp, ui: &mut egui::Ui, settings: &mut lightcraft_engine::animated_gif::GifSettings) {
    use lightcraft_engine::animated_gif::SECONDS_MAX;
    let t = Tokens::get(ui.ctx());
    let photos = app.session.selected().len();
    let seconds = photos as f64 * f64::from(settings.delay()) / 100.0;
    let summary = crate::i18n::tr_format!(
        "{n} photos selected · {time}",
        n = photos,
        time = if seconds < 60.0 {
            format!("{} s", (seconds * 10.0).round() / 10.0)
        } else {
            format!("{}:{:02}", (seconds / 60.0).floor() as u64, (seconds % 60.0).round() as u64)
        }
    );
    ui.label(egui::RichText::new(summary).color(t.text_dim));
    choices(ui, "Size", "animatedGifSize", &[(480, "480 px"), (800, "800 px"), (1200, "1200 px")], &mut settings.size);
    field(ui, "Each photo", |ui| {
        let r = ui.add(egui::Slider::new(&mut settings.seconds, 0.1..=SECONDS_MAX.min(10.0)).logarithmic(true).suffix(" s").max_decimals(2));
        crate::widgets::register(ui, "slider:animatedGifSeconds", r.rect);
    });
    let r = ui.checkbox(&mut settings.loop_forever, crate::i18n::tr("Loop forever"));
    crate::widgets::register(ui, "check:animatedGifLoop", r.rect);
    let r = ui.checkbox(&mut settings.dither, crate::i18n::tr("Smooth gradients (dithering)"));
    crate::widgets::register(ui, "check:animatedGifDither", r.rect);
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "One frame per selected photo, in grid order, with its edits; raw files are rendered directly. Photos of another shape sit on black.",
        ))
        .color(t.text_dim),
    );
    ui.label(
        egui::RichText::new(crate::i18n::tr("A GIF has 256 colours per frame and gets big fast: keep the size small for sharing.")).color(t.text_dim),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pick::PickKind;
    use crate::state::Dialog;
    use crate::tests_fixtures::SyntheticLibrary;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// The app, with three real file-backed photos, all selected, and a picker that answers
    /// `answer` and remembers what it was asked. Keep the returned fixture alive until the app is
    /// dropped.
    fn app_answering(tag: &str, answer: String) -> (SyntheticLibrary, LightcraftApp, Arc<Mutex<Vec<PickRequest>>>) {
        let files = SyntheticLibrary::with_photos(tag, 3);
        let asked = Arc::new(Mutex::new(Vec::<PickRequest>::new()));
        let requests = asked.clone();
        let services = crate::Services {
            picker: Some(Box::new(move |req| {
                requests.lock().unwrap().push(req);
                let (tx, rx) = std::sync::mpsc::channel();
                tx.send(vec![answer.clone()]).unwrap();
                Ok(rx)
            })),
            ..Default::default()
        };
        let mut app = files.app();
        app.services = services;
        let ids: Vec<u64> = app.session.visible_cloned().into_iter().map(|id| id.0).collect();
        app.session.execute("library.select", &json!({"ids": ids})).unwrap();
        (files, app, asked)
    }

    #[test]
    fn menu_dialog_picker_and_background_export() {
        let dir = std::env::temp_dir().join(format!("lc-ui-gif-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Anim.gif").to_string_lossy().to_string();
        let (_files, mut app, asked) = app_answering("gif", path.clone());
        assert!(crate::menus::ui_enabled(&app, "dialog.animatedGif"));
        app.run("dialog.animatedGif", json!({})).unwrap();
        let Some(Dialog::AnimatedGif { settings }) = app.ui.dialog.as_mut() else { panic!("no animated GIF dialog") };
        assert_eq!((settings.size, settings.seconds, settings.loop_forever, settings.dither), (800, 1.0, true, true));
        settings.size = 96;
        settings.seconds = 0.25;
        let mut h = crate::headless::Headless::new(app, [1000.0, 800.0], 1.0);
        let dialog = h.app.ui.dialog.take().unwrap();
        crate::panels::dialogs::confirm_dialog(&mut h.app, &dialog).unwrap();
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            h.step();
            if h.app.animated_gif_reported && h.app.session.animated_gif.is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "export did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        let r = h.app.session.animated_gif.clone().unwrap().result().unwrap().unwrap();
        let req = asked.lock().unwrap();
        assert_eq!(req[0].kind, PickKind::Save);
        assert_eq!(req[0].filter.as_ref().unwrap().1, &["gif"]);
        assert_eq!((r["frames"].as_u64(), r["seconds"].as_f64()), (Some(3), Some(0.75)), "{r}");
        assert_eq!(r["width"].as_u64().unwrap().max(r["height"].as_u64().unwrap()), 96);
        assert!(std::fs::read(&path).unwrap().starts_with(b"GIF89a"));
        let toast = h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
        assert!(toast.contains("Animated GIF exported: 3 frames"), "{toast}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_without_a_selection() {
        let (_files, mut app, _) = app_answering("gif-none", String::new());
        app.session.execute("library.select", &json!({"ids": []})).unwrap();
        app.session.selection.active = None;
        assert!(!crate::menus::ui_enabled(&app, "dialog.animatedGif"));
        assert!(!crate::menus::ui_enabled(&app, "app.animatedGif"));
    }
}
