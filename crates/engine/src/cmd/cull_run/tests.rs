use lightcraft_catalog::{Analysis, Eyes, Flag, Op};
use serde_json::json;

use super::*;

fn eyes(closed: Option<bool>, focus: Option<f32>) -> Option<Eyes> {
    Some(Eyes { faces: 1, main: 1, closed, focus })
}

fn row(id: PhotoId, t: i64, sharp: f32, e: Option<Eyes>) -> Row {
    let mut sig = [0f32; 64];
    sig[0] = 1.0;
    Row { id, time: Some(t), sharp, clip: 0.0, sig, eyes: e, fresh: None, eye_error: None }
}

fn measured(rows: Vec<Row>) -> Measured {
    let params = Params { reject_below: None, pick_best: false, gap: 10.0, same: 0.93 };
    Measured { params, rows, failed: Vec::new(), eyes_missing: None, mesh_engine: None, stopped: false }
}

/// A burst's best frame is the sharpest whose eyes are open and in focus; closed or soft eyes are suggested for
/// rejection, never rejected; the rule fields find them.
#[test]
fn bursts_prefer_open_sharp_eyes_and_eyes_are_only_suggested_for_rejection() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.visible_cloned().into_iter().take(4).collect();
    let rows = vec![
        // the sharpest frame blinks, the next is focused on the ear, the third is the one
        row(ids[0], 100, 80.0, eyes(Some(true), Some(90.0))),
        row(ids[1], 101, 75.0, eyes(Some(false), Some(20.0))),
        row(ids[2], 102, 60.0, eyes(Some(false), Some(85.0))),
        // eyes that could not be read are neither closed nor open
        row(ids[3], 500, 70.0, eyes(None, None)),
    ];
    let r = apply(&mut s, measured(rows)).unwrap();
    assert_eq!(r["groups"], 1, "{r}");
    let a = |s: &Session, k: usize| s.catalog.photo(ids[k]).unwrap().analysis.unwrap();
    assert!(a(&s, 2).best && !a(&s, 0).best && !a(&s, 1).best, "{r}");
    assert_eq!(a(&s, 0).eyes.and_then(|e| e.closed), Some(true));
    let suggested: Vec<(u64, Value)> = r["suggestReject"].as_array().unwrap().iter().map(|v| (v["id"].as_u64().unwrap(), v["why"].clone())).collect();
    assert_eq!(suggested, vec![(ids[0].0, json!(["eyesClosed"])), (ids[1].0, json!(["eyesOutOfFocus"]))]);
    assert!(ids.iter().all(|id| s.catalog.photo(*id).unwrap().flag != Flag::Reject), "never rejected for the eyes");
    let sorted = |mut v: Vec<PhotoId>| {
        v.sort();
        v
    };
    let filter = |s: &mut Session, field: &str, value: bool| {
        s.execute("library.filter", &json!({"ruleSet": {"rules": [{"field": field, "op": "is", "value": value}]}})).unwrap();
        let mut v: Vec<PhotoId> = s.visible_cloned().into_iter().filter(|id| ids.contains(id)).collect();
        v.sort();
        v
    };
    assert_eq!(filter(&mut s, "eyesClosed", true), vec![ids[0]]);
    assert_eq!(filter(&mut s, "eyesClosed", false), sorted(vec![ids[1], ids[2]]), "unknown eyes are not open");
    assert_eq!(filter(&mut s, "eyesOutOfFocus", true), vec![ids[1]]);
    assert_eq!(filter(&mut s, "eyesOutOfFocus", false), sorted(vec![ids[0], ids[2]]));
}

/// Every frame of a burst blinking: the sharpest is still its best.
#[test]
fn a_burst_that_blinks_throughout_keeps_its_sharpest() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.visible_cloned().into_iter().take(2).collect();
    let rows = vec![row(ids[0], 1, 40.0, eyes(Some(true), None)), row(ids[1], 2, 70.0, eyes(Some(true), None))];
    apply(&mut s, measured(rows)).unwrap();
    assert!(s.catalog.photo(ids[1]).unwrap().analysis.unwrap().best);
}

/// Without a face detector or face mesh (and no people parse), no eyes are read: each photo says why, the other
/// scores are filed, and nothing panics.
#[test]
fn without_models_the_eyes_are_unknown_and_the_rest_is_scored() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|i| i.0).collect();
    let r = s.execute("photo.analyze", &json!({"ids": ids})).unwrap();
    assert_eq!(r["photos"].as_array().map(Vec::len), Some(3), "{r}");
    assert!(r["eyes"]["unavailable"].is_string(), "{r}");
    for p in r["photos"].as_array().unwrap() {
        assert!(p["eyes"].is_null() && p["eyesError"].is_string(), "{p}");
        let id = PhotoId(p["id"].as_u64().unwrap());
        let an = s.catalog.photo(id).unwrap().analysis.unwrap();
        assert!(an.eyes.is_none() && an.sharpness.is_finite(), "{an:?}");
    }
    assert!(r["suggestReject"].as_array().unwrap().is_empty());
}

/// Eyes measured before are kept (by a run that measures and one that doesn't); a bad `eyes` is refused.
#[test]
fn eyes_measured_before_are_kept() {
    let mut s = Session::with_demo();
    let id = s.visible_cloned()[0];
    let before = eyes(Some(true), Some(12.0));
    s.commit("t", Op::SetAnalysis { id, analysis: Some(Analysis { eyes: before, ..Analysis::default() }) }).unwrap();
    for p in [json!({"ids": [id.0]}), json!({"ids": [id.0], "eyes": false})] {
        let r = s.execute("photo.analyze", &p).unwrap();
        assert_eq!(s.catalog.photo(id).unwrap().analysis.unwrap().eyes, before, "{p}: {r}");
        assert_eq!(r["photos"][0]["eyes"]["closed"], true, "{r}");
    }
    let r = s.execute("photo.analyze", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(r["eyes"]["kept"], 1, "{r}");
    assert_eq!(r["suggestReject"][0]["why"], json!(["eyesClosed", "eyesOutOfFocus"]));
    let undo = s.commits();
    for bad in [json!("yes"), json!(1), json!({"again": true})] {
        assert!(s.execute("photo.analyze", &json!({"ids": [id.0], "eyes": bad})).is_err(), "{bad}");
    }
    assert_eq!(s.commits(), undo);
}

/// A stopped run files nothing.
#[test]
fn a_stopped_run_changes_nothing() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|i| i.0).collect();
    let plan = plan(&mut s, &json!({"ids": ids})).unwrap();
    assert_eq!(plan.len(), 3);
    let stop = AtomicBool::new(true);
    let m = measure(plan, &stop, &|_| {});
    assert!(m.stopped && m.rows.is_empty());
    let undo = s.commits();
    assert!(apply(&mut s, m).is_err());
    assert_eq!(s.commits(), undo);
}
