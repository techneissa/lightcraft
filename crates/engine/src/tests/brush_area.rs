//! Brushed strokes that close on themselves (`fork/brush-area.md`): `spot.add` stores a lasso, the shape of the loop's
//! inside; an open stroke stays a stroke, byte for byte; Make Stroke gives a lasso back its stroke.

use lightcraft_develop::{Spot, SpotFill, SpotMode};
use lightcraft_geom::Point;
use serde_json::json;

use crate::Session;

fn spots(s: &Session) -> Vec<Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

/// `n` points around (cx, cy), radius `rad` of the width, `turn` radians of the way round.
fn ring_at(cx: f64, cy: f64, rad: f64, n: usize, turn: f64) -> Vec<[f64; 2]> {
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64 * turn;
            [cx + rad * t.cos(), cy + rad * t.sin()]
        })
        .collect()
}

/// A loop a little short of closing, around the middle of the photo.
fn ring(n: usize, turn: f64) -> Vec<[f64; 2]> {
    ring_at(0.5, 0.5, 0.2, n, turn)
}

// Given a stroke that comes back to its start, the spot is one shape (a lasso) with no source, one undo step
#[test]
fn a_closed_stroke_is_a_lasso() {
    let mut s = Session::with_demo();
    let commits = s.commits();
    let r = s.execute("spot.add", &json!({"mode": "remove", "points": ring(120, 6.15), "size": 0.02})).unwrap();
    assert_eq!(r["lasso"], json!(true));
    assert_eq!(s.commits(), commits + 1);
    let all = spots(&s);
    assert_eq!(all.len(), 1);
    let sp = &all[0];
    let area = sp.area.as_ref().unwrap();
    assert_eq!((area.kind.as_str(), area.label()), ("lasso", "Lasso"));
    assert_eq!((sp.mode, sp.fill, sp.source_offset), (SpotMode::Remove, SpotFill::Classic, None));
    assert_eq!(sp.points.len(), 1, "the pin sits in the middle");
    assert!((sp.points[0].x - 0.5).abs() < 0.05 && (sp.points[0].y - 0.5).abs() < 0.08, "{:?}", sp.points);
    assert_eq!(sp.size, 0.02, "the spot reaches one brush radius past the loop (its Expand)");
    assert_eq!((area.stroke.len(), area.brush), (120, 0.02), "the stroke is kept");
    // it survives a save and a load
    let back: Spot = serde_json::from_value(serde_json::to_value(sp).unwrap()).unwrap();
    assert_eq!(&back, sp);
}

