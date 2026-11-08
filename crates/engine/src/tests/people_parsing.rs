//! Fork feature (`fork/people-parsing.md`): parses come from the cache the second time, a waiting caller joins the
//! run already going, a missing model, a damaged cache file and a failing run are messages, the chosen set is kept,
//! and the commands refuse what they should. A fake runner stands in for the models; nothing needs the network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lightcraft_parsing::{ModelSet, NormRect, Parsing, Part, Person, Plane, RgbImage, cache};
use serde_json::json;

use super::find_removals::{session_with_wire, temp_dir};
use crate::activity::Activity;
use crate::people_parsing::{Outcome, PeopleParsing, Runner};

fn picture(seed: u8) -> RgbImage {
    let (w, h) = (24, 16);
    RgbImage::new(w, h, (0..w * h * 3).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect()).unwrap()
}

/// One person whose Hair is a square.
fn fake_parse(set: ModelSet) -> Parsing {
    let mut person = Person { face: Some(NormRect::new(0.4, 0.2, 0.6, 0.5)), face_score: 0.9, ..Person::default() };
    person.parts.insert(Part::Hair, vec![Plane { rect: NormRect::new(0.3, 0.1, 0.7, 0.3), width: 4, height: 2, data: vec![1.0; 8] }]);
    Parsing { model_set: set.id().into(), model_version: set.version(), width: 24, height: 16, people: vec![person] }
}

fn fake_runner() -> Runner {
    Arc::new(|set, _, _, _, _| Ok(fake_parse(set)))
}

fn parsing(tag: &str) -> (PeopleParsing, PathBuf, Activity) {
    let dir = temp_dir(&format!("parsing-{tag}"));
    let activity = Activity::default();
    let p = PeopleParsing::new(activity.clone());
    p.set_paths(Some(dir.join("models")), Some(dir.join("cache")), Some(dir.join("parsing-model.json")));
    (p, dir, activity)
}

#[test]
fn a_second_parse_of_the_same_photo_comes_from_the_cache() {
    let (p, dir, _) = parsing("cache");
    p.set_runner(Some(fake_runner()));
    let Outcome::Ready { parsing, cached } = p.parse(picture(1), Vec::new(), true) else { panic!("no parse") };
    assert!(!cached);
    assert_eq!(parsing.people.len(), 1);
    assert_eq!(p.runs(), 1);
    // again: from memory
    assert!(matches!(p.parse(picture(1), Vec::new(), true), Outcome::Ready { cached: true, .. }));
    assert_eq!(p.runs(), 1);
    // a new session with the same cache folder: from disk, no run
    let fresh = PeopleParsing::new(Activity::default());
    fresh.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    fresh.set_runner(Some(fake_runner()));
    let Outcome::Ready { parsing: again, cached: true } = fresh.parse(picture(1), Vec::new(), false) else { panic!("not from disk") };
    assert_eq!(fresh.runs(), 0);
    assert_eq!(again.people[0].parts[&Part::Hair][0].width, 4);
    // other pixels, or another model set: a new run
    assert!(matches!(p.parse(picture(2), Vec::new(), true), Outcome::Ready { cached: false, .. }));
    p.set_model_set(ModelSet::BiSeNet).unwrap();
    assert!(matches!(p.parse(picture(1), Vec::new(), true), Outcome::Ready { cached: false, .. }));
    assert_eq!(p.runs(), 3);
}

