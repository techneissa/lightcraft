//! Shaped Remove spots (`fork/object-remove.md`): Remove This Area, Object clicks and strokes, Find results, and which
//! spots have a source.

use lightcraft_develop::{SegMask, Spot, SpotFill, SpotMode};
use lightcraft_geom::Point;
use serde_json::json;

use crate::{ErrorKind, Session};

fn spots(s: &Session) -> Vec<Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

/// A selection of the box `[x0, y0, x1, y1]` (0..1) over the whole photo, as SAM 3 would store it.
fn box_seg(b: [f64; 4]) -> SegMask {
    let side = 128;
    let l: Vec<f32> = (0..side * side)
        .map(|i| {
            let (u, v) = (((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64);
            if (b[0]..b[2]).contains(&u) && (b[1]..b[3]).contains(&v) { 8.0 } else { -8.0 }
        })
        .collect();
    SegMask::from_logits(side, &l)
}

/// The demo session with a model folder that has no model (so every SAM 3 request says it is missing).
fn session() -> Session {
    let mut s = Session::with_demo();
    s.segmenter.dir = Some(std::env::temp_dir().join(format!("lc-object-remove-none-{}", std::process::id())));
    s
}

/// The box of the cells inside the spot's shape (its grid's rectangle holds a margin more).
fn bounds(sp: &Spot) -> [f64; 4] {
    let seg = &sp.area.as_ref().unwrap().seg;
    let (r, side, l) = (seg.bounds().unwrap(), seg.side as usize, seg.logits().unwrap());
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for (i, v) in l.iter().enumerate() {
        if *v > 0.0 {
            let (u, w) = ((i % side) as f64 + 0.5, (i / side) as f64 + 0.5);
            let (x, y) = (r[0] + u / side as f64 * (r[2] - r[0]), r[1] + w / side as f64 * (r[3] - r[1]));
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }
    b
}

// Given any mask, Remove This Area makes one Remove spot of its shape, in one undo step, and the mask stays
#[test]
fn a_mask_becomes_one_spot_of_its_shape() {
    let mut s = session();
    s.execute("mask.add", &json!({"kind": "radial", "center": [0.3, 0.6], "rx": 0.08, "ry": 0.08, "feather": 0})).unwrap();
    let commits = s.commits();
    let r = s.execute("spot.fromMask", &json!({})).unwrap();
    assert_eq!(s.commits(), commits + 1, "one undo step");
    let all = spots(&s);
    assert_eq!(all.len(), 1);
    let sp = &all[0];
    assert_eq!((r["index"].as_u64(), r["kind"].as_str()), (Some(0), Some("mask")));
    assert_eq!(s.active_spot, Some(0));
    assert_eq!((sp.mode, sp.fill, sp.source_offset), (SpotMode::Remove, SpotFill::Lama, None), "LaMa by default, no source");
    // the shape sits on the circle: around (0.3, 0.6), a little wider than its radius on the long edge
    let b = bounds(sp);
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    assert!((cx - 0.3).abs() < 0.02 && (cy - 0.6).abs() < 0.03, "{b:?}");
    // (0.16 of the long edge across: of the photo's width and height, whichever is the long one)
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    assert!((bw.min(bh) - 0.16).abs() < 0.02 && bw.max(bh) < 0.3, "{b:?}");
    assert_eq!(s.develop_of(s.active().unwrap()).unwrap().masks.len(), 1, "the mask stays");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s).is_empty());
    // Classic on request; nothing to remove is an error, not an empty spot
    s.execute("spot.fromMask", &json!({"fill": "classic", "expand": 0.02})).unwrap();
    let sp = &spots(&s)[0];
    assert_eq!((sp.fill, sp.size), (SpotFill::Classic, 0.02));
    s.execute("mask.add", &json!({"kind": "radial", "center": [3.0, 3.0], "rx": 0.01, "ry": 0.01})).unwrap();
    assert!(s.execute("spot.fromMask", &json!({})).is_err());
    // a mask over everything is refused too
    s.execute("mask.add", &json!({"kind": "linear", "start": [0.0, -0.5], "end": [0.0, -1.0]})).unwrap();
    let e = s.execute("spot.fromMask", &json!({})).unwrap_err().to_string();
    assert!(e.contains("most of the photo"), "{e}");
    assert_eq!(spots(&s).len(), 1);
}

// Given an Object mask (SAM 3's selection on the photo), its spot is in the object's place
#[test]
fn an_object_mask_becomes_a_spot_where_the_object_is() {
    let mut s = session();
    let seg = serde_json::to_value(box_seg([0.6, 0.2, 0.8, 0.5])).unwrap();
    s.execute("mask.add", &json!({"kind": "object", "seg": seg})).unwrap();
    s.execute("spot.fromMask", &json!({})).unwrap();
    let b = bounds(&spots(&s)[0]);
    assert!((b[0] - 0.6).abs() < 0.02 && (b[2] - 0.8).abs() < 0.02 && (b[1] - 0.2).abs() < 0.02 && (b[3] - 0.5).abs() < 0.02, "{b:?}");
}

// Given clicks on an object (the model's answer given), one spot takes it out; ⇧ and ⌥ change that spot, each one undo step
#[test]
fn clicks_make_and_change_one_object_spot() {
    let mut s = session();
    let seg = |b| serde_json::to_value(box_seg(b)).unwrap();
    let commits = s.commits();
    let r = s.execute("spot.object", &json!({"x": 0.4, "y": 0.4, "seg": seg([0.3, 0.3, 0.5, 0.5])})).unwrap();
    assert_eq!(r["index"], 0);
    assert_eq!(s.commits(), commits + 1);
    let sp = spots(&s)[0].clone();
    let a = sp.area.as_ref().unwrap();
    assert_eq!((a.kind.as_str(), a.hint.len(), a.exclude.len()), ("object", 1, 0));
    assert_eq!(sp.source_offset, None);
    assert!((sp.points[0].x - 0.4).abs() < 0.03, "the pin sits in the middle: {:?}", sp.points);
    // ⇧-click: the same spot, a bigger object
    s.execute("spot.object", &json!({"x": 0.55, "y": 0.4, "add": true, "seg": seg([0.3, 0.3, 0.6, 0.5])})).unwrap();
    assert_eq!(s.commits(), commits + 2);
    let all = spots(&s);
    assert_eq!(all.len(), 1, "changed, not added");
    assert!(bounds(&all[0])[2] > bounds(&sp)[2]);
    // ⌥-click: a part left out
    s.execute("spot.object", &json!({"x": 0.35, "y": 0.35, "subtract": true, "seg": seg([0.4, 0.3, 0.6, 0.5])})).unwrap();
    let a = spots(&s)[0].area.clone().unwrap();
    assert_eq!((a.hint.len(), a.exclude.len()), (2, 1));
    // a plain click starts another object
    s.execute("spot.object", &json!({"x": 0.8, "y": 0.8, "seg": seg([0.75, 0.75, 0.85, 0.85])})).unwrap();
    assert_eq!(spots(&s).len(), 2);
    // ⌥ with a stroke spot selected has nothing to change
    s.execute("spot.add", &json!({"mode": "heal", "points": [[0.1, 0.1]]})).unwrap();
    assert!(s.execute("spot.object", &json!({"x": 0.1, "y": 0.1, "subtract": true, "seg": seg([0.0, 0.0, 0.2, 0.2])})).is_err());
    // three undo steps take the three object edits back, one at a time
    for n in [2, 1, 1] {
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(spots(&s).len(), n);
    }
    // hostile points
    for p in [json!({"x": 1.5, "y": 0.5}), json!({"x": f64::MAX}), json!({}), json!({"at": ["a", 1]})] {
        assert!(s.execute("spot.object", &p).is_err(), "{p}");
    }
}

// Given no SAM 3, a click asks for the model and a loose stroke stays a brush-shaped spot
#[test]
fn without_the_model_clicks_ask_for_it_and_strokes_stay_brush_spots() {
    let mut s = session();
    let e = s.execute("spot.object", &json!({"x": 0.5, "y": 0.5})).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::ModelMissing(crate::AiModel::Sam3), "{e}");
    let r = s.execute("spot.objectStroke", &json!({"points": [[0.4, 0.5], [0.5, 0.5]], "size": 0.03, "fill": "classic"})).unwrap();
    assert_eq!(r["snapped"], false);
    let sp = &spots(&s)[0];
    assert!(sp.area.is_none() && sp.points.len() == 2 && (sp.size - 0.03).abs() < 1e-12);
    assert!(sp.source_offset.is_some(), "a Classic Remove spot has its source, as from the brush");
    assert!(s.execute("spot.objectStroke", &json!({"points": []})).is_err());
}

