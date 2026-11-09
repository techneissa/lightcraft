//! The early check for faces (`parse_ahead/peek.rs`, issue #167): a photo that has no face is seen to have none as it
//! opens, from the parse the index knows (no model) or from the face detector alone, as a hint the face search,
//! renders and exports ignore; nothing runs when parsing ahead is off, in Low Power Mode or while the user works. A
//! fake detector (or the 64-pixel stand-in for YuNet, which finds nobody) and a fake parse runner stand in for the
//! models.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::retouch::{Face, NormRect};
use lightcraft_parsing::{Parsing, cache};
use serde_json::{Value, json};

use super::parse_ahead::{fake_parse, library, open};
use crate::Session;
use crate::parse_ahead::power::Power;
use crate::retouch_input::FaceProvider;

/// A detector that finds `faces` faces in any picture and counts its runs.
struct Detector {
    faces: usize,
    runs: Arc<AtomicUsize>,
}

impl FaceProvider for Detector {
    fn name(&self) -> &str {
        "Fake Detector"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok((0..self.faces).map(|i| Face::boxed(NormRect::new(0.1 + 0.3 * i as f64, 0.2, 0.3 + 0.3 * i as f64, 0.5))).collect())
    }
}

fn with_detector(s: &mut Session, faces: usize) -> Arc<AtomicUsize> {
    let runs = Arc::new(AtomicUsize::new(0));
    s.retouch_faces.set_provider(Arc::new(Detector { faces, runs: runs.clone() }));
    runs
}

fn pump(s: &mut Session, pace: &str) -> Value {
    s.execute("parsing.ahead.pump", &json!({"pace": pace})).unwrap()
}