#[test]
fn a_waiting_caller_joins_the_run_going_and_the_run_has_an_activity_row() {
    let (p, _, activity) = parsing("join");
    let gate = Arc::new(AtomicBool::new(false));
    let g = gate.clone();
    p.set_runner(Some(Arc::new(move |set, _, _, _, _| {
        while !g.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(fake_parse(set))
    })));
    assert!(matches!(p.parse(picture(3), Vec::new(), false), Outcome::Running));
    assert!(activity.list().iter().any(|t| t.kind == "parsing"), "{:?}", activity.list());
    assert!(matches!(p.parse(picture(3), Vec::new(), false), Outcome::Running));
    let waiter = {
        let p = p.clone();
        std::thread::spawn(move || p.parse(picture(3), Vec::new(), true))
    };
    gate.store(true, Ordering::Release);
    assert!(matches!(waiter.join().unwrap(), Outcome::Ready { .. }));
    assert_eq!(p.runs(), 1);
    // the row goes when the run ends
    for _ in 0..400 {
        if activity.list().iter().all(|t| t.kind != "parsing") {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(activity.list().iter().all(|t| t.kind != "parsing"));
}

#[test]
fn no_model_a_failing_run_and_a_damaged_cache_file_are_messages() {
    let (p, dir, _) = parsing("errors");
    let Outcome::Failed(e) = p.parse(picture(4), Vec::new(), true) else { panic!("parsed without models") };
    assert!(e.contains("not downloaded") && e.contains("parsing.download"), "{e}");
    p.set_runner(Some(Arc::new(|_, _, _, _, _| -> Result<Parsing, String> { panic!("boom") })));
    let Outcome::Failed(e) = p.parse(picture(4), Vec::new(), true) else { panic!("a panic became a parse") };
    assert!(e.contains("unexpectedly"), "{e}");
    assert!(p.message().is_some());
    p.set_runner(Some(Arc::new(|_, _, _, _, _| Err("cancelled".to_string()))));
    assert!(matches!(p.parse(picture(4), Vec::new(), true), Outcome::Failed(_)));
    // a damaged file in the cache is parsed again, and replaced
    p.set_runner(Some(fake_runner()));
    let key = cache::key(&picture(5), &[], &p.model_set().version());
    std::fs::create_dir_all(dir.join("cache")).unwrap();
    std::fs::write(cache::path(&dir.join("cache"), &key), b"LCPARSE1 broken").unwrap();
    assert!(matches!(p.parse(picture(5), Vec::new(), true), Outcome::Ready { cached: false, .. }));
    assert!(cache::load(&dir.join("cache"), &key).unwrap().is_some());
}

#[test]
fn the_chosen_set_is_kept_and_installed_files_count() {
    let (p, dir, _) = parsing("choice");
    assert_eq!(p.model_set(), ModelSet::Segformer);
    // with only BiSeNet's files in place it is the default
    for f in ModelSet::BiSeNet.files() {
        std::fs::create_dir_all(dir.join("models").join(f.folder)).unwrap();
        let file = std::fs::File::create(f.path_in(&dir.join("models"))).unwrap();
        file.set_len(f.size).unwrap();
    }
    assert!(p.installed(ModelSet::BiSeNet) && !p.installed(ModelSet::Segformer));
    assert_eq!(p.model_set(), ModelSet::BiSeNet);
    p.set_model_set(ModelSet::Segformer).unwrap();
    let again = PeopleParsing::new(Activity::default());
    again.set_paths(Some(dir.join("models")), None, Some(dir.join("parsing-model.json")));
    assert_eq!(again.model_set(), ModelSet::Segformer);
    // a damaged choice file is no choice
    std::fs::write(dir.join("parsing-model.json"), b"{nope").unwrap();
    again.set_paths(Some(dir.join("models")), None, Some(dir.join("parsing-model.json")));
    assert_eq!(again.model_set(), ModelSet::BiSeNet);
    // asking to download what is there is a message
    assert!(again.start_download(ModelSet::BiSeNet).unwrap_err().contains("already"));
}

#[test]
fn the_commands_check_what_they_are_given() {
    let dir = temp_dir("parsing-commands");
    let mut s = session_with_wire(&dir);
    s.parsing.set_paths(Some(dir.join("models")), Some(dir.join("cache")), Some(dir.join("choice.json")));
    let status = s.execute("parsing.status", &json!({})).unwrap();
    assert_eq!(status["sets"].as_array().unwrap().len(), 2);
    assert_eq!(status["modelSet"], "segformer");
    assert!(s.execute("parsing.modelSet", &json!({"set": "nope"})).is_err());
    assert_eq!(s.execute("parsing.modelSet", &json!({"set": "bisenet"})).unwrap()["modelSet"], "bisenet");
    let refused = s.execute("parsing.download", &json!({})).unwrap_err().to_string();
    assert!(refused.contains("MB") && refused.contains("confirm"), "{refused}");
    assert!(s.execute("parsing.download", &json!({"set": "x", "confirm": true})).is_err());
    // no models: the run says so
    let r = s.execute("parsing.run", &json!({"wait": true})).unwrap();
    assert_eq!(r["photos"][0]["state"], "failed", "{r}");
    // with a (fake) model: a parse, then the cache; and a mask file
    let seen: Arc<Mutex<Vec<usize>>> = Arc::default();
    let log = seen.clone();
    s.parsing.set_runner(Some(Arc::new(move |set, _, img, _, _| {
        if let Ok(mut l) = log.lock() {
            l.push(img.width.max(img.height));
        }
        Ok(fake_parse(set))
    })));
    let r = s.execute("parsing.run", &json!({"wait": true})).unwrap();
    assert_eq!(r["photos"][0]["state"], "ready", "{r}");
    assert_eq!(r["photos"][0]["cached"], false);
    assert!(r["photos"][0]["parse"]["people"][0]["parts"]["hair"].as_f64().unwrap() > 0.0);
    let r = s.execute("parsing.run", &json!({})).unwrap();
    assert_eq!(r["photos"][0]["cached"], true, "{r}");
    assert_eq!(seen.lock().unwrap().as_slice(), &[crate::people_parsing::PARSE_EDGE]);
    let out = dir.join("hair.png");
    let m = s.execute("parsing.mask", &json!({"part": "hair", "path": out.to_str().unwrap(), "edge": 300})).unwrap();
    assert_eq!((m["width"].as_u64(), m["height"].as_u64()), (Some(300), Some(200)));
    assert!(out.is_file());
    assert!(s.execute("parsing.mask", &json!({"part": "hair", "person": 5, "path": out.to_str().unwrap()})).is_err());
    assert!(s.execute("parsing.mask", &json!({"part": "elbows", "path": out.to_str().unwrap()})).is_err());
    assert!(s.execute("parsing.cancel", &json!({})).is_ok());
}

/// Opt-in, over the network: `LIGHTCRAFT_PARSING_DOWNLOAD_TEST=<empty folder>` downloads both sets there and checks
/// that every file arrived whole (size and SHA-256 are checked by the downloader).
#[test]
fn both_sets_download_and_check_their_files() {
    let Some(dir) = std::env::var_os("LIGHTCRAFT_PARSING_DOWNLOAD_TEST").filter(|v| !v.is_empty()).map(PathBuf::from) else {
        lightcraft_testkit::skip!(Input, "set LIGHTCRAFT_PARSING_DOWNLOAD_TEST to an empty folder to download the people parsing models");
        return;
    };
    let p = PeopleParsing::new(Activity::default());
    p.set_paths(Some(dir), None, None);
    for set in ModelSet::ALL {
        if !p.installed(set) {
            p.start_download(set).unwrap();
        }
        // a guard against a hung download only, not a speed check
        for _ in 0..36_000 {
            if !matches!(p.download_state(set), crate::people_parsing::Download::Running { .. }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(p.installed(set), "{set:?}: {:?}", p.download_state(set));
    }
}