// Given a loose stroke and the model's answer, the spot is the object, cut to near the stroke
#[test]
fn a_stroke_snaps_to_the_object_near_it() {
    let mut s = session();
    // the model selects a tall object and a far-away part; the stroke runs over the tall one
    let seg = serde_json::to_value(box_seg([0.45, 0.1, 0.55, 0.9])).unwrap();
    let r = s.execute("spot.objectStroke", &json!({"points": [[0.5, 0.4], [0.5, 0.6]], "size": 0.05, "seg": seg})).unwrap();
    assert_eq!(r["snapped"], true);
    let b = bounds(&spots(&s)[0]);
    assert!(b[0] > 0.4 && b[2] < 0.6, "the object's width, not the brush's: {b:?}");
    assert!(b[1] > 0.15 && b[3] < 0.85, "cut to a few brush radii of the stroke: {b:?}");
}

// Given the fills and modes, only spots that copy from a source get one (and so a pin)
#[test]
fn only_heal_clone_and_classic_remove_have_a_source() {
    let mut s = session();
    let add = |s: &mut Session, p| s.execute("spot.add", &p).unwrap()["index"].as_u64().unwrap() as usize;
    let lama = add(&mut s, json!({"mode": "remove", "points": [[0.3, 0.3]], "fill": "lama"}));
    let classic = add(&mut s, json!({"mode": "remove", "points": [[0.5, 0.5]]}));
    let heal = add(&mut s, json!({"mode": "heal", "points": [[0.6, 0.6]]}));
    let clone = add(&mut s, json!({"mode": "clone", "points": [[0.7, 0.7]]}));
    let aware = add(&mut s, json!({"mode": "fill", "points": [[0.2, 0.7]], "fill": "lama"}));
    let all = spots(&s);
    assert_eq!(all[lama].source_offset, None, "LaMa Remove");
    assert_eq!(all[aware].source_offset, None, "Content-Aware");
    for i in [classic, heal, clone] {
        assert!(all[i].source_offset.is_some(), "spot {i}");
    }
    assert!(s.execute("spot.refreshSource", &json!({"index": lama})).is_err());
    // LaMa → Classic gives the spot a source; back to LaMa takes it away, in one undo step each
    s.execute("spot.update", &json!({"index": lama, "fill": "classic"})).unwrap();
    assert!(spots(&s)[lama].source_offset.is_some());
    let commits = s.commits();
    s.execute("spot.update", &json!({"index": lama, "fill": "lama"})).unwrap();
    assert_eq!(spots(&s)[lama].source_offset, None);
    assert_eq!(s.commits(), commits + 1);
    // a Classic Remove spot changed to LaMa loses its source; Heal keeps its own
    s.execute("spot.update", &json!({"index": classic, "fill": "lama"})).unwrap();
    assert_eq!(spots(&s)[classic].source_offset, None);
    assert!(spots(&s)[heal].source_offset.is_some());
}

