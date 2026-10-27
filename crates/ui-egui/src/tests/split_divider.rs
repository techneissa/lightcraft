//! Headless tests of the draggable divider in the Before/After split views: dragging the line
//! moves where Before ends and After begins, double-clicking it puts it back in the middle, and
//! a drag on it does not pan, zoom or paint with a tool.

use std::time::Duration;

use egui::{Event, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

use super::before_after::mean_around;
use crate::headless::Headless;
use crate::render::Slot;
use crate::state::{BeforeAfter, Zoom};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);
const NONE: egui::Modifiers = egui::Modifiers::NONE;

/// Press at `from`, move to `to` over a few frames and release; the cursor seen mid-drag.
fn drag(h: &mut Headless, from: Pos2, to: Pos2) -> egui::CursorIcon {
    h.app.synthetic.push(Event::PointerMoved(from));
    h.step();
    h.app.synthetic.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: NONE });
    h.step();
    for i in 1..=6 {
        h.app.synthetic.push(Event::PointerMoved(from + (to - from) * (i as f32 / 6.0)));
        h.step();
    }
    let cursor = h.last_cursor;
    h.app.synthetic.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: NONE });
    h.step();
    h.settle(SETTLE);
    cursor
}

fn click(h: &mut Headless, at: Pos2) {
    h.app.synthetic.push(Event::PointerMoved(at));
    h.step();
    h.app.synthetic.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: NONE });
    h.step();
    h.app.synthetic.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: NONE });
    h.step();
}

/// The Detail view of the demo library's first photo with +1.5 stops, so Before and After differ.
fn detail(mode: BeforeAfter) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"control": "light.exposure", "value": 1.5}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.before_after = mode;
    h.settle(SETTLE);
    h
}

/// The photo on screen and where its line is at `at`.
fn geometry(h: &Headless, at: f32) -> (Rect, Pos2) {
    let (img, canvas) = (h.app.image_rect.expect("photo"), h.app.canvas_rect.expect("canvas"));
    (img.intersect(canvas), crate::region::wipe_line_at(img, canvas, at))
}

fn luma(m: [f32; 3]) -> f32 {
    m.iter().sum()
}

/// The screen at Fit as the edited photo and as the original.
fn references(h: &mut Headless, mode: BeforeAfter) -> (egui::ColorImage, egui::ColorImage) {
    let mut shot = |m| {
        h.app.ui.before_after = m;
        h.settle(SETTLE);
        h.paint()
    };
    let (edited, original) = (shot(BeforeAfter::Off), shot(BeforeAfter::Original));
    h.app.ui.before_after = mode;
    h.settle(SETTLE);
    (edited, original)
}

/// Dragging the line of Before/After Split to 25 % of the photo: left of it is the Before render,
/// right of it the After one (the middle of the photo, Before until then, is After now).
#[test]
fn dragging_the_line_to_a_quarter_shows_before_left_and_after_right() {
    let mut h = detail(BeforeAfter::Split);
    let (edited, original) = references(&mut h, BeforeAfter::Split);
    let (shown, line) = geometry(&h, 0.5);
    let y = shown.center().y;
    let (on_grip, on_line) = (pos2(line.x, y), pos2(line.x, shown.top() + 80.0));
    for grab in [on_grip, on_line] {
        h.app.ui.split_at = None;
        h.settle(SETTLE);
        let to = pos2(shown.left() + 0.25 * shown.width(), grab.y);
        let cursor = drag(&mut h, grab, to);
        assert_eq!(cursor, egui::CursorIcon::ResizeHorizontal, "the resize cursor shows while the line is dragged");
        let at = h.app.ui.split_at.expect("the line moved");
        assert!((at - 0.25).abs() < 0.01, "the line is at {at}");
        let s = h.paint();
        let x = |f: f32| shown.left() + f * shown.width();
        let (left, between, right) = (pos2(x(0.12), y), pos2(x(0.37), y), pos2(x(0.75), y));
        assert!((luma(mean_around(&s, left)) - luma(mean_around(&original, left))).abs() < 6.0, "left of the line is the Before");
        assert!((luma(mean_around(&s, between)) - luma(mean_around(&edited, between))).abs() < 6.0, "between 25 % and the middle is After now");
        assert!((luma(mean_around(&s, right)) - luma(mean_around(&edited, right))).abs() < 6.0, "right of the line is the After");
        assert!(luma(mean_around(&original, between)) + 30.0 < luma(mean_around(&edited, between)), "(the two renders differ there)");
    }
}

