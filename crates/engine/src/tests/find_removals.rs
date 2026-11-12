//! Find People / Find Wires: the commands, the list of candidates, one-click removal in one undo step.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use lightcraft_develop::{SegMask, SpotMode};
use lightcraft_pipeline::find::{Kind, people};
use serde_json::json;

use crate::find_removals::{Job, PeopleSources, find};
use crate::{ErrorKind, Session};

pub(super) fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-find-removals-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A sky with one sagging wire, as a PNG in `dir`, opened in a session.
pub(super) fn session_with_wire(dir: &std::path::Path) -> Session {
    let (w, h) = (1200usize, 800usize);
    let data: Vec<[u8; 4]> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            let t = x / w as f64;
            let cy = 250.0 + 60.0 * t + 50.0 * 4.0 * t * (1.0 - t);
            let dark = ((1.25 + 0.5 - (y - cy).abs()).clamp(0.0, 1.0) as f32) * 0.25;
            let v = 0.75 - 0.2 * (y / h as f64) as f32 - dark;
            let b = (v * 255.0) as u8;
            [(v * 0.85 * 255.0) as u8, (v * 0.92 * 255.0) as u8, b, 255]
        })
        .collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
        &Default::default(),
    )
    .unwrap();
    std::fs::write(dir.join("wire.png"), png).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.join("wire.png").to_string_lossy()]})).unwrap();
    s
}

fn spots(s: &Session) -> Vec<lightcraft_develop::Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

