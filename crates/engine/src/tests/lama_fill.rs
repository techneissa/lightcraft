//! Fork feature (`fork/lama-fill.md`): the `fill` of a spot, the LaMa provider without and with its model, the
//! download's refusals. Nothing here needs the network; the one test that runs the real model skips without it.

use std::path::PathBuf;
use std::sync::Arc;

use lightcraft_develop::{SpotFill, SpotMode};
use lightcraft_pipeline::learned_fill::LearnedFill;
use serde_json::json;

use super::find_removals::{session_with_wire, temp_dir};
use crate::lama_fill::{LamaFill, MODEL_FILE};
use crate::{ErrorKind, Session};

fn spots(s: &Session) -> Vec<lightcraft_develop::Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

fn fill_with_dir(tag: &str) -> (LamaFill, PathBuf, crate::activity::Activity) {
    let dir = temp_dir(tag);
    let activity = crate::activity::Activity::default();
    let f = LamaFill::new(activity.clone());
    f.set_dir(Some(dir.clone()));
    (f, dir, activity)
}

fn picture(w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let mut rgb = Vec::new();
    for y in 0..h {
        for x in 0..w {
            rgb.extend_from_slice(&[x as f32 / w as f32, y as f32 / h as f32, 0.4]);
        }
    }
    let mut hole = vec![0.0; w * h];
    for y in h / 3..2 * h / 3 {
        for x in w / 3..2 * w / 3 {
            hole[y * w + x] = 1.0;
        }
    }
    (rgb, hole)
}

#[test]
fn the_fill_is_stored_with_each_spot_and_survives_a_round_trip() {
    let mut s = Session::with_demo();
    let a = s.execute("spot.add", &json!({"mode": "fill", "points": [[0.3, 0.3]], "size": 0.05})).unwrap();
    assert!(a.get("fill").is_none(), "Classic adds nothing to the answer: {a}");
    let b = s.execute("spot.add", &json!({"mode": "fill", "points": [[0.6, 0.6]], "size": 0.05, "fill": "lama"})).unwrap();
    assert_eq!(b["fill"], "lama");
    assert!(b["note"].as_str().unwrap().contains("not installed"), "a spot without the model says so: {b}");
    let c = s.execute("spot.add", &json!({"mode": "remove", "points": [[0.8, 0.2]], "fill": "LaMa"})).unwrap();
    assert_eq!(c["index"], 2);
    assert_eq!(spots(&s).iter().map(|sp| sp.fill).collect::<Vec<_>>(), [SpotFill::Classic, SpotFill::Lama, SpotFill::Lama]);
    // each keeps its own, and the file form reads back the same (Classic is not written at all)
    let d = s.develop_of(s.active().unwrap()).unwrap();
    let value = serde_json::to_value(&*d).unwrap();
    assert!(value["spots"][0].get("fill").is_none() && value["spots"][1]["fill"] == "lama", "{value}");
    let back: lightcraft_develop::DevelopSettings = serde_json::from_value(value).unwrap();
    assert_eq!(back.spots, d.spots);
    // changing one spot's fill leaves the others alone, in one undo step
    let commits = s.commits();
    s.execute("spot.update", &json!({"index": 1, "fill": "classic"})).unwrap();
    assert_eq!(spots(&s).iter().map(|sp| sp.fill).collect::<Vec<_>>(), [SpotFill::Classic, SpotFill::Classic, SpotFill::Lama]);
    assert_eq!(s.commits(), commits + 1);
    s.undo_step().unwrap();
    assert_eq!(spots(&s)[1].fill, SpotFill::Lama);
}

