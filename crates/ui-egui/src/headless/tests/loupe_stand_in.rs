//! What the loupe shows while a photo's render is on its way.
use super::*;
use crate::render::Slot;
use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use std::{cell::Cell, rc::Rc};

/// Opening an unedited raw put the camera's JPEG up first (its grid thumbnail or embedded preview), and
/// the render replaced it a moment later: the camera's colours and lens correction are not LightCraft's,
/// so the photo flashed another look and shifted. The loupe draws nothing of it (a plain frame saying
/// "Rendering…") until the render is up, and never the previous photo's pixels (it held those for the
/// raw's first 0.7 s: the wrong photo); the camera's JPEG stands in only for a raw that can't be rendered.
#[test]
fn the_loupe_never_draws_another_photos_pixels_or_the_camera_jpeg() {
    let mut h = demo([1000.0, 700.0]);
    let t = Duration::from_secs(10);
    h.request("ui.set", json!({"view": "detail", "right": "none"}), t);
    assert!(h.step_until(SETTLE, |h| h.app.loupe_shown.is_some_and(|l| l.1 == "render")));
    let first = h.app.session.active().expect("a photo is open");
    // an unedited raw with a camera JPEG (orange; its file need not exist: the render is held, then fails)
    h.app.session.media.preview_loader =
        Some(Arc::new(|_, _| Some(lightcraft_raster::Rgba8 { width: 6, height: 4, data: vec![[230, 120, 40, 255]; 24] })));
    let id = PhotoId(h.app.session.catalog.photos().map(|p| p.id.0).max().unwrap_or(0) + 1);
    let dir = std::env::temp_dir().join(format!("lc-ui-stand-in-{}", std::process::id()));
    let mut raw = Photo::new(
        id,
        Source::File { path: dir.join("A7V1.ARW").to_string_lossy().into_owned() },
        "A7V1.ARW",
        "ARW",
        600,
        400,
        "2026-10-01T00:00:00",
    );
    raw.kind = MediaKind::Raw;
    raw.develop = Arc::new(raw.camera_defaults());
    h.app.session.commit("setup", Op::AddPhoto { photo: Box::new(raw) }).unwrap();
    // its loupe render waits; quick previews and thumbnails don't
    let open = Rc::new(Cell::new(false));
    h.app.renderer.set_offload(Box::new(crate::headless::RenderGate::failing(open.clone(), |s| s == Slot::Main)));
    crate::panels::grid::request_thumb(&mut h.app, id, 256, 10);
    h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [id.0], "active": id.0}}), t);
    let camera = |h: &Headless| h.app.renderer.thumb(id).is_some_and(|t| t.quick == Some(lightcraft_engine::media::QuickSource::Embedded));
    assert!(h.step_until(SETTLE, camera), "the camera JPEG thumbnail is there to be (mis)used");
    for _ in 0..20 {
        h.step();
        assert_eq!(h.app.loupe_shown, Some((id, "none")), "neither the last photo nor the camera's JPEG");
    }
    // the previous photo's texture is still in the slot, and is not drawn
    assert_eq!(h.app.renderer.textures.get(&Slot::Main).map(|t| t.photo), Some(first));
    assert!(h.painted_text().iter().any(|text| text == "Rendering…"), "a plain frame that says so");
    // a raw that can't be rendered has nothing but the camera's JPEG: it shows, with the failure said
    open.set(true);
    assert!(h.step_until(SETTLE, |h| h.app.renderer.failure_of(Slot::Main, id).is_some()));
    assert!(h.step_until(SETTLE, |h| h.app.loupe_shown.is_some_and(|l| l.0 == id && matches!(l.1, "thumb" | "embedded"))));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The loupe's stand-in is this photo with its current look (a cached render or a thumbnail made from
/// its settings as they are now, scaled to fit), drawn until the render lands; one made for another
/// look, or another photo's texture, is never drawn: a plain frame instead.
#[test]
fn the_loupe_draws_only_a_stand_in_of_the_photos_current_look() {
    let mut h = demo([1000.0, 700.0]);
    let t = Duration::from_secs(10);
    h.request("ui.set", json!({"view": "detail", "right": "none"}), t);
    assert!(h.step_until(SETTLE, |h| h.app.loupe_shown.is_some_and(|l| l.1 == "render")));
    let first = h.app.session.active().expect("a photo is open");
    let other = h.app.session.visible_cloned().into_iter().map(|p| p.0).find(|p| *p != first.0).expect("a second photo");
    let other = PhotoId(other);
    // its loupe render waits; stand-ins and thumbnails don't
    let open = Rc::new(Cell::new(false));
    h.app.renderer.set_offload(Box::new(crate::headless::RenderGate::new(open.clone())));
    h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [other.0], "active": other.0}}), t);
    let own_stand_in = |h: &Headless| h.app.loupe_shown.is_some_and(|l| l.0 == other && matches!(l.1, "cached" | "small" | "thumb"));
    assert!(h.step_until(SETTLE, own_stand_in), "its stand-in, of its own look: {:?}", h.app.loupe_shown);
    // whatever is drawn is the photo's own, never the previous photo's
    assert!(h.app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == first), "the previous render is still in the slot");
    // the stand-ins are of an older look (settings changed since): not drawn, the frame says it is rendering
    for slot in [Slot::Preview, Slot::ThumbQuick(other), Slot::Thumb(other)] {
        if let Some(t) = h.app.renderer.textures.get_mut(&slot) {
            t.key ^= 1;
        }
    }
    h.step();
    assert_eq!(h.app.loupe_shown, Some((other, "none")), "a stand-in of another look is not drawn");
    assert!(h.painted_text().iter().any(|text| text == "Rendering…"));
    open.set(true);
    assert!(h.step_until(SETTLE, |h| h.app.loupe_shown == Some((other, "render"))), "{:?}", h.app.loupe_shown);
}
