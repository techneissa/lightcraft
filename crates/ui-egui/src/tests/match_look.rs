//! Fork feature (`fork/match-look.md`): Photo ▸ Match Look… picks a reference, shows it small, and a click on Match
//! gives the selected photo the reference's look in the background, one undo step.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().rev().find(|(w, _)| w == id).map(|(_, r)| *r)
}

#[test]
fn match_look_window_matches_the_selected_photo_to_a_library_photo() {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    app.ui.settings.gpu = false;
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let ids = h.app.session.visible_cloned();
    let (target, reference) = (ids[0], ids[1]);
    h.app.session.execute("develop.set", &json!({"ids": [reference.0], "values": {"light.exposure": -1.5, "wb.temp": 9000}})).unwrap();
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [target.0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.menu.invoke", json!({"id": "dialog.matchLook", "params": {"photo": reference.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(widget(&h, &format!("matchLook:photo:{}", reference.0)).is_some(), "the view's photos are offered");
    assert!(h.step_until(SETTLE, |h| widget(h, "matchLook:reference").is_some()), "the reference shows small");
    let before = (*h.app.session.develop_of(target).unwrap()).clone();
    let r = h.request("ui.clickWidget", json!({"id": "matchLook:apply"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let done = h.step_until(SETTLE, |h| {
        h.app.ui.match_look.as_ref().is_some_and(|s| !s.running()) && h.app.session.develop_of(target).is_some_and(|d| *d != before)
    });
    assert!(done, "the match lands on the photo");
    assert!(h.app.session.develop_of(target).unwrap().light.exposure < before.light.exposure, "darker, like the reference");
    h.app.session.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(*h.app.session.develop_of(target).unwrap(), before, "one undo step");
}

#[test]
fn a_chosen_file_is_read_off_the_ui_thread_and_shows_when_ready() {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    app.ui.settings.gpu = false;
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let dir = std::env::temp_dir().join(format!("lightcraft-match-look-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reference.png");
    let img = lightcraft_raster::Rgba8::from_fn(320, 200, |x, y| [(x % 256) as u8, (y % 256) as u8, 120, 255]);
    std::fs::write(&path, lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap()).unwrap();
    let r = h.app.run("matchLook.chooseFile", json!({"path": path.to_string_lossy()}));
    assert!(r.is_ok(), "{r:?}");
    assert!(!h.app.ui.match_look.as_ref().unwrap().preview_ready(), "the command returns before the file is read");
    assert!(h.step_until(SETTLE, |h| widget(h, "matchLook:reference").is_some()), "the preview shows once read");
    let _ = std::fs::remove_dir_all(&dir);
}
