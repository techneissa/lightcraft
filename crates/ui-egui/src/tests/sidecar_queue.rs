//! The desktop app's automatic XMP sidecar writes go through the session's queue: the frame loop notes how each went,
//! and quitting lands what is still queued.

use std::time::Duration;

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);

fn rating(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let (_, rest) = text.split_once("<xmp:Rating>")?;
    rest.split_once("</xmp:Rating>").map(|(r, _)| r.to_string())
}

#[test]
fn the_frame_loop_lands_queued_sidecar_writes_and_quit_drains_them() {
    let dir = std::env::temp_dir().join(format!("lc-ui-sidecar-queue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = lightcraft_engine::Session::new();
    let n = 40;
    for i in 1..=n {
        let path = dir.join(format!("IMG_{i}.jpg"));
        std::fs::write(&path, b"jpeg").unwrap();
        let p =
            Photo::new(PhotoId(i), Source::File { path: path.to_string_lossy().to_string() }, &format!("IMG_{i}.jpg"), "JPEG", 4, 3, "2026-10-28");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.execute("library.xmpPreferences", &json!({"autoWrite": true})).unwrap();
    s.queue_sidecar_writes(true);
    let mut h = Headless::new(LightcraftApp::new(s, Services { png: None, ..Default::default() }), [1000.0, 700.0], 1.0);
    let ids: Vec<u64> = (1..=n).collect();
    h.app.session.execute("library.select", &json!({"ids": ids})).unwrap();
    let r = h.request("engine.execute", json!({"command": "photo.rate", "params": {"rating": 4}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.settle(T), "the frames run until the queue is empty");
    assert!((1..=n).all(|i| rating(&dir.join(format!("IMG_{i}.xmp"))).as_deref() == Some("4")));
    assert_eq!((h.app.session.sidecar_writes_pending(), h.app.session.unsaved_sidecars()), (0, 0));
    assert!(!h.app.session.activity.list().iter().any(|t| t.kind == "xmp"), "the row is gone");
    // quit right after a change: what `on_exit` does lands the queue
    h.app.session.execute("photo.rate", &json!({"rating": 2})).unwrap();
    h.app.session.close_library().unwrap();
    assert_eq!(h.app.session.sidecar_writes_pending(), 0);
    assert!((1..=n).all(|i| rating(&dir.join(format!("IMG_{i}.xmp"))).as_deref() == Some("2")), "none is lost");
    let _ = std::fs::remove_dir_all(&dir);
}
