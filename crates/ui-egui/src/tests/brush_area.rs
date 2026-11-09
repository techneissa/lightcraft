//! Fork feature (`fork/brush-area.md`): a brushed spot is drawn as one outline and, while painting, one translucent
//! shape; a stroke that closes on itself becomes a lasso (headless).

use std::time::Duration;

use egui::epaint::Shape;
use lightcraft_develop::Spot;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn detail() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    exec(&mut h, "panel.remove", json!({}));
    h.step();
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn spots(h: &Headless) -> Vec<Spot> {
    h.app.session.develop_of(h.app.session.active().unwrap()).unwrap_or_default().spots.clone()
}

/// (circles, outline paths with many points, translucent fills) the last frame painted.
fn counts(h: &Headless) -> (usize, usize, usize) {
    let fill = egui::Color32::from_white_alpha(60);
    let shapes = h.view.painted_shapes();
    let circles = shapes.iter().filter(|s| matches!(s, Shape::Circle(_))).count();
    let outlines = shapes.iter().filter(|s| matches!(s, Shape::Path(p) if p.points.len() > 12)).count();
    let fills = shapes.iter().filter(|s| matches!(s, Shape::Mesh(m) if m.vertices.first().is_some_and(|v| v.color == fill))).count();
    (circles, outlines, fills)
}

fn stroke(n: usize) -> Vec<[f64; 2]> {
    (0..n).map(|i| [0.2 + 0.6 * i as f64 / (n - 1) as f64, 0.5 + 0.05 * (i as f64 / 8.0).sin()]).collect()
}

// Given a long brushed spot, the photo shows one outline for it, not a circle for each of its points: a stroke of 400
// points draws as many shapes as one of 40
#[test]
fn a_brushed_spot_is_one_outline() {
    let mut h = detail();
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    let mut drawn = Vec::new();
    for n in [40, 400] {
        exec(&mut h, "spot.add", json!({"mode": "heal", "points": stroke(n), "size": 0.02, "source": [0.0, 0.2]}));
        exec(&mut h, "spot.select", json!({"index": 0}));
        h.step();
        h.step();
        drawn.push(counts(&h));
        exec(&mut h, "spot.delete", json!({"index": 0}));
        h.step();
    }
    assert_eq!(drawn[0], drawn[1], "the shapes drawn do not grow with the stroke: {drawn:?}");
}

