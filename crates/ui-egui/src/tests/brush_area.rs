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