/// In Split Top/Bottom the line moves up and down: above it the Before, below it the After.
#[test]
fn dragging_the_top_bottom_line_shows_before_above_and_after_below() {
    let mut h = detail(BeforeAfter::SplitTopBottom);
    let (edited, original) = references(&mut h, BeforeAfter::SplitTopBottom);
    let (shown, line) = geometry(&h, 0.5);
    let x = shown.center().x;
    let to = pos2(x + 40.0, shown.top() + 0.25 * shown.height());
    let cursor = drag(&mut h, pos2(shown.left() + 90.0, line.y), to);
    assert_eq!(cursor, egui::CursorIcon::ResizeVertical);
    let at = h.app.ui.split_at.expect("the line moved");
    assert!((at - 0.25).abs() < 0.01, "the line is at {at}");
    let s = h.paint();
    let y = |f: f32| shown.top() + f * shown.height();
    let (above, between, below) = (pos2(x, y(0.12)), pos2(x, y(0.37)), pos2(x, y(0.75)));
    assert!((luma(mean_around(&s, above)) - luma(mean_around(&original, above))).abs() < 6.0, "above the line is the Before");
    assert!((luma(mean_around(&s, between)) - luma(mean_around(&edited, between))).abs() < 6.0, "below the line is the After");
    assert!((luma(mean_around(&s, below)) - luma(mean_around(&edited, below))).abs() < 6.0);
}

/// A double-click on the line puts it back in the middle; the position is shared by both splits
/// and follows the photo on screen, and a position past the photo is held to its edge.
#[test]
fn a_double_click_puts_the_line_back_in_the_middle() {
    let mut h = detail(BeforeAfter::Split);
    let (shown, line) = geometry(&h, 0.5);
    let grip = pos2(line.x, shown.center().y);
    drag(&mut h, grip, grip - vec2(shown.width() * 0.3, 0.0));
    let at = h.app.ui.split_at.expect("the line moved");
    assert!((at - 0.2).abs() < 0.01, "{at}");
    // both split views draw the one line
    h.app.ui.before_after = BeforeAfter::SplitTopBottom;
    h.settle(SETTLE);
    assert_eq!(h.app.ui.split_at, Some(at));
    h.app.ui.before_after = BeforeAfter::Split;
    h.settle(SETTLE);
    let (shown, line) = geometry(&h, at);
    let grip = pos2(line.x, shown.center().y);
    click(&mut h, grip);
    click(&mut h, grip);
    h.settle(SETTLE);
    assert_eq!(h.app.ui.split_at, None, "back in the middle");
    // a drag far past the photo's edge holds the line to the edge
    let (shown, line) = geometry(&h, 0.5);
    drag(&mut h, pos2(line.x, shown.center().y), pos2(shown.right() + 60.0, shown.center().y));
    assert_eq!(h.app.ui.split_at, Some(1.0));
}

/// Zoomed in, a drag on the line moves the line and leaves pan and zoom alone, while a drag
/// anywhere else still pans.
#[test]
fn a_drag_on_the_line_does_not_pan_or_zoom() {
    let mut h = detail(BeforeAfter::Split);
    h.app.ui.zoom = Zoom::Percent(200.0);
    h.app.ui.pan = (0.5, 0.5);
    h.settle(SETTLE);
    let (shown, line) = geometry(&h, 0.5);
    let (pan, zoom) = (h.app.ui.pan, h.app.ui.zoom);
    drag(&mut h, pos2(line.x, shown.top() + 100.0), pos2(line.x - 200.0, shown.top() + 140.0));
    assert_eq!((h.app.ui.pan, h.app.ui.zoom), (pan, zoom), "the view stays put");
    let at = h.app.ui.split_at.expect("the line moved");
    assert!((at - (0.5 - 200.0 / shown.width())).abs() < 0.01, "{at}");
    // elsewhere the view behaves as before
    let (shown, _) = geometry(&h, at);
    let from = pos2(shown.right() - 100.0, shown.center().y);
    drag(&mut h, from, from - vec2(100.0, 0.0));
    assert_ne!(h.app.ui.pan, pan, "a drag off the line still pans the zoomed photo");
    // the line follows the photo on screen: it kept its fraction of the visible part
    assert_eq!(h.app.ui.split_at, Some(at));
}

