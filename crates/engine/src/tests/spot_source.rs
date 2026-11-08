//! Issue #97: with `background: true`, `spot.add` returns before a new spot's source is found; the source arrives on
//! the spot a moment later, as part of the step that added it.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::Session;

fn spots(s: &Session) -> Vec<lightcraft_develop::Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

/// Polls (as each frame does) until no search is left; a hang guard, not a time limit on the search.
fn settle(s: &mut Session) {
    let start = Instant::now();
    while s.spot_sources.pending() > 0 {
        s.poll_spot_sources();
        assert!(start.elapsed() < Duration::from_secs(60), "the search never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    s.poll_spot_sources();
}

#[test]
fn a_spot_added_in_the_background_returns_before_its_source_is_found() {
    let _turn = crate::spot_source::TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::with_demo();
    let add = json!({"mode": "heal", "points": [[0.4, 0.5]], "size": 0.03});
    // the same spot found the old way, here and now, to compare with
    let mut sync = Session::with_demo();
    sync.execute("spot.add", &add).unwrap();
    let want = spots(&sync)[0].source_offset;
    assert!(want.is_some());

    // hold the search on its worker until the command has returned
    let (go, wait) = std::sync::mpsc::channel();
    *crate::spot_source::GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(wait);
    let mut bg = add.clone();
    bg["background"] = json!(true);
    let r = s.execute("spot.add", &bg).unwrap();
    assert_eq!(r, json!({"index": 0, "sourcePending": true}));
    assert_eq!(spots(&s).len(), 1, "the spot is there at once");
    assert_eq!(spots(&s)[0].source_offset, None, "its source is not found yet");
    assert_eq!(s.spot_sources.pending(), 1);
    assert_eq!(s.poll_spot_sources(), 0, "nothing to put while the search waits");
    go.send(()).unwrap();
    settle(&mut s);
    assert_eq!(spots(&s)[0].source_offset, want, "the source the old way finds");
    // one undo removes the spot and its source together
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s).is_empty());
}

#[test]
fn a_spot_changed_before_its_source_arrives_keeps_choosing_its_own() {
    let _turn = crate::spot_source::TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::with_demo();
    let (go, wait) = std::sync::mpsc::channel();
    *crate::spot_source::GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(wait);
    s.execute("spot.add", &json!({"mode": "remove", "points": [[0.4, 0.5]], "size": 0.03, "background": true})).unwrap();
    s.execute("spot.update", &json!({"index": 0, "move": [0.05, 0.0]})).unwrap();
    go.send(()).unwrap();
    settle(&mut s);
    assert_eq!(spots(&s)[0].source_offset, None, "the answer was for the spot where it was");
    // an explicit source, or a Content-Aware spot, starts no search
    let r = s.execute("spot.add", &json!({"mode": "fill", "points": [[0.6, 0.5]], "size": 0.03, "background": true})).unwrap();
    assert!(r.get("sourcePending").is_none(), "{r}");
    assert_eq!(s.spot_sources.pending(), 0);
}
