//! The Portrait tab's histogram and its ⌥-drag preview (issue #164): ⌥ held while a Portrait slider is dragged turns
//! the photo into a grey / white map of where the feature works on the face(s) it changes, until ⌥ or the mouse is
//! let go. A plain drag shows the photo.

use std::sync::Arc;
use std::time::Duration;

use egui::{Event, Modifiers, PointerButton};
use lightcraft_develop::retouch::{FEATURES, Face, NormRect, Part, PartMask};
use lightcraft_engine::retouch_input::FaceProvider;
use lightcraft_pipeline::Overlay;
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// Two faces, each with a Facial Skin mask that is all 1 over its box.
struct TwoFaces;

impl FaceProvider for TwoFaces {
    fn name(&self) -> &str {
        "Two Faces"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        let face = |b: NormRect| {
            let mask = PartMask { plane: lightcraft_raster::Plane::filled(8, 8, 1.0), area: b };
            Face { masks: vec![(Part::FacialSkin, mask)], ..Face::boxed(b) }
        };
        Ok(vec![face(NormRect::new(0.15, 0.2, 0.35, 0.5)), face(NormRect::new(0.6, 0.25, 0.8, 0.55))])
    }
}

/// The Portrait tab on the demo photo with its Skin section open and the faces found.
fn portrait() -> Headless {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = RightPanel::Portrait;
    h.app.renderer.keep_pixels = true;
    let photo = h.app.session.active().unwrap();
    h.app.session.retouch_detect_now(photo, false).unwrap();
    h.step();
    h.step();
    let r = h.request("ui.clickWidget", json!({"id": "section:portrait.skin"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn shown(h: &Headless) -> Overlay {
    let d = h.app.session.active().and_then(|id| h.app.session.develop_of(id)).map(|d| (*d).clone()).unwrap_or_default();
    crate::panels::detail::shown_overlay(&h.app, &d, h.view.ctx.cumulative_frame_nr())
}

fn frames(h: &mut Headless, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

fn track(h: &Headless, control: &str) -> egui::Rect {
    let id = format!("slider:{control}");
    h.app.widgets.iter().rev().find(|(w, _)| *w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id} on screen"))
}

/// Press on the middle of `control`'s track and drag it rightwards, with `modifiers` held; returns the points.
fn drag(h: &mut Headless, control: &str, modifiers: Modifiers) -> (egui::Pos2, egui::Vec2) {
    let t = track(h, control);
    let (from, step) = (t.center(), egui::vec2(t.width() * 0.04, 0.0));
    h.app.synthetic.push(Event::ModifiersChanged(modifiers));
    h.app.synthetic.push(Event::PointerMoved(from));
    h.app.synthetic.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    for i in 1..=4 {
        h.app.synthetic.push(Event::PointerMoved(from + step * i as f32));
    }
    frames(h, 10);
    (from, step)
}

fn skin() -> u8 {
    FEATURES.iter().position(|f| f.key == "skin").unwrap() as u8
}

/// (black, white, grey that is neither) shares of the loupe render's pixels, and whether every pixel is grey.
fn tones(h: &Headless) -> (f64, f64, bool) {
    let t = h.app.renderer.textures.get(&crate::render::Slot::Main).expect("loupe render");
    let px = &t.pixels.as_ref().expect("pixels kept").pixels;
    let n = px.len().max(1) as f64;
    let grey = px.iter().all(|c| c.r() == c.g() && c.g() == c.b());
    let black = px.iter().filter(|c| c.r() == 0).count() as f64 / n;
    let white = px.iter().filter(|c| c.r() == 255).count() as f64 / n;
    (black, white, grey)
}

/// ⌥-drag on Skin Smoothness shows the skin's area for every face, ⌥ let go brings the photo back (the drag goes on),
/// ⌥ again shows it, and the release of the mouse ends it.
#[test]
fn alt_drag_on_a_portrait_slider_shows_the_features_area_until_released() {
    let mut h = portrait();
    assert_eq!(shown(&h), Overlay::None);
    let control = "retouch.skin.smoothness";
    let (from, step) = drag(&mut h, control, Modifiers::ALT);
    assert_eq!(h.app.ui.dragging_control.as_deref(), Some(control), "the drag is going");
    assert_eq!(shown(&h), Overlay::RetouchArea { feature: skin(), face: Overlay::ALL_FACES });
    // (the map: grey, white on the two faces' skin, black elsewhere)
    h.settle(SETTLE);
    let (black, white, grey) = tones(&h);
    assert!(grey && white > 0.01 && black > 0.5, "a grey map with the faces white: black {black}, white {white}, grey {grey}");
    // ⌥ let go with the mouse still down: the photo again
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::NONE));
    h.app.synthetic.push(Event::PointerMoved(from + step * 5.0));
    frames(&mut h, 10);
    assert!(h.app.ui.dragging_control.is_some(), "the drag goes on");
    assert_eq!(shown(&h), Overlay::None);
    h.settle(SETTLE);
    assert!(!tones(&h).2, "the photo is no grey map");
    // ⌥ again, then the mouse released with ⌥ still down
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::ALT));
    h.app.synthetic.push(Event::PointerMoved(from + step * 6.0));
    frames(&mut h, 10);
    assert_eq!(shown(&h), Overlay::RetouchArea { feature: skin(), face: Overlay::ALL_FACES });
    h.app.synthetic.push(Event::PointerButton { pos: from + step * 6.0, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::ALT });
    h.settle(SETTLE);
    assert!(h.app.ui.dragging_control.is_none());
    assert_eq!(shown(&h), Overlay::None);
}

/// A drag without ⌥ shows the photo.
#[test]
fn a_plain_portrait_drag_shows_the_photo() {
    let mut h = portrait();
    let (from, step) = drag(&mut h, "retouch.skin.smoothness", Modifiers::NONE);
    assert_eq!(h.app.ui.dragging_control.as_deref(), Some("retouch.skin.smoothness"));
    assert_eq!(shown(&h), Overlay::None);
    h.app.synthetic.push(Event::PointerButton { pos: from + step * 4.0, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.settle(SETTLE);
    assert_eq!(shown(&h), Overlay::None);
}

/// With a face picked, the map is that face's.
#[test]
fn the_preview_is_the_picked_faces() {
    let mut h = portrait();
    let r = h.request("ui.clickWidget", json!({"id": "portrait:face:1"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    drag(&mut h, "retouch.skin.smoothness", Modifiers::ALT);
    assert_eq!(shown(&h), Overlay::RetouchArea { feature: skin(), face: 1 });
}

/// The Portrait tab shows the Edit tab's histogram above its sections, and hides it with View ▸ Histogram.
#[test]
fn the_portrait_tab_has_the_histogram() {
    let mut h = portrait();
    assert!(h.app.widgets.iter().any(|(w, _)| w == "histogram"), "the histogram is on the Portrait tab");
    h.app.ui.histogram = false;
    h.settle(SETTLE);
    h.step();
    assert!(!h.app.widgets.iter().any(|(w, _)| w == "histogram"), "and goes with the setting");
}
