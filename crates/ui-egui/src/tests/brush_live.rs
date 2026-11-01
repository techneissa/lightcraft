//! Headless tests of the brush's live adjustment preview ([`crate::brush_live`]): painting shows the
//! mask's adjustment under the brush at once, and the render that follows the stroke agrees.

use std::time::Duration;

use egui::ColorImage;
use serde_json::json;

use crate::headless::Headless;
use crate::render::Slot;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(60);

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
}

fn pointer(h: &mut Headless, events: serde_json::Value) {
    let r = h.request("ui.pointer", json!({"events": events}), T);
    assert_eq!(r["ok"], true, "{r}");
}

/// The Detail view with the brush tool, on a mask that already has an adjustment (a new brush mask
/// starts at zero, which shows nothing) and a stroke in the corner.
fn brush_ready() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "brushSize": 0.08, "brushFeather": 0.0, "brushFlow": 100.0, "brushErase": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "tool.brush", json!({"new": true}));
    exec(&mut h, "mask.brushStroke", json!({"points": [[0.05, 0.05], [0.06, 0.05]], "size": 0.02}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": 1.5}}));
    settle_previews(&mut h);
    h
}

/// Until the loupe and the full-coverage render are both the current ones, with their pixels.
fn settle_previews(h: &mut Headless) {
    let ready = |h: &Headless| {
        let r = &h.app.renderer;
        let ok = |s: Slot, key: Option<u64>| {
            r.textures.get(&s).is_some_and(|t| t.pixels.is_some() && key.is_some() && Some(t.key) == key) && !r.is_pending(s)
        };
        ok(Slot::Main, r.wanted(Slot::Main)) && ok(Slot::BrushFull, r.brush_target)
    };
    assert!(h.step_until(Duration::from_secs(120), ready), "the loupe and the brush preview never settled");
    // (a few frames more: the keyed re-render of the loupe after the tool opened)
    for _ in 0..5 {
        h.step();
    }
    assert!(h.step_until(Duration::from_secs(120), ready));
}

fn pixels(h: &Headless, slot: Slot) -> std::sync::Arc<ColorImage> {
    h.app.renderer.textures.get(&slot).and_then(|t| t.pixels.clone()).expect("pixels kept while the brush is open")
}

/// The pixel under normalized image position `(x, y)`.
fn at(img: &ColorImage, x: f64, y: f64) -> [i32; 3] {
    let (px, py) = (((x * img.size[0] as f64) as usize).min(img.size[0] - 1), ((y * img.size[1] as f64) as usize).min(img.size[1] - 1));
    let c = img.pixels[py * img.size[0] + px];
    [i32::from(c.r()), i32::from(c.g()), i32::from(c.b())]
}

fn diff(a: [i32; 3], b: [i32; 3]) -> i32 {
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).max().unwrap_or(0)
}

/// Painting a drag shows the adjustment under the brush while the button is still down, leaves the
/// rest of the photo alone, and the committed render then agrees with what was shown.
#[test]
fn the_adjustment_shows_under_the_brush_while_painting() {
    let mut h = brush_ready();
    let (base, full) = (pixels(&h, Slot::Main), pixels(&h, Slot::BrushFull));
    assert!(diff(at(&base, 0.5, 0.5), at(&full, 0.5, 0.5)) > 20, "the adjustment is visible in the middle of the photo");
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.5}, {"kind": "drag", "x": 0.4, "y": 0.5}, {"kind": "drag", "x": 0.5, "y": 0.5}, {"kind": "drag", "x": 0.6, "y": 0.5}]),
    );
    h.step();
    assert!(h.app.brush_live_ms().is_some(), "the stroke shows the adjustment, not only its colour");
    let live = h.app.brush_live.as_ref().and_then(|s| s.blend_image()).expect("blend").clone();
    // under the brush: the adjusted photo; far from it: untouched
    assert!(diff(at(&live, 0.45, 0.5), at(&full, 0.45, 0.5)) <= 1, "{:?} vs {:?}", at(&live, 0.45, 0.5), at(&full, 0.45, 0.5));
    assert_eq!(at(&live, 0.5, 0.9), at(&base, 0.5, 0.9));
    assert_eq!(at(&live, 0.9, 0.1), at(&base, 0.9, 0.1));
    pointer(&mut h, json!([{"kind": "up", "x": 0.6, "y": 0.5}]));
    settle_previews(&mut h);
    let after = pixels(&h, Slot::Main);
    // the settled render has the stroke: inside the core of the brush it is the full-coverage render
    for x in [0.35, 0.45, 0.55] {
        assert!(diff(at(&after, x, 0.5), at(&full, x, 0.5)) <= 2, "x {x}: {:?} vs {:?}", at(&after, x, 0.5), at(&full, x, 0.5));
    }
    assert!(diff(at(&after, 0.5, 0.9), at(&base, 0.5, 0.9)) <= 1, "away from the stroke nothing changed");
    assert!(h.app.brush_live.is_none(), "the held blend is dropped once the render has arrived");
}

/// Erasing blends toward the photo without the brush's coverage, and an undo restores the photo
/// as it was before the stroke.
#[test]
fn erasing_shows_live_and_undo_restores_the_photo() {
    let mut h = brush_ready();
    // paint a broad stroke first, then look at what erasing a part of it does
    exec(&mut h, "mask.brushStroke", json!({"points": [[0.2, 0.5], [0.8, 0.5]], "size": 0.08, "feather": 0.0}));
    settle_previews(&mut h);
    let painted = pixels(&h, Slot::Main);
    h.app.ui.brush_erase = true;
    settle_previews(&mut h);
    let empty = pixels(&h, Slot::BrushFull);
    assert!(diff(at(&painted, 0.5, 0.5), at(&empty, 0.5, 0.5)) > 20, "the stroke is visible before erasing");
    pointer(&mut h, json!([{"kind": "down", "x": 0.4, "y": 0.5}, {"kind": "drag", "x": 0.5, "y": 0.5}, {"kind": "drag", "x": 0.6, "y": 0.5}]));
    h.step();
    let live = h.app.brush_live.as_ref().and_then(|s| s.blend_image()).expect("blend").clone();
    assert!(diff(at(&live, 0.5, 0.5), at(&empty, 0.5, 0.5)) <= 1, "erased under the brush");
    assert!(diff(at(&live, 0.25, 0.5), at(&painted, 0.25, 0.5)) <= 1, "kept outside it");
    pointer(&mut h, json!([{"kind": "up", "x": 0.6, "y": 0.5}]));
    settle_previews(&mut h);
    let erased = pixels(&h, Slot::Main);
    assert!(diff(at(&erased, 0.5, 0.5), at(&empty, 0.5, 0.5)) <= 2, "the committed erase agrees with the preview");
    exec(&mut h, "edit.undo", json!({}));
    settle_previews(&mut h);
    let undone = pixels(&h, Slot::Main);
    assert!(diff(at(&undone, 0.5, 0.5), at(&painted, 0.5, 0.5)) <= 1, "undo brings the stroke back");
    assert!(h.app.brush_live.is_none());
}