// Given a stroke being painted, it shows as one translucent shape, not a filled circle for each point
#[test]
fn a_stroke_being_painted_is_one_shape() {
    let mut h = detail();
    h.app.ui.remove_size = 0.03;
    let mut events = vec![json!({"kind": "down", "x": 0.3, "y": 0.5})];
    for i in 1..30 {
        events.push(json!({"kind": "drag", "x": 0.3 + 0.01 * i as f64, "y": 0.5}));
    }
    let r = h.request("ui.pointer", json!({"events": events}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.gesture.is_some(), "still painting");
    let (_, outlines, fills) = counts(&h);
    assert_eq!(fills, 1, "one translucent fill");
    assert!(outlines >= 1);
    let circles_filled =
        h.view.painted_shapes().iter().filter(|s| matches!(s, Shape::Circle(c) if c.fill == egui::Color32::from_white_alpha(60))).count();
    assert_eq!(circles_filled, 0);
    // the outline has a dark halo under it, so it shows on white sky
    let halos = h
        .view
        .painted_shapes()
        .iter()
        .filter(|s| matches!(s, Shape::Path(p) if p.stroke.color == egui::epaint::ColorMode::Solid(egui::Color32::from_black_alpha(110))))
        .count();
    assert!(halos >= 1, "a halo under the outline");
    let _ = h.request("ui.pointer", json!({"events": [{"kind": "up", "x": 0.59, "y": 0.5}]}), T);
}

// Given a stroke drawn around a spot of the photo and back to where it began, one lasso spot is made, in one undo step
#[test]
fn a_stroke_that_closes_is_a_lasso() {
    let mut h = detail();
    h.app.ui.remove_size = 0.03;
    let n = 48;
    let at = |i: usize| {
        let t = i as f64 / n as f64 * std::f64::consts::TAU;
        (0.5 + 0.25 * t.cos(), 0.5 + 0.25 * t.sin())
    };
    let mut events = vec![json!({"kind": "down", "x": at(0).0, "y": at(0).1})];
    for i in 1..=n {
        events.push(json!({"kind": "drag", "x": at(i).0, "y": at(i).1}));
    }
    events.push(json!({"kind": "up", "x": at(n).0, "y": at(n).1}));
    let r = h.request("ui.pointer", json!({"events": events}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let all = spots(&h);
    assert_eq!(all.len(), 1, "one spot");
    assert!(all[0].area.as_ref().is_some_and(|a| a.kind == "lasso"), "{:?}", all[0].area.as_ref().map(|a| a.kind.clone()));
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn circle_stroke_events(n: usize) -> Vec<serde_json::Value> {
    let at = |i: usize| {
        let t = i as f64 / n as f64 * std::f64::consts::TAU;
        (0.5 + 0.25 * t.cos(), 0.5 + 0.25 * t.sin())
    };
    let mut events = vec![json!({"kind": "down", "x": at(0).0, "y": at(0).1})];
    for i in 1..=n {
        events.push(json!({"kind": "drag", "x": at(i).0, "y": at(i).1}));
    }
    events.push(json!({"kind": "up", "x": at(n).0, "y": at(n).1}));
    events
}

// Given Fill Closed Loops turned off, a stroke that closes stays a stroke; the choice is saved with the tool options
#[test]
fn fill_closed_loops_can_be_turned_off() {
    let mut h = detail();
    h.app.ui.remove_size = 0.03;
    assert!(has(&h, "removeLasso") && h.app.ui.remove_lasso, "on by default");
    click(&mut h, "removeLasso");
    assert!(!h.app.ui.remove_lasso);
    assert_eq!(serde_json::to_value(&h.app.ui).unwrap()["removeLasso"], json!(false), "saved like the other options");
    let r = h.request("ui.pointer", json!({"events": circle_stroke_events(48)}), T);
    assert_eq!(r["ok"], true, "{r}");
    // (a Classic Remove stroke finds its source on a worker and is added a moment later)
    for _ in 0..400 {
        if !spots(&h).is_empty() {
            break;
        }
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let all = spots(&h);
    assert_eq!(all.len(), 1);
    assert!(all[0].area.is_none() && all[0].points.len() > 10, "a plain stroke");
    // an old ui.json without the key reads as on
    let old: crate::state::UiState = serde_json::from_value(json!({"removeSize": 0.03})).unwrap();
    assert!(old.remove_lasso);
}

// Given a lasso, its row has Make Stroke, which turns it back into the stroke; other rows have none
#[test]
fn a_lasso_row_can_make_its_stroke() {
    let mut h = detail();
    h.app.ui.remove_size = 0.03;
    exec(&mut h, "spot.add", json!({"mode": "heal", "points": stroke(40), "size": 0.02, "source": [0.0, 0.2]}));
    let r = h.request("ui.pointer", json!({"events": circle_stroke_events(48)}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(spots(&h).len(), 2);
    assert!(spots(&h)[1].area.as_ref().is_some_and(|a| a.kind == "lasso"));
    assert!(has(&h, "spotStroke:1") && !has(&h, "spotStroke:0"), "only the lasso has Make Stroke");
    let commits = h.app.session.commits();
    click(&mut h, "spotStroke:1");
    assert_eq!(h.app.session.commits(), commits + 1, "one undo step");
    assert!(spots(&h)[1].area.is_none() && spots(&h)[1].points.len() > 10);
    assert!(!has(&h, "spotStroke:1"));
}

// Given a size change on a spot, its outline is worked out again (the cache follows the spot)
#[test]
fn the_outline_follows_the_spot() {
    let mut h = detail();
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    exec(&mut h, "spot.add", json!({"mode": "heal", "points": stroke(60), "size": 0.02, "source": [0.0, 0.2]}));
    exec(&mut h, "spot.select", json!({"index": null}));
    let width = |h: &Headless| {
        h.view
            .painted_shapes()
            .iter()
            .filter_map(|s| match s {
                Shape::Path(p) if p.points.len() > 12 && p.stroke.color == egui::epaint::ColorMode::Solid(egui::Color32::from_white_alpha(150)) => {
                    let (a, b) = p.points.iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.y), b.max(q.y)));
                    Some(b - a)
                }
                _ => None,
            })
            .fold(0.0f32, f32::max)
    };
    h.step();
    h.step();
    let before = width(&h);
    exec(&mut h, "spot.update", json!({"index": 0, "size": 0.05}));
    h.step();
    h.step();
    let after = width(&h);
    assert!(before > 0.0 && after > before * 1.3, "the outline grew with the brush: {before} → {after}");
}