// Given a shaped spot, it moves with its shape and never becomes a Heal or Clone spot with a source
#[test]
fn a_shaped_spot_moves_whole_and_has_no_source() {
    let mut s = session();
    let seg = serde_json::to_value(box_seg([0.3, 0.3, 0.5, 0.5])).unwrap();
    s.execute("spot.object", &json!({"x": 0.4, "y": 0.4, "seg": seg})).unwrap();
    let before = bounds(&spots(&s)[0]);
    s.execute("spot.update", &json!({"move": [0.1, 0.05], "size": 0.02})).unwrap();
    let sp = spots(&s)[0].clone();
    let after = bounds(&sp);
    assert!((after[0] - before[0] - 0.1).abs() < 1e-9 && (after[1] - before[1] - 0.05).abs() < 1e-9);
    assert_eq!(sp.size, 0.02, "Expand");
    for p in [json!({"mode": "heal"}), json!({"mode": "clone"}), json!({"source": [0.1, 0.0]}), json!({"moveSource": [0.1, 0.0]})] {
        assert!(s.execute("spot.update", &p).is_err(), "{p}");
    }
    s.execute("spot.update", &json!({"mode": "fill", "fill": "classic"})).unwrap();
    assert_eq!(spots(&s)[0].source_offset, None);
    assert!(s.execute("spot.refreshSource", &json!({})).is_err());
}