#[test]
fn the_fill_parameter_is_validated() {
    let mut s = Session::with_demo();
    for bad in [json!("sharp"), json!(3), json!(true), json!(["lama"])] {
        let e = s.execute("spot.add", &json!({"points": [[0.5, 0.5]], "fill": bad})).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::BadParams, "{bad}");
    }
    // LaMa takes Remove and Content-Aware spots only
    for mode in ["heal", "clone"] {
        let e = s.execute("spot.add", &json!({"mode": mode, "points": [[0.5, 0.5]], "source": [0.1, 0.0], "fill": "lama"})).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::BadParams, "{mode}");
    }
    assert!(spots(&s).is_empty(), "a refused spot is not added");
    s.execute("spot.add", &json!({"mode": "heal", "points": [[0.5, 0.5]], "source": [0.1, 0.0]})).unwrap();
    assert!(s.execute("spot.update", &json!({"fill": "lama"})).is_err());
    assert!(s.execute("spot.update", &json!({"fill": "nope"})).is_err());
    assert_eq!(spots(&s)[0].fill, SpotFill::Classic);
    // a null fill is no fill
    s.execute("spot.add", &json!({"points": [[0.2, 0.2]], "fill": null})).unwrap();
    assert_eq!(spots(&s)[1].fill, SpotFill::Classic);
}

#[test]
fn remove_all_uses_the_fill_it_is_given() {
    let dir = temp_dir("remove-all-fill");
    let mut s = session_with_wire(&dir);
    s.execute("spot.findWires", &json!({})).unwrap();
    // a bad fill is refused before anything is taken from the list
    assert!(s.execute("spot.removeCandidates", &json!({"all": true, "fill": "magic"})).is_err());
    assert!(spots(&s).is_empty());
    assert_eq!(s.execute("spot.candidates", &json!({})).unwrap()["candidates"].as_array().unwrap().len(), 1, "the found wire is still listed");
    let r = s.execute("spot.removeCandidates", &json!({"all": true, "fill": "lama"})).unwrap();
    assert_eq!(r["fill"], "lama");
    assert!(!spots(&s).is_empty() && spots(&s).iter().all(|sp| sp.fill == SpotFill::Lama && sp.mode == SpotMode::Fill));
    // and without `fill`, Classic, as before
    let mut again = session_with_wire(&temp_dir("remove-all-classic"));
    again.execute("spot.findWires", &json!({})).unwrap();
    let r = again.execute("spot.removeCandidates", &json!({"all": true})).unwrap();
    assert!(r.get("fill").is_none() && !spots(&again).is_empty() && spots(&again).iter().all(|sp| sp.fill == SpotFill::Classic));
}

#[test]
fn without_the_model_a_lama_spot_shows_the_classic_fill_and_says_why() {
    let (fill, _dir, activity) = fill_with_dir("no-model");
    assert!(!fill.installed());
    let (rgb, hole) = picture(64, 64);
    assert!(fill.fill(&rgb, &hole, 64, 64, false).is_none());
    let message = fill.message().expect("a message");
    assert!(message.contains("not installed") && message.contains(MODEL_FILE), "{message}");
    assert!(activity.list().is_empty(), "no row for a run that never started");
    // a session with the same folder draws the Classic fill for a LaMa spot, exactly as for a Classic one
    let mut classic = Session::with_demo();
    let mut lama = Session::with_demo();
    classic.execute("spot.add", &json!({"mode": "fill", "points": [[0.5, 0.5]], "size": 0.06})).unwrap();
    lama.execute("spot.add", &json!({"mode": "fill", "points": [[0.5, 0.5]], "size": 0.06, "fill": "lama"})).unwrap();
    let (a, b) = (classic.render_now(classic.active().unwrap(), 300, 200).unwrap().image, lama.render_now(lama.active().unwrap(), 300, 200).unwrap().image);
    assert_eq!(a.data, b.data);
}

#[test]
fn a_damaged_model_is_a_message_not_a_crash() {
    let (fill, dir, activity) = fill_with_dir("damaged");
    std::fs::write(dir.join(MODEL_FILE), b"this is not a model").unwrap();
    assert!(fill.installed());
    let (rgb, hole) = picture(64, 64);
    assert!(fill.fill(&rgb, &hole, 64, 64, false).is_none());
    assert!(fill.message().unwrap().contains("Classic"), "{:?}", fill.message());
    assert!(activity.list().is_empty(), "the row goes when the run ends");
    assert_eq!(fill.cached_fills(), 0);
}