// Given an open stroke, a loop with Heal or Clone, `lasso: false`, or an explicit `source`, the spot stays a stroke
#[test]
fn other_strokes_stay_strokes() {
    let mut s = Session::with_demo();
    let open: Vec<[f64; 2]> = (0..60).map(|i| [0.2 + 0.6 * i as f64 / 59.0, 0.5]).collect();
    let r = s.execute("spot.add", &json!({"points": open, "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    assert!(r.get("lasso").is_none());
    s.execute("spot.add", &json!({"mode": "heal", "points": ring(120, 6.15), "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    s.execute("spot.add", &json!({"mode": "clone", "points": ring(120, 6.15), "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    s.execute("spot.add", &json!({"mode": "remove", "points": ring(120, 6.15), "size": 0.02, "lasso": false})).unwrap();
    // an explicit source is kept with the stroke it names, never silently dropped
    s.execute("spot.add", &json!({"mode": "remove", "points": ring(120, 6.15), "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    s.execute("spot.add", &json!({"mode": "remove", "points": ring(120, 3.0), "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    let all = spots(&s);
    assert_eq!(all.len(), 6);
    assert!(all.iter().all(|sp| sp.area.is_none() && sp.points.len() > 1));
    assert!(all[4].source_offset.is_some(), "the named source stays");
    // (an old spot saves as it did: no area written)
    assert!(serde_json::to_value(&all[0]).unwrap().get("area").is_none());
}

// Given a small circle around a blemish (inside under three brush discs), it stays a source-copy stroke with its pin
#[test]
fn a_small_loop_stays_a_stroke() {
    let mut s = Session::with_demo();
    s.execute("spot.add", &json!({"mode": "remove", "points": ring_at(0.4, 0.4, 0.03, 40, 6.15), "size": 0.02})).unwrap();
    let all = spots(&s);
    assert!(all[0].area.is_none() && all[0].points.len() == 40);
    assert!(all[0].source_offset.is_some(), "a Classic Remove stroke copies from a source");
}

// Given a loop over most of the photo, the command refuses it and adds nothing, with Remove This Area's message
#[test]
fn a_loop_over_most_of_the_photo_is_refused() {
    let mut s = Session::with_demo();
    let commits = s.commits();
    let e = s.execute("spot.add", &json!({"mode": "remove", "points": ring_at(0.5, 0.5, 0.49, 160, 6.25), "size": 0.02})).unwrap_err();
    assert!(e.to_string().contains("covers most of the photo; Remove rebuilds smaller areas from what is around them"), "{e}");
    assert_eq!((spots(&s).len(), s.commits()), (0, commits));
    // the same stroke with the lasso off is a stroke
    s.execute("spot.add", &json!({"mode": "remove", "points": ring_at(0.5, 0.5, 0.49, 160, 6.25), "size": 0.02, "lasso": false})).unwrap();
    assert_eq!(spots(&s).len(), 1);
}

// Given a loop that runs off the photo, the shape and its pin stay on the photo
#[test]
fn a_loop_off_the_edge_stays_on_the_photo() {
    let mut s = Session::with_demo();
    s.execute("spot.add", &json!({"mode": "fill", "points": ring_at(0.98, 0.5, 0.15, 100, 6.15), "size": 0.02})).unwrap();
    let sp = &spots(&s)[0];
    let r = sp.area.as_ref().unwrap().bounds().unwrap();
    assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= 1.0 && r[3] <= 1.0, "{r:?}");
    assert!((0.0..=1.0).contains(&sp.points[0].x) && (0.0..=1.0).contains(&sp.points[0].y));
}

// Given a lasso, Make Stroke gives back the stroke it came from, in one undo step; Heal-less strokes pick a source
#[test]
fn make_stroke_gives_back_the_stroke() {
    let mut s = Session::with_demo();
    let pts = ring(120, 6.15);
    s.execute("spot.add", &json!({"mode": "remove", "points": pts, "size": 0.03, "feather": 70.0, "opacity": 80.0})).unwrap();
    assert!(spots(&s)[0].area.is_some());
    let commits = s.commits();
    s.execute("spot.makeStroke", &json!({"index": 0})).unwrap();
    assert_eq!(s.commits(), commits + 1, "one undo step");
    let sp = &spots(&s)[0];
    let want: Vec<Point> = pts.iter().map(|p| Point::new(p[0], p[1])).collect();
    assert!(sp.area.is_none());
    assert_eq!((&sp.points, sp.size, sp.feather, sp.opacity, sp.mode), (&want, 0.03, 70.0, 80.0, SpotMode::Remove), "lossless");
    assert!(sp.source_offset.is_some(), "a Classic Remove stroke picks its source again");
    // not a lasso any more, so nothing to give back
    assert!(s.execute("spot.makeStroke", &json!({"index": 0})).is_err());
    // undo brings the lasso back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s)[0].area.is_some());
}

// Given a lasso that was moved, its stroke moves with it, so Make Stroke gives it back where it is
#[test]
fn a_moved_lasso_keeps_its_stroke_with_it() {
    let mut s = Session::with_demo();
    s.execute("spot.add", &json!({"mode": "fill", "points": ring(120, 6.15), "size": 0.02})).unwrap();
    s.execute("spot.update", &json!({"index": 0, "move": [0.1, -0.05]})).unwrap();
    s.execute("spot.makeStroke", &json!({"index": 0})).unwrap();
    let sp = &spots(&s)[0];
    let first = ring(120, 6.15)[0];
    assert!((sp.points[0].x - (first[0] + 0.1)).abs() < 1e-9 && (sp.points[0].y - (first[1] - 0.05)).abs() < 1e-9, "{:?}", sp.points[0]);
}

// Given a stroke that is not a lasso (an Object spot, a plain stroke), Make Stroke refuses
#[test]
fn make_stroke_only_for_a_lasso() {
    let mut s = Session::with_demo();
    s.execute("spot.add", &json!({"points": [[0.3, 0.3], [0.31, 0.3]], "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    assert!(s.execute("spot.makeStroke", &json!({"index": 0})).is_err());
    assert!(s.execute("spot.makeStroke", &json!({"index": 5})).is_err());
}

// Given a hostile stroke (100k points, NaN, far off), the command answers or refuses and the photo is untouched
#[test]
fn a_hostile_stroke_does_not_panic() {
    let mut s = Session::with_demo();
    let mut pts: Vec<serde_json::Value> = (0..100_000).map(|i| json!([(i % 97) as f64 / 97.0, (i % 89) as f64 / 89.0])).collect();
    pts[10] = json!([f64::NAN, 0.5]);
    pts[11] = json!([1e300, -1e300]);
    let first = pts[0].clone();
    pts.push(first);
    let _ = s.execute("spot.add", &json!({"points": pts, "size": 0.02}));
    let _ = s.execute("spot.add", &json!({"points": pts, "size": 1e300}));
    let _ = s.execute("spot.add", &json!({"points": pts, "size": -1.0, "mode": "fill"}));
    assert!(s.develop_of(s.active().unwrap()).is_some());
}
