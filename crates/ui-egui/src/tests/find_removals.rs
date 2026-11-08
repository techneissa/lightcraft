//! Find People / Find Wires in the Remove panel and on the photo (headless).

use std::time::Duration;

use lightcraft_engine::find_removals::Entry;
use lightcraft_pipeline::find::{Candidate, Kind, cover, shape::Mask};
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// A person-shaped candidate: a block at `x0..x1`, `y0..y1` (0..1 of the photo).
fn person(x0: f64, y0: f64, x1: f64, y1: f64) -> Candidate {
    let (w, h) = (300usize, 200usize);
    let m = Mask::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        (x0..x1).contains(&u) && (y0..y1).contains(&v)
    });
    Candidate {
        kind: Kind::Person,
        source: "sam3",
        outline: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
        closed: true,
        bbox: [x0, y0, x1, y1],
        area: (x1 - x0) * (y1 - y0),
        score: 0.9,
        main: false,
        spots: cover::cover_mask(&m),
        region: None,
    }
}

fn found(entries: Vec<Candidate>) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    assert_eq!(h.request("engine.execute", json!({"command": "panel.remove"}), T)["ok"], true);
    assert_eq!(h.app.ui.right, RightPanel::Remove);
    let photo = h.app.session.active();
    let entries = entries.into_iter().enumerate().map(|(i, candidate)| Entry { id: i as u32 + 1, candidate, selected: false }).collect();
    h.app.session.finds.restore(photo, entries);
    h
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
}

fn spots(h: &Headless) -> usize {
    h.app.session.develop_of(h.app.session.active().unwrap()).unwrap().spots.len()
}

#[test]
fn clicking_outlined_people_selects_them_and_remove_acts_on_the_selection() {
    let mut h = found(vec![person(0.1, 0.2, 0.2, 0.8), person(0.6, 0.3, 0.7, 0.9)]);
    h.settle(Duration::from_secs(10));
    // Remove has nothing to remove until something is selected
    let r = h.request("ui.clickWidget", json!({"id": "button:findRemove"}), T);
    assert!(r["ok"] == false || spots(&h) == 0, "{r}");
    click(&mut h, "findCandidate:2");
    let sel: Vec<bool> = h.app.session.finds.view(h.app.session.active()).entries.iter().map(|e| e.selected).collect();
    assert_eq!(sel, vec![false, true]);
    click(&mut h, "button:findRemove");
    assert!(spots(&h) > 0);
    let view = h.app.session.finds.view(h.app.session.active());
    assert_eq!(view.entries.len(), 1, "the removed one leaves the list");
    // Remove All takes the rest, in one undo step
    let before = spots(&h);
    click(&mut h, "button:findRemoveAll");
    assert!(spots(&h) > before);
    assert!(h.app.session.finds.view(h.app.session.active()).entries.is_empty());
    h.request("engine.execute", json!({"command": "edit.undo"}), T);
    assert_eq!(spots(&h), before);
}

#[test]
fn clear_forgets_the_candidates_and_changes_nothing() {
    let mut h = found(vec![person(0.1, 0.2, 0.2, 0.8)]);
    h.settle(Duration::from_secs(10));
    click(&mut h, "button:findClear");
    assert!(h.app.session.finds.view(h.app.session.active()).entries.is_empty());
    assert_eq!(spots(&h), 0);
}

#[test]
fn the_search_buttons_are_in_the_remove_panel() {
    let mut h = found(vec![]);
    h.settle(Duration::from_secs(10));
    // Find Wires runs on the demo photo and finds nothing in it; the button works and the app does not break
    click(&mut h, "button:findWires");
    for _ in 0..400 {
        if h.app.session.finds.view(h.app.session.active()).running.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(h.app.session.finds.view(h.app.session.active()).running.is_empty());
    assert_eq!(spots(&h), 0);
}
