//! The early check for faces (`parse_ahead/peek.rs`, issue #167): a photo that has no face is known to have none as it
//! opens, from the parse the index knows (no model) or from the face detector alone; nothing runs when parsing ahead is
//! off, in Low Power Mode or while the user works. A fake detector and a fake parse runner stand in for the models.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

#[test]
fn a_photo_without_faces_is_known_to_have_none_from_the_detector_alone_and_checked_once() {
    let (mut s, _dir, ids) = library("peek-detect", 2);
    let runs = with_detector(&mut s, 0);
    open(&mut s, ids[0]);
    assert!(!s.retouch_no_people(ids[0]), "not checked yet");
    pump_until(&mut s, "light", "the photo known to have no face", |s| s.retouch_no_people(ids[0]));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(s.retouch_faces.started(), 0, "no Portrait search ran: the tab reuses the answer");
    // the next pumps check nothing again; the other photo is not checked while it is not open
    for _ in 0..5 {
        pump(&mut s, "light");
    }
    assert_eq!((runs.load(Ordering::SeqCst), s.parse_ahead_peeks()), (1, 1));
    // opening the Portrait tab's search finds it kept
    assert_eq!(s.retouch_search(ids[0], false), None);
    assert_eq!(s.retouch_faces.started(), 0);
    // another photo opens: its check runs
    open(&mut s, ids[1]);
    pump_until(&mut s, "light", "the other photo known to have no face", |s| s.retouch_no_people(ids[1]));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
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
    pump_until(&mut s, "light", "the photo known to have no face", |s| s.retouch_no_people(ids[0]));
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