/// Pump (as the frontend does every frame) until `cond` holds; a hang guard only.
fn pump_until(s: &mut Session, pace: &str, what: &str, mut cond: impl FnMut(&Session) -> bool) {
    for _ in 0..6000 {
        pump(s, pace);
        if cond(s) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never happened: {what}");
}

/// Wait for photo `id`'s face search to end with an answer.
fn until_found(s: &mut Session, id: lightcraft_catalog::PhotoId) {
    for _ in 0..6000 {
        if s.retouch_faces_of(id).found.is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never happened: the face search ending");
}

#[test]
fn a_photo_without_faces_is_hinted_from_the_detector_alone_and_checked_once() {
    let (mut s, _dir, ids) = library("peek-detect", 2);
    let runs = with_detector(&mut s, 0);
    open(&mut s, ids[0]);
    assert!(!s.retouch_no_people(ids[0]), "not checked yet");
    pump_until(&mut s, "light", "the photo seen to have no face", |s| s.retouch_no_people(ids[0]));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // a hint, not a search result: nothing was kept as faces, and no search ran
    let st = s.retouch_faces_of(ids[0]);
    assert!(st.found.is_none() && st.hint && !st.busy, "the early check's answer is a hint");
    assert_eq!(s.retouch_faces.started(), 0);
    // the next pumps check nothing again
    for _ in 0..5 {
        pump(&mut s, "light");
    }
    assert_eq!((runs.load(Ordering::SeqCst), s.parse_ahead_peeks()), (1, 1));
    // the face search does not take the hint for its answer: it runs, and its answer replaces the hint
    assert_eq!(s.retouch_search(ids[0], false), None);
    until_found(&mut s, ids[0]);
    assert_eq!(runs.load(Ordering::SeqCst), 2, "the real search ran the detector");
    let st = s.retouch_faces_of(ids[0]);
    assert!(st.no_people() && !st.hint, "a real answer now");
    // another photo opens: its check runs
    open(&mut s, ids[1]);
    pump_until(&mut s, "light", "the other photo seen to have no face", |s| s.retouch_no_people(ids[1]));
    assert_eq!(runs.load(Ordering::SeqCst), 3);
}

#[test]
fn a_photo_with_faces_keeps_no_answer_and_is_checked_once() {
    let (mut s, _dir, ids) = library("peek-faces", 1);
    let runs = with_detector(&mut s, 2);
    open(&mut s, ids[0]);
    pump_until(&mut s, "light", "the check ending", |s| {
        runs.load(Ordering::SeqCst) == 1 && s.parse_ahead_peeks() == 1 && !s.retouch_faces_of(ids[0]).busy
    });
    for _ in 0..20 {
        pump(&mut s, "light");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!((runs.load(Ordering::SeqCst), s.parse_ahead_peeks()), (1, 1), "checked again");
    let st = s.retouch_faces_of(ids[0]);
    assert!(st.found.is_none() && !st.no_people(), "a photo with faces is not marked");
    // the face search itself still finds them, with the tab
    s.retouch_detect_now(ids[0], false).unwrap();
    assert_eq!(s.retouch_faces_of(ids[0]).found.map(|f| f.input.faces.len()), Some(2));
}

#[test]
fn a_parse_the_index_knows_answers_without_a_model_or_a_detector() {
    let (mut s, dir, ids) = library("peek-parse", 1);
    let runs = with_detector(&mut s, 3);
    // (a detector that would find faces: only the parse may answer)
    let seen = Arc::new(AtomicUsize::new(0));
    let counted = seen.clone();
    s.parsing.set_runner(Some(Arc::new(move |set, _, _, _, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(fake_parse(set))
    })));
    let version = s.parsing.version();
    let key = cache::key(&lightcraft_parsing::RgbImage::new(1, 1, vec![3; 3]).unwrap(), &[], &version);
    let nobody = Parsing { people: Vec::new(), ..fake_parse(s.parsing.model_set()) };
    cache::store(&dir.join("cache"), &key, &nobody).unwrap();
    crate::parse_ahead::remember(&mut s, ids[0], crate::people_parsing::Filed { version, key });
    open(&mut s, ids[0]);
    pump_until(&mut s, "light", "the photo seen to have no face", |s| s.retouch_no_people(ids[0]));
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the detector ran");
    assert_eq!(seen.load(Ordering::SeqCst), 0, "a model ran");
}

#[test]
fn nothing_is_checked_when_parsing_ahead_is_off_on_low_power_or_while_the_user_works() {
    let (mut s, _dir, ids) = library("peek-off", 1);
    let runs = with_detector(&mut s, 0);
    open(&mut s, ids[0]);
    s.set_parse_ahead(false);
    for _ in 0..10 {
        pump(&mut s, "light");
    }
    assert_eq!(s.parse_ahead_peeks(), 0, "started while off");
    assert!(!s.retouch_no_people(ids[0]));
    // on again, but in Low Power Mode, and on a low battery
    s.set_parse_ahead(true);
    for power in [Power { low_power: true, ..Power::default() }, Power { on_battery: true, percent: Some(20), low_power: false }] {
        s.parse_ahead.power.fix(Some(power));
        for _ in 0..10 {
            pump(&mut s, "light");
        }
        assert_eq!(s.parse_ahead_peeks(), 0, "started with {power:?}");
    }
    // the user working (a drag, typing, scrolling): the pace is pause
    s.parse_ahead.power.fix(Some(Power::default()));
    for _ in 0..10 {
        pump(&mut s, "pause");
    }
    assert_eq!(s.parse_ahead_peeks(), 0, "started while the user works");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the detector ran");
    pump_until(&mut s, "light", "the check once nothing holds it back", |s| s.retouch_no_people(ids[0]));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_photo_with_face_regions_is_left_to_the_face_search() {
    let (mut s, _dir, ids) = library("peek-regions", 1);
    let runs = with_detector(&mut s, 0);
    let mut meta = s.catalog.photo(ids[0]).unwrap().meta.clone();
    meta.regions.push(lightcraft_meta::Region {
        rect: lightcraft_geom::Rect { x0: 0.3, y0: 0.2, x1: 0.6, y1: 0.6 },
        kind: lightcraft_meta::RegionKind::Face,
        name: None,
        description: None,
    });
    s.apply_system(lightcraft_catalog::Op::SetMeta { id: ids[0], meta: Box::new(meta) }).unwrap();
    open(&mut s, ids[0]);
    for _ in 0..10 {
        pump(&mut s, "light");
    }
    assert_eq!((s.parse_ahead_peeks(), runs.load(Ordering::SeqCst)), (0, 0));
}

/// Only one test at a time uses the process's one loaded YuNet.
static YUNET: Mutex<()> = Mutex::new(());

/// The 64-pixel YuNet stand-in (it finds nobody) installed as the session's face detector.
fn tiny_yunet(s: &mut Session, dir: &Path) {
    let home = dir.join("faces").join(lightcraft_faces::known::YUNET_ID);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("model.onnx"), lightcraft_faces::synthetic::tiny_detector_model()).unwrap();
    std::fs::write(home.join("face-model.json"), b"{}").unwrap();
    s.face_models_dir = Some(dir.join("faces"));
    s.face_shared_dir = None;
}

fn retouch_on(s: &mut Session, id: lightcraft_catalog::PhotoId) {
    s.selection = crate::Selection::single(id);
    s.execute("develop.set", &json!({"control": "retouch.skinTone.faceLight", "value": 100.0})).unwrap();
}

/// The people parse is installed and YuNet misses the face: the early check hints "nobody", and still the face search
/// (Search Again, `retouch.detect`, an export) goes by the parse and finds the face; the hint never decides a search
/// or a render.
#[test]
fn a_photo_yunet_misses_but_the_parse_finds_is_still_searched_and_retouched() {
    let _one = YUNET.lock().unwrap_or_else(PoisonError::into_inner);
    let (mut s, dir, ids) = library("peek-miss", 2);
    tiny_yunet(&mut s, &dir);
    s.parsing.set_runner(Some(Arc::new(|set, _, _, _, _| Ok(fake_parse(set)))));
    for id in &ids {
        open(&mut s, *id);
        pump_until(&mut s, "light", "the photo hinted", |s| s.retouch_faces_of(*id).hint);
        assert!(s.retouch_no_people(*id), "the hint dims the icon");
    }
    // Search Again: the real search, from the parse
    s.execute("retouch.search", &json!({"id": ids[0].0, "again": true})).unwrap();
    until_found(&mut s, ids[0]);
    let st = s.retouch_faces_of(ids[0]);
    assert_eq!(st.found.as_ref().map(|f| f.input.faces.len()), Some(1), "the parse's face");
    assert!(!st.no_people() && !st.hint, "the icon is normal again");
    // an export after a hint: the face is found first and retouching applies to it
    retouch_on(&mut s, ids[1]);
    assert!(s.retouch_faces_of(ids[1]).hint);
    s.retouch_before_export(ids[1]);
    let job = s.render_job(ids[1], 48, 48, false, false).unwrap();
    assert_eq!(job.info.retouch.as_ref().map(|r| r.faces.len()), Some(1), "the export renders without the face the parse finds");
    assert!(!s.retouch_no_people(ids[1]));
}

/// A session builds YuNet once, however many photos open.
#[test]
fn yunet_is_loaded_once_across_photo_opens() {
    let _one = YUNET.lock().unwrap_or_else(PoisonError::into_inner);
    let (mut s, dir, ids) = library("peek-once", 3);
    tiny_yunet(&mut s, &dir);
    let before = s.face_detector_loads();
    for id in &ids {
        open(&mut s, *id);
        pump_until(&mut s, "light", "the photo hinted", |s| s.retouch_faces_of(*id).hint);
    }
    assert_eq!(s.face_detector_loads() - before, 1, "YuNet was built again");
    // the session's own use finds the net the worker loaded (and offers it the GPU)
    let d = crate::cmd::face_detect::detector(&s).unwrap();
    assert_eq!(s.face_detector_loads() - before, 1);
    drop(d);
}
