//! Issue #97: with `background: true`, `spot.add` returns before a new spot's source is found; the spot is added with
//! its source when the search on a worker answers, as one undo step, in the order the spots were asked for.

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

/// Hold the session's first search on its worker until the sender says go.
fn hold_first_search() -> std::sync::mpsc::Sender<()> {
    let (go, wait) = std::sync::mpsc::channel();
    *crate::spot_source::GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((0, wait));
    go
}

#[test]
fn a_spot_asked_in_the_background_returns_before_its_source_is_found() {
    let _turn = crate::spot_source::TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let add = json!({"mode": "heal", "points": [[0.4, 0.5]], "size": 0.03});
    // the same spot found the old way, here and now, to compare with
    let mut sync = Session::with_demo();
    sync.execute("spot.add", &add).unwrap();
    let want = spots(&sync)[0].clone();
    assert!(want.source_offset.is_some());

    let mut s = Session::with_demo();
    let go = hold_first_search();
    let mut bg = add.clone();
    bg["background"] = json!(true);
    let r = s.execute("spot.add", &bg).unwrap();
    assert_eq!(r, json!({"pending": true}), "the command returned while the search waits");
    assert_eq!(s.spot_sources.pending(), 1);
    assert_eq!(s.poll_spot_sources(), 0, "nothing to add while the search waits");
    assert!(spots(&s).is_empty());
    go.send(()).unwrap();
    settle(&mut s);
    assert_eq!(spots(&s), vec![want], "the spot, with the source the old way finds");
    assert_eq!(s.active_spot, Some(0), "and selected");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s).is_empty(), "one undo step");
}

#[test]
fn spots_asked_in_the_background_are_added_in_the_order_asked() {
    let _turn = crate::spot_source::TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::with_demo();
    // the first search waits, the second answers first
    let go = hold_first_search();
    for x in [0.3, 0.6] {
        s.execute("spot.add", &json!({"mode": "remove", "points": [[x, 0.5]], "size": 0.03, "background": true})).unwrap();
    }
    let start = Instant::now();
    while s.spot_sources.answered() < 1 {
        assert!(start.elapsed() < Duration::from_secs(60), "the second search never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(s.poll_spot_sources(), 0, "the second waits for the first");
    go.send(()).unwrap();
    settle(&mut s);
    assert_eq!(spots(&s).iter().map(|p| p.points[0].x).collect::<Vec<_>>(), vec![0.3, 0.6]);
    // an explicit source, or a Content-Aware spot, needs no search: added at once
    let r = s.execute("spot.add", &json!({"mode": "fill", "points": [[0.5, 0.8]], "size": 0.03, "background": true})).unwrap();
    assert_eq!(r["index"], json!(2), "{r}");
}