#[test]
fn find_wires_lists_them_and_remove_all_is_one_undo_step() {
    let dir = temp_dir("wires");
    let mut s = session_with_wire(&dir);
    let id = s.active().unwrap();
    let before = s.render_now(id, 900, 600).unwrap().image;
    let r = s.execute("spot.findWires", &json!({})).unwrap();
    let list = r["candidates"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{r}");
    assert_eq!((list[0]["kind"].as_str(), list[0]["source"].as_str()), (Some("wire"), Some("lines")));
    assert!(spots(&s).is_empty(), "finding changes nothing in the photo");
    let q = s.execute("spot.candidates", &json!({})).unwrap();
    assert_eq!(q["candidates"].as_array().unwrap().len(), 1);
    let commits = s.commits();
    let r = s.execute("spot.removeCandidates", &json!({"all": true})).unwrap();
    let n = r["spots"].as_u64().unwrap() as usize;
    assert!(n == 1 && spots(&s).len() == n, "{r}");
    assert_eq!(s.commits(), commits + 1, "one undo step");
    // one spot in the wire's shape, with no source (`fork/object-remove.md`)
    assert!(
        spots(&s).iter().all(|sp| sp.mode == SpotMode::Remove && sp.source_offset.is_none() && sp.area.as_ref().is_some_and(|a| a.kind == "wire")),
        "a shaped spot"
    );
    assert_eq!(s.active_spot, Some(0));
    assert!(s.execute("spot.candidates", &json!({})).unwrap()["candidates"].as_array().unwrap().is_empty(), "removed ones leave the list");
    // the wire is gone from the render
    let after = s.render_now(id, 900, 600).unwrap().image;
    let at = |img: &lightcraft_raster::Rgba8, y: usize| img.data[y * 900 + 450][2] as i32;
    let dark = (232..262).map(|y| at(&before, y)).min().unwrap();
    let still = (232..262).map(|y| at(&after, y)).min().unwrap();
    assert!(still > dark + 20, "wire {dark} -> {still}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_removes_what_was_found_in_the_same_call() {
    let dir = temp_dir("add");
    let mut s = session_with_wire(&dir);
    let r = s.execute("spot.findWires", &json!({"add": true})).unwrap();
    assert_eq!(r["added"], 1, "{r}");
    assert!(!spots(&s).is_empty());
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// SAM 3's answer for two people: a big one in the middle and a small one at the right.
fn two_people_mask() -> SegMask {
    let l: Vec<f32> = (0..288 * 288)
        .map(|i| {
            let (u, v) = (((i % 288) as f64 + 0.5) / 288.0, ((i / 288) as f64 + 0.5) / 288.0);
            if ((u - 0.45).abs() < 0.08 && (0.25..0.85).contains(&v)) || ((u - 0.85).abs() < 0.02 && (0.55..0.7).contains(&v)) { 6.0 } else { -6.0 }
        })
        .collect();
    SegMask::from_logits(288, &l)
}

fn sam_says(mask: Option<SegMask>) -> PeopleSources {
    PeopleSources { sam: Some(Box::new(move |_| Ok(mask))), faces: None }
}

fn photo() -> lightcraft_raster::Rgba8 {
    lightcraft_raster::Rgba8 { width: 1500, height: 1000, data: vec![[120, 130, 140, 255]; 1_500_000] }
}

#[test]
fn the_main_subject_is_not_proposed_unless_asked() {
    let never = AtomicBool::new(false);
    let (found, text) = find(Job::People { include_main: false }, &photo(), sam_says(Some(two_people_mask())), &never).unwrap();
    assert_eq!(found.len(), 1, "{text}");
    assert!(!found[0].main && found[0].bbox[0] > 0.8, "only the small one at the right");
    assert!(text.contains("not counting the main subject"), "{text}");
    let (both, _) = find(Job::People { include_main: true }, &photo(), sam_says(Some(two_people_mask())), &never).unwrap();
    assert_eq!(both.len(), 2);
    assert_eq!(both.iter().filter(|c| c.main).count(), 1);
    // nobody in the photo
    let (none, text) = find(Job::People { include_main: false }, &photo(), sam_says(None), &never).unwrap();
    assert!(none.is_empty() && text == "No people found");
    // a lone person is the main subject: nothing to propose, and the text says why
    let alone = SegMask::from_logits(
        288,
        &(0..288 * 288)
            .map(|i| if ((i % 288) as f64 / 288.0 - 0.5).abs() < 0.1 && ((i / 288) as f64 / 288.0 - 0.5).abs() < 0.3 { 6.0 } else { -6.0 })
            .collect::<Vec<f32>>(),
    );
    let (none, text) = find(Job::People { include_main: false }, &photo(), sam_says(Some(alone)), &never).unwrap();
    assert!(none.is_empty() && text.contains("main subject"), "{text}");
}

#[test]
fn people_search_failures_are_errors_not_panics() {
    let never = AtomicBool::new(false);
    // SAM fails and there is no face detector
    let failing = PeopleSources { sam: Some(Box::new(|_| Err("the GPU fell over".into()))), faces: None };
    assert_eq!(find(Job::People { include_main: false }, &photo(), failing, &never).unwrap_err(), "the GPU fell over");
    // neither source
    assert!(find(Job::People { include_main: false }, &photo(), PeopleSources { sam: None, faces: None }, &never).is_err());
    // wires on a tiny or empty photo
    let tiny = lightcraft_raster::Rgba8 { width: 4, height: 4, data: vec![[0; 4]; 16] };
    assert_eq!(find(Job::Wires { sensitivity: 50.0 }, &tiny, PeopleSources { sam: None, faces: None }, &never).unwrap().1, "No wires found");
}

#[test]
fn people_need_a_model_and_say_which() {
    let mut s = Session::with_demo();
    let dir = temp_dir("nomodel");
    s.segmenter.dir = Some(dir.clone());
    let e = s.execute("spot.findPeople", &json!({})).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::ModelMissing(crate::AiModel::Sam3), "{e}");
    assert!(e.to_string().contains("face detector"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Candidates put in the list directly: selecting by id and by click, toggling, removing the selection or all of it.
#[test]
fn select_then_remove_the_selection_or_everything() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let mask = two_people_mask();
    let logits = mask.logits().unwrap();
    let mut found = people::from_logits(288, &logits, 1500, 1000);
    found.sort_by(|a, b| b.area.total_cmp(&a.area));
    s.finds.inject(id, found);
    let list = s.execute("spot.candidates", &json!({})).unwrap();
    let ids: Vec<u64> = list["candidates"].as_array().unwrap().iter().map(|c| c["id"].as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 2);
    // removing with nothing selected is refused
    assert!(s.execute("spot.removeCandidates", &json!({})).is_err());
    // a click on the small person at (0.85, 0.62) selects it only
    let r = s.execute("spot.selectCandidate", &json!({"at": [0.85, 0.62]})).unwrap();
    assert_eq!(r["selected"], json!([ids[1]]));
    // shift-click on the big one adds it; shift-click again removes it
    let r = s.execute("spot.selectCandidate", &json!({"at": [0.45, 0.5], "toggle": true})).unwrap();
    assert_eq!(r["selected"].as_array().unwrap().len(), 2);
    let r = s.execute("spot.selectCandidate", &json!({"id": ids[0], "toggle": true})).unwrap();
    assert_eq!(r["selected"], json!([ids[1]]));
    // a click on empty ground clears
    assert_eq!(s.execute("spot.selectCandidate", &json!({"at": [0.1, 0.1]})).unwrap()["selected"], json!([]));
    assert!(s.execute("spot.selectCandidate", &json!({"id": 999})).is_err());
    // remove the selected small one: its spots only, one undo step
    s.execute("spot.selectCandidate", &json!({"id": ids[1]})).unwrap();
    let commits = s.commits();
    let r = s.execute("spot.removeCandidates", &json!({})).unwrap();
    assert_eq!(r["removed"], 1);
    let first = spots(&s).len();
    assert_eq!(first as u64, r["spots"].as_u64().unwrap());
    assert_eq!(s.commits(), commits + 1);
    // Remove All takes the rest, again as one step
    let r = s.execute("spot.removeCandidates", &json!({"all": true})).unwrap();
    assert_eq!(r["removed"], 1);
    assert!(spots(&s).len() > first);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s).len(), first, "Remove All undoes in one step");
    // removing ids that are not there fails without touching the photo
    assert!(s.execute("spot.removeCandidates", &json!({"ids": [999]})).is_err());
    // the list belongs to its photo
    s.execute("library.next", &json!({})).unwrap();
    assert!(s.execute("spot.candidates", &json!({})).unwrap()["candidates"].as_array().unwrap().is_empty());
}

#[test]
fn hostile_command_parameters_are_refused() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    s.finds.inject(id, people::from_logits(288, &two_people_mask().logits().unwrap(), 1500, 1000));
    for p in [json!({"ids": "all"}), json!({"ids": [-1]}), json!({"ids": [1.5]}), json!({"ids": [4294967296u64]})] {
        assert!(s.execute("spot.removeCandidates", &p).is_err(), "{p}");
    }
    for p in [json!({"id": "x"}), json!({"id": -3}), json!({"id": 1e30})] {
        assert!(s.execute("spot.selectCandidate", &p).is_err(), "{p}");
    }
    for at in [json!(["a", null]), json!([0.5]), json!("0.5,0.5"), json!({"x": 1})] {
        s.execute("spot.selectCandidate", &json!({"id": null})).unwrap();
        let before = s.execute("spot.candidates", &json!({})).unwrap();
        assert!(s.execute("spot.selectCandidate", &json!({"at": at})).is_err(), "{at}");
        assert_eq!(s.execute("spot.candidates", &json!({})).unwrap(), before, "a refused click changes nothing");
    }
    assert!(s.execute("spot.findWires", &json!({"sensitivity": f64::MAX, "wait": true})).is_ok());
    s.execute("spot.clearCandidates", &json!({})).unwrap();
    assert!(s.execute("spot.candidates", &json!({})).unwrap()["candidates"].as_array().unwrap().is_empty());
    assert!(spots(&s).is_empty());
}

/// A search runs on its own thread with a cancellable row in the activity stack, and leaves nothing when cancelled.
#[test]
fn a_search_has_a_row_and_can_be_cancelled() {
    let dir = temp_dir("row");
    let mut s = session_with_wire(&dir);
    s.segmenter.background = true;
    let (tx, rx) = std::sync::mpsc::channel();
    *crate::find_removals::GATE.lock().unwrap() = Some(rx);
    let r = s.execute("spot.findWires", &json!({})).unwrap();
    assert_eq!(r["started"], true, "{r}");
    let rows = s.activity.list();
    let row = rows.iter().find(|t| t.kind == "findWires").expect("a row while it runs");
    assert!(row.cancellable && row.background, "its row is in the ring, with a ✕");
    assert_eq!(s.finds.view(s.active()).running, vec![Kind::Wire]);
    assert!(s.execute("spot.findWires", &json!({})).is_err(), "one search of a kind at a time");
    s.activity.cancel(row.id).unwrap();
    tx.send(()).unwrap();
    for _ in 0..200 {
        if s.finds.view(s.active()).running.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(s.finds.view(s.active()).running.is_empty());
    assert!(s.activity.list().iter().all(|t| t.kind != "findWires"), "the row is gone");
    assert!(s.finds.view(s.active()).entries.is_empty(), "a cancelled search proposes nothing");
    // and a second, uncancelled one in the background finishes into the list
    s.execute("spot.findWires", &json!({})).unwrap();
    for _ in 0..400 {
        if s.finds.view(s.active()).running.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(s.finds.view(s.active()).entries.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// SAM 3's "power line" mask: two bands across the photo.
fn wire_mask() -> SegMask {
    let l: Vec<f32> = (0..288 * 288)
        .map(|i| {
            let (x, y) = (i % 288, i / 288);
            let c1 = 60.0 + 0.1 * x as f64;
            let c2 = 140.0 + 0.05 * x as f64;
            if (y as f64 - c1).abs() < 1.5 || (y as f64 - c2).abs() < 1.5 { 6.0 } else { -6.0 }
        })
        .collect();
    SegMask::from_logits(288, &l)
}

#[test]
fn wires_come_from_sam_when_it_is_there_and_from_the_line_detector_when_not() {
    let never = AtomicBool::new(false);
    let sky = photo();
    let (found, text) = find(Job::Wires { sensitivity: 50.0 }, &sky, sam_says(Some(wire_mask())), &never).unwrap();
    assert_eq!(found.len(), 2, "{text}");
    assert!(found.iter().all(|c| c.kind == Kind::Wire && c.source == "sam3" && !c.main && !c.spots.is_empty() && c.bbox[2] - c.bbox[0] > 0.9));
    // SAM answers "nothing": believed, not second-guessed
    let (none, text) = find(Job::Wires { sensitivity: 50.0 }, &sky, sam_says(None), &never).unwrap();
    assert!(none.is_empty() && text == "No wires found", "{text}");
    // SAM fails: the line detector runs and the text says so
    let failing = PeopleSources { sam: Some(Box::new(|_| Err("the GPU fell over".into()))), faces: None };
    let (_, text) = find(Job::Wires { sensitivity: 50.0 }, &sky, failing, &never).unwrap();
    assert!(text.contains("SAM 3 failed") && text.contains("line detector"), "{text}");
}

/// `add` removes only what its own search found: the other kind's candidates stay listed.
#[test]
fn add_leaves_the_other_kinds_candidates_alone() {
    let dir = temp_dir("kinds");
    let mut s = session_with_wire(&dir);
    let id = s.active().unwrap();
    // a person found earlier, then Find Wires with add
    s.finds.inject(id, people::from_logits(288, &two_people_mask().logits().unwrap(), 1500, 1000));
    let r = s.execute("spot.findWires", &json!({"add": true})).unwrap();
    assert_eq!(r["added"], 1, "{r}");
    let left = s.execute("spot.candidates", &json!({})).unwrap();
    let kinds: Vec<&str> = left["candidates"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["person", "person"], "the people found before are still listed: {left}");
    assert!(!spots(&s).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
