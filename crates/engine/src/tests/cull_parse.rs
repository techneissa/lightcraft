//! Fork feature (`fork/ai-culling.md`): Assisted Culling reads the eyes of a photo's people parse when it has one, so a
//! parsed photo runs no model, even when the run starts before the parse-ahead index is read; a parse that has no
//! face mesh is not taken for one.

use std::path::Path;
use std::sync::Arc;

use lightcraft_parsing::{Landmark, ModelSet, NormRect, Parsing, Person};
use serde_json::json;

use super::parse_ahead::{fake_parse, library, open, settle};
use crate::Session;
use crate::people_parsing::Runner;

/// A parse whose one face has a face mesh (478 points, all at one place: its eyes read as unknown, but it has them).
fn meshed_parse(set: ModelSet) -> Parsing {
    let person = Person {
        face: Some(NormRect::new(0.4, 0.2, 0.6, 0.5)),
        face_score: 0.9,
        landmarks: vec![Landmark { x: 0.5, y: 0.35, z: 0.0 }; 478],
        landmark_score: 0.9,
        ..Person::default()
    };
    Parsing { model_set: set.id().into(), model_version: set.version(), width: 32, height: 24, people: vec![person] }
}

/// Parse both photos of a library with `runner` (the parse-ahead queue files them and their index), then open the
/// same folder in a new session whose index is not read yet.
fn restarted(tag: &str, runner: Runner) -> (Session, Vec<lightcraft_catalog::PhotoId>) {
    let (mut s, dir, ids) = library(tag, 2);
    s.parsing.set_runner(Some(runner));
    open(&mut s, ids[0]);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["parsed"], 2, "{st}");
    drop(s);
    let mut s = Session::new().with_fs();
    let paths: Vec<String> = (0..2).map(|i| dir.join(format!("photo-{i:02}.png")).to_string_lossy().into_owned()).collect();
    s.execute("library.import", &json!({ "paths": paths })).unwrap();
    // (attached without waiting for the index: the run below starts first)
    s.parsing.set_paths(Some(dir.join("models")), Some(Path::new(&dir).join("cache")), None);
    let ids = s.visible_cloned();
    (s, ids)
}

/// A parsed photo runs no model, though the run was planned before the index was read: its eyes come from the parse
/// (no detector or face mesh is installed here, so any model run would have failed the photo).
#[test]
fn assisted_culling_runs_no_model_for_a_parsed_photo_before_the_index_is_read() {
    let runner: Runner = Arc::new(|set, _, _, _, _| Ok(meshed_parse(set)));
    let (mut s, ids) = restarted("cull-parsed", runner);
    let runs = s.parsing.runs();
    let r = s.execute("photo.analyze", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()})).unwrap();
    assert_eq!(r["eyes"]["fromParse"], 2, "{r}");
    assert_eq!((r["eyes"]["failed"].as_u64(), r["eyes"]["measured"].as_u64()), (Some(0), Some(0)), "{r}");
    assert_eq!(s.parsing.runs(), runs, "no model ran");
    for id in &ids {
        assert!(s.catalog.photo(*id).unwrap().analysis.unwrap().eyes.is_some_and(|e| e.faces == 1));
    }
}

/// A parse whose faces have no face mesh (made before landmarks were kept) says nothing about the eyes: with no
/// models the photo is "not measured" (and tried again next time), never stored as having unreadable eyes.
#[test]
fn assisted_culling_retries_a_parse_without_landmarks() {
    let runner: Runner = Arc::new(|set, _, _, _, _| Ok(fake_parse(set)));
    let (mut s, ids) = restarted("cull-no-mesh", runner);
    let r = s.execute("photo.analyze", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()})).unwrap();
    assert_eq!(r["eyes"]["fromParse"], 0, "{r}");
    for p in r["photos"].as_array().unwrap() {
        assert!(p["eyes"].is_null() && p["eyesError"].is_string(), "{p}");
    }
    for id in &ids {
        assert_eq!(s.catalog.photo(*id).unwrap().analysis.unwrap().eyes, None);
    }
}
