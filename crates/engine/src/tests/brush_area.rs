//! Brushed strokes that close on themselves (`fork/brush-area.md`): `spot.add` stores a lasso, the shape of the stroke
//! with its inside; an open stroke stays a stroke, byte for byte.

use lightcraft_develop::{Spot, SpotFill, SpotMode};
use serde_json::json;

use crate::Session;

fn spots(s: &Session) -> Vec<Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

/// `n` points around (0.5, 0.5), radius 0.2 of the width, a little short of closing.
fn ring(n: usize, turn: f64) -> Vec<[f64; 2]> {
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64 * turn;
            [0.5 + 0.2 * t.cos(), 0.5 + 0.2 * t.sin()]
        })
        .collect()
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
    assert!(sp.size > 0.0 && sp.size <= 0.01, "the margin is the Expand: {}", sp.size);
    // it survives a save and a load
    let back: Spot = serde_json::from_value(serde_json::to_value(sp).unwrap()).unwrap();
    assert_eq!(&back, sp);
}

// Given an open stroke, a loop with Heal or Clone, or `lasso: false`, the spot stays a stroke
#[test]
fn other_strokes_stay_strokes() {
    let mut s = Session::with_demo();
    let open: Vec<[f64; 2]> = (0..60).map(|i| [0.2 + 0.6 * i as f64 / 59.0, 0.5]).collect();
    let r = s.execute("spot.add", &json!({"points": open, "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    assert!(r.get("lasso").is_none());
    s.execute("spot.add", &json!({"mode": "heal", "points": ring(120, 6.15), "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    s.execute("spot.add", &json!({"mode": "clone", "points": ring(120, 6.15), "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    s.execute("spot.add", &json!({"mode": "remove", "points": ring(120, 6.15), "size": 0.02, "source": [0.1, 0.0], "lasso": false})).unwrap();
    s.execute("spot.add", &json!({"mode": "remove", "points": ring(120, 3.0), "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    let all = spots(&s);
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|sp| sp.area.is_none() && sp.points.len() > 1));
    // (an old spot saves as it did: no area written)
    let v = serde_json::to_value(&all[0]).unwrap();
    assert!(v.get("area").is_none());
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
    let _ = s.execute("spot.add", &json!({"points": pts, "size": 0.02, "source": [0.1, 0.0]}));
    let _ = s.execute("spot.add", &json!({"points": pts, "size": 1e300}));
    let _ = s.execute("spot.add", &json!({"points": pts, "size": -1.0, "mode": "fill"}));
    assert!(s.develop_of(s.active().unwrap()).is_some());
}