#[test]
fn drafts_start_nothing_and_cached_fills_come_back_at_once() {
    let (fill, dir, activity) = fill_with_dir("drafts");
    std::fs::write(dir.join(MODEL_FILE), b"x").unwrap();
    let (rgb, hole) = picture(48, 48);
    assert!(fill.fill(&rgb, &hole, 48, 48, true).is_none());
    assert!(fill.message().is_none(), "a draft that has no fill yet is not an error");
    assert!(activity.list().is_empty());
    // wrong buffer sizes and empty pictures are refused quietly
    assert!(fill.fill(&rgb[..10], &hole, 48, 48, false).is_none());
    assert!(fill.fill(&[], &[], 0, 0, false).is_none());
}

#[test]
fn the_cache_key_of_a_photo_changes_only_when_lama_spots_meet_a_new_model() {
    let (fill, dir, _) = fill_with_dir("salt");
    let plain = lightcraft_develop::DevelopSettings::default();
    let with_spot = lightcraft_develop::DevelopSettings {
        spots: vec![lightcraft_develop::Spot { mode: SpotMode::Fill, fill: SpotFill::Lama, ..Default::default() }],
        ..Default::default()
    };
    assert_eq!(fill.cache_salt(&plain), 0);
    let before = fill.cache_salt(&with_spot);
    assert_ne!(before, 0);
    std::fs::write(dir.join(MODEL_FILE), b"x").unwrap();
    assert_ne!(fill.cache_salt(&with_spot), before, "installing the model redraws the spots");
    assert_eq!(fill.cache_salt(&plain), 0);
}

#[test]
fn the_download_refuses_what_it_should() {
    let (fill, dir, _) = fill_with_dir("download");
    std::fs::write(dir.join(MODEL_FILE), b"x").unwrap();
    assert!(fill.start_download().unwrap_err().contains("already installed"));
    let mut s = Session::with_demo();
    // no model folder (a bare session): nothing to download into
    let e = s.execute("lama.download", &json!({})).unwrap_err();
    assert!(e.to_string().contains("no model folder"), "{e}");
    let status = s.execute("lama.status", &json!({})).unwrap();
    assert_eq!((status["installed"].as_bool(), status["download"]["state"].as_str()), (Some(false), Some("idle")));
    assert_eq!(status["downloadBytes"], 205_803_670_u64);
}

#[test]
fn the_model_is_looked_for_in_the_shared_folder() {
    let dir = crate::lama_fill::default_lama_dir();
    // (whatever this computer's setup, the answer ends in a folder named lama)
    assert!(dir.is_none_or(|d| d.ends_with("lama")) || std::env::var_os("LIGHTCRAFT_LAMA_DIR").is_some());
}

/// The real model, when this computer has it (`LIGHTCRAFT_LAMA_DIR` or the shared folder): a hole is rebuilt, the rest of
/// the picture is not touched, and the same picture asked again comes from the cache.
#[test]
fn the_real_model_fills_a_hole_once() {
    let fill = LamaFill::new(crate::activity::Activity::default());
    fill.set_dir(crate::lama_fill::default_lama_dir());
    if !fill.installed() {
        lightcraft_testkit::skip!(Model, "no big-lama.pt in the LaMa model folder");
        return;
    }
    let (w, h) = (96, 96);
    let (rgb, hole) = picture(w, h);
    let out = fill.fill(&rgb, &hole, w, h, false).expect("a fill");
    assert_eq!(out.len(), rgb.len());
    let inside = (h / 2 * w + w / 2) * 3;
    assert_ne!(&out[inside..inside + 3], &rgb[inside..inside + 3], "the hole was rebuilt");
    assert_eq!(&out[..3], &rgb[..3], "the rest is the picture");
    assert_eq!(fill.cached_fills(), 1);
    let again: Arc<dyn LearnedFill> = Arc::new(fill.clone());
    assert_eq!(again.fill(&rgb, &hole, w, h, true), Some(out), "a draft takes the cached fill");
}
