//! A brush stroke's points are held once, however many History and undo steps hold the mask (issue #370).

use lightcraft_develop::{DevelopSettings, MaskShape};
use serde_json::json;

/// Where the points of stroke `i` of the first mask are in memory.
fn points_at(d: &DevelopSettings, i: usize) -> *const lightcraft_geom::Point {
    let MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("a brush mask") };
    strokes[i].points.as_ptr()
}

/// Each stroke made every History step (and its undo step) a copy of the whole settings, every earlier stroke's
/// points included: memory grew with the square of the strokes. The steps now share the points.
#[test]
fn a_new_stroke_shares_the_earlier_strokes_points_with_every_step() {
    let mut s = super::demo();
    let id = s.active().unwrap();
    for k in 0..3 {
        let y = 0.2 + 0.2 * f64::from(k);
        s.execute("mask.brushStroke", &json!({"points": [[0.1, y], [0.5, y], [0.9, y]], "size": 0.02})).unwrap();
    }
    let now = s.develop_of(id).unwrap();
    let history = &s.catalog.photo(id).unwrap().history;
    let painted: Vec<_> = history
        .iter()
        .filter(|h| {
            let part = h.settings.masks.first().and_then(|m| m.components.first());
            matches!(part.map(|c| &c.shape), Some(MaskShape::Brush { strokes }) if !strokes.is_empty())
        })
        .collect();
    assert_eq!(painted.len(), 3, "a History step per stroke");
    for step in &painted {
        assert_eq!(points_at(&step.settings, 0), points_at(&now, 0), "{}: the first stroke's points are shared", step.label);
    }
    assert_eq!(points_at(&painted[1].settings, 1), points_at(&now, 1));
    // undo and redo bring back the same strokes
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(points_at(&s.develop_of(id).unwrap(), 0), points_at(&now, 0));
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(*s.develop_of(id).unwrap(), *now);
}