// Given found people with their shapes, Remove makes one shaped spot per person
#[test]
fn found_people_become_shaped_spots() {
    use lightcraft_pipeline::find::{cover, shape::Mask};
    let mut s = session();
    let id = s.active().unwrap();
    let person = |x0: f64, x1: f64| {
        let m = Mask::from_fn(300, 200, |x, y| (x0..x1).contains(&((x as f64 + 0.5) / 300.0)) && (0.3..0.8).contains(&((y as f64 + 0.5) / 200.0)));
        lightcraft_pipeline::find::Candidate {
            kind: lightcraft_pipeline::find::Kind::Person,
            source: "sam3",
            outline: vec![(x0, 0.3), (x1, 0.3), (x1, 0.8), (x0, 0.8)],
            closed: true,
            bbox: [x0, 0.3, x1, 0.8],
            area: (x1 - x0) * 0.5,
            score: 0.9,
            main: false,
            spots: cover::cover_mask(&m),
            region: Some(std::sync::Arc::new(m)),
        }
    };
    s.finds.inject(id, vec![person(0.1, 0.2), person(0.7, 0.75)]);
    let commits = s.commits();
    let r = s.execute("spot.removeCandidates", &json!({"all": true, "fill": "lama"})).unwrap();
    assert_eq!(r["spots"], 2, "{r}");
    assert_eq!(s.commits(), commits + 1);
    let all = spots(&s);
    assert!(all.iter().all(|sp| sp.area.as_ref().is_some_and(|a| a.kind == "person") && sp.source_offset.is_none() && sp.fill == SpotFill::Lama));
    let b = bounds(&all[1]);
    assert!((b[0] - 0.7).abs() < 0.01 && (b[2] - 0.75).abs() < 0.01, "{b:?}");
}

// Given a photo with old stroke spots, nothing about them changes: the same file, the same picture
#[test]
fn old_spots_save_and_render_as_before() {
    let mut s = session();
    let id = s.active().unwrap();
    s.execute("spot.add", &json!({"mode": "heal", "points": [[0.3, 0.3]]})).unwrap();
    s.execute("spot.add", &json!({"mode": "remove", "points": [[0.6, 0.6], [0.62, 0.6]]})).unwrap();
    let json = serde_json::to_string(&*s.develop_of(id).unwrap()).unwrap();
    assert!(!json.contains("area"), "nothing new is written for stroke spots");
    let back: lightcraft_develop::DevelopSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    let a = s.render_now(id, 300, 200).unwrap().image;
    let b = s.render_now(id, 300, 200).unwrap().image;
    assert_eq!(a.data, b.data);
    let _ = Point::new(0.0, 0.0);
}

// Given the app (background mode) and a damaged model, clicks never wait: they queue with a row in the activity stack,
// a second ⇧-click builds on the first, and the failure comes back as a message with no spot made
#[cfg(feature = "sam")]
#[test]
fn app_clicks_queue_and_failures_come_back_as_messages() {
    let mut s = Session::with_demo();
    s.segmenter.background = true;
    let dir = super::segment::damaged_model("object-remove");
    s.segmenter.dir = Some(dir.clone());
    let r = s.execute("spot.object", &json!({"x": 0.5, "y": 0.5})).unwrap();
    assert_eq!(r["pending"], true);
    assert!(s.object_jobs.busy());
    assert!(s.activity.list().iter().any(|t| t.label == "Selecting with SAM 3"), "a row while it works");
    let r = s.execute("spot.object", &json!({"x": 0.6, "y": 0.5, "subtract": true})).unwrap();
    assert_eq!((r["include"].as_u64(), r["exclude"].as_u64()), (Some(1), Some(1)), "builds on the click on its way");
    let t = std::time::Instant::now();
    let mut messages = Vec::new();
    while s.object_jobs.busy() {
        messages.extend(s.segment_poll().messages);
        assert!(t.elapsed() < std::time::Duration::from_secs(120), "the worker never answered");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!messages.is_empty(), "the failure is said");
    assert!(spots(&s).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// Given a spot already over the object, the next shape is measured on the photo, not on the filled-in pixels (a ⇧-click
// that refines an object spot came out as a blocky shape before)
#[test]
fn a_shape_is_measured_without_the_spots_over_it() {
    let mut s = session();
    let seg = serde_json::to_value(box_seg([0.3, 0.3, 0.6, 0.6])).unwrap();
    s.execute("mask.add", &json!({"kind": "object", "seg": seg})).unwrap();
    s.execute("spot.fromMask", &json!({"fill": "classic"})).unwrap();
    s.execute("spot.fromMask", &json!({"fill": "classic"})).unwrap();
    let all = spots(&s);
    assert_eq!(all[0].area, all[1].area, "the same mask, the same shape, whatever spots the photo has");
}