/// With the Remove tool armed, a drag on the line moves the line and paints no spot; a drag
/// elsewhere paints one.
#[test]
fn a_drag_on_the_line_paints_nothing_with_a_tool() {
    let mut h = detail(BeforeAfter::Split);
    let r = h.request("engine.execute", json!({"command": "panel.remove"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let spots = |h: &Headless| {
        let id = h.app.session.active().expect("active photo");
        h.app.session.develop_of(id).unwrap_or_default().spots.len()
    };
    let (shown, line) = geometry(&h, 0.5);
    let y = shown.center().y - 120.0;
    drag(&mut h, pos2(line.x, y), pos2(line.x - 150.0, y));
    assert!(h.app.ui.split_at.is_some_and(|a| a < 0.45), "the line moved");
    assert_eq!(spots(&h), 0, "no spot");
    let (shown, line) = geometry(&h, h.app.ui.split_at.unwrap_or(0.5));
    let from = pos2(line.x + 200.0, shown.center().y);
    drag(&mut h, from, from + vec2(40.0, 0.0));
    assert_eq!(spots(&h), 1, "off the line the tool paints");
}

/// Zoomed in to 1:1, the Before and After window renders are cut at the line where it was moved
/// to, not at the middle.
#[test]
fn region_windows_are_cut_at_the_moved_line() {
    for mode in [BeforeAfter::Split, BeforeAfter::SplitTopBottom] {
        let fixture = crate::tests_render_fixtures::RenderFixture::new("preview", 2);
        let app = LightcraftApp::new(fixture.session(), Services { png: None, ..Default::default() });
        let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
        let r = h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": false}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.app.ui.before_after = mode;
        h.request("engine.execute", json!({"command": "view.zoom100"}), T);
        h.settle(SETTLE);
        h.step();
        let (shown, line) = geometry(&h, 0.5);
        let vertical = mode == BeforeAfter::Split;
        let (from, to) = if vertical {
            (pos2(line.x, shown.top() + 100.0), pos2(shown.left() + 0.25 * shown.width(), shown.top() + 100.0))
        } else {
            (pos2(shown.left() + 100.0, line.y), pos2(shown.left() + 100.0, shown.top() + 0.25 * shown.height()))
        };
        drag(&mut h, from, to);
        h.step();
        let at = h.app.ui.split_at.expect("the line moved");
        let line = geometry(&h, at).1;
        let drawn = |slot: Slot| -> Vec<Rect> {
            let Some(tex) = h.app.renderer.textures.get(&slot) else { return vec![] };
            h.view
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::epaint::Shape::Mesh(m) if m.texture_id == tex.tex.id() => Some(c.clip_rect),
                    _ => None,
                })
                .collect()
        };
        let (before, after) = (drawn(Slot::RegionBefore), drawn(Slot::Region));
        assert!(!before.is_empty() && !after.is_empty(), "{mode:?}: both windows are painted");
        if vertical {
            assert!(before[0].right() <= line.x + 0.5 && after[0].left() >= line.x - 0.5, "{mode:?}: {before:?} {after:?} at x = {}", line.x);
            assert!(line.x < shown.center().x - 100.0, "{mode:?}: the line is not in the middle");
        } else {
            assert!(before[0].bottom() <= line.y + 0.5 && after[0].top() >= line.y - 0.5, "{mode:?}: {before:?} {after:?} at y = {}", line.y);
            assert!(line.y < shown.center().y - 100.0, "{mode:?}: the line is not in the middle");
        }
    }
}

/// The bottom bar has a Split button next to Before/After: one click opens the split, a second closes it.
#[test]
fn the_bottom_bar_has_a_split_button() {
    let mut h = detail(BeforeAfter::Off);
    assert_eq!(h.request("ui.clickWidget", json!({"id": "icon:beforeAfterSplit"}), T)["ok"], true);
    assert_eq!(h.app.ui.before_after, BeforeAfter::Split);
    h.settle(SETTLE);
    assert_eq!(h.request("ui.clickWidget", json!({"id": "icon:beforeAfterSplit"}), T)["ok"], true);
    assert_eq!(h.app.ui.before_after, BeforeAfter::Off);
}
