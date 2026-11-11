//! Fork feature (`fork/parse-ahead.md`): people parsing ahead of time. The queue's order, the pause while the user
//! works (and while the photo on screen is being made), the row's ✕, the off setting, power, photos skipped because
//! their parse is cached (also after a restart), a parsed photo opening without a model run, and the cache keeping the
//! parses used most recently. A fake runner stands in for the models; no test waits on a clock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

use lightcraft_catalog::PhotoId;
use lightcraft_meta::{Region, RegionKind};
use lightcraft_parsing::{ModelSet, NormRect, Parsing, Part, Person, Plane, cache};
use serde_json::{Value, json};

use super::find_removals::temp_dir;
use crate::Session;
use crate::parse_ahead::power::Power;
use crate::people_parsing::Runner;

/// Guards against a hang only: a step that has not happened after this many polls fails the test.
const POLLS: usize = 6000;
const POLL: Duration = Duration::from_millis(5);

fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..POLLS {
        if cond() {
            return;
        }
        std::thread::sleep(POLL);
    }
    panic!("never happened: {what}");
}

/// Pump at `p` until `cond` holds (as the frontend pumps every frame), guarding against a hang only.
fn pump_until(s: &mut Session, p: Value, what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..POLLS {
        s.execute("parsing.ahead.pump", &p).unwrap();
        if cond() {
            return;
        }
        std::thread::sleep(POLL);
    }
    panic!("never happened: {what}");
}

pub(super) fn fake_parse(set: ModelSet) -> Parsing {
    let mut person = Person { face: Some(NormRect::new(0.4, 0.2, 0.6, 0.5)), face_score: 0.9, ..Person::default() };
    person.parts.insert(Part::Hair, vec![Plane { rect: NormRect::new(0.3, 0.1, 0.7, 0.3), width: 4, height: 2, data: vec![1.0; 8], split: false }]);
    Parsing { model_set: set.id().into(), model_version: set.version(), width: 32, height: 24, people: vec![person] }
}

/// What the fake model saw: the first pixel of each picture it parsed, in order.
type Seen = Arc<Mutex<Vec<[u8; 3]>>>;

fn recording_runner(seen: &Seen) -> Runner {
    let log = seen.clone();
    Arc::new(move |set, _, img, _, _| {
        if let (Ok(mut l), Some(px)) = (log.lock(), img.data.get(..3)) {
            l.push([px[0], px[1], px[2]]);
        }
        Ok(fake_parse(set))
    })
}

/// A runner that waits for a permit (or a stop) before it finishes; `entered` counts the runs that started, `stopped`
/// the runs that ended because they were stopped.
#[derive(Clone, Default)]
struct Gate {
    permits: Arc<(Mutex<usize>, Condvar)>,
    entered: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
}

impl Gate {
    fn runner(&self) -> Runner {
        let g = self.clone();
        Arc::new(move |set, _, _, _, cancelled| {
            g.entered.fetch_add(1, Ordering::SeqCst);
            let (m, cv) = &*g.permits;
            let mut n = m.lock().unwrap();
            loop {
                if cancelled() {
                    g.stopped.fetch_add(1, Ordering::SeqCst);
                    return Err("cancelled".to_string());
                }
                if *n > 0 {
                    *n -= 1;
                    return Ok(fake_parse(set));
                }
                n = cv.wait_timeout(n, Duration::from_millis(5)).unwrap().0;
            }
        })
    }

    fn open(&self, n: usize) {
        let (m, cv) = &*self.permits;
        *m.lock().unwrap() += n;
        cv.notify_all();
    }
}

/// A small solid PNG of colour `i` in `dir`.
fn write_photo(dir: &Path, i: usize) -> PathBuf {
    let (w, h) = (48usize, 32usize);
    let c = [(20 + i * 18) as u8, 90, (220 - i * 14) as u8, 255];
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data: vec![c; w * h] }),
        &Default::default(),
    )
    .unwrap();
    let path = dir.join(format!("photo-{i:02}.png"));
    std::fs::write(&path, png).unwrap();
    path
}

fn import(s: &mut Session, paths: &[PathBuf]) {
    let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    s.execute("library.import", &json!({ "paths": paths })).unwrap();
}

/// A session with `n` photos in a scratch folder, its own parse cache, mains power, and no models (the test sets a
/// runner).
pub(super) fn library(tag: &str, n: usize) -> (Session, PathBuf, Vec<PhotoId>) {
    let dir = temp_dir(&format!("ahead-{tag}"));
    let paths: Vec<PathBuf> = (0..n).map(|i| write_photo(&dir, i)).collect();
    let mut s = Session::new().with_fs();
    import(&mut s, &paths);
    attach(&mut s, &dir);
    let ids = s.visible_cloned();
    assert_eq!(ids.len(), n);
    (s, dir, ids)
}

fn attach(s: &mut Session, dir: &Path) {
    s.parsing.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    s.parse_ahead.power.fix(Some(Power::default()));
    // (a photo open in the Edit view is parsed at once; the dwell has its own test)
    s.parse_ahead_test_dwell(Duration::ZERO);
    index_read(s);
}

fn add_face(s: &mut Session, id: PhotoId, name: Option<&str>) {
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.regions.push(Region {
        rect: lightcraft_geom::Rect { x0: 0.3, y0: 0.2, x1: 0.6, y1: 0.6 },
        kind: RegionKind::Face,
        name: name.map(str::to_string),
        description: None,
    });
    s.apply_system(lightcraft_catalog::Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
}

pub(super) fn open(s: &mut Session, id: PhotoId) {
    s.selection = crate::Selection::single(id);
}

fn pump(s: &mut Session, p: Value) -> Value {
    s.execute("parsing.ahead.pump", &p).unwrap()
}

/// Pump at `p`, waiting (a few seconds a call), until the queue is done or stands still, as an agent does.
pub(super) fn settle(s: &mut Session, mut p: Value) -> Value {
    p["wait"] = json!(true);
    for _ in 0..100 {
        let st = pump(s, p.clone());
        if !matches!(st["state"].as_str(), Some("working" | "paused" | "loading")) {
            return st;
        }
    }
    panic!("parsing ahead never settled");
}

/// Wait for the worker to read the index beside the cache (nothing is queued before).
fn index_read(s: &mut Session) {
    for _ in 0..POLLS {
        if s.parse_ahead_test_index_read() {
            return;
        }
        std::thread::sleep(POLL);
    }
    panic!("never happened: the index is read");
}

/// Each photo's first rendered pixel, as the parse sees it.
fn first_pixels(s: &mut Session, ids: &[PhotoId]) -> HashMap<[u8; 3], PhotoId> {
    let mut m = HashMap::new();
    for id in ids {
        let (rgb, _, _) = crate::cmd::face_detect::render_rgb(s, *id, crate::people_parsing::PARSE_EDGE).unwrap();
        assert!(m.insert([rgb[0], rgb[1], rgb[2]], *id).is_none(), "two photos render alike");
    }
    m
}

fn order_seen(seen: &Seen, pixels: &HashMap<[u8; 3], PhotoId>) -> Vec<PhotoId> {
    seen.lock().unwrap().iter().map(|px| pixels[px]).collect()
}

fn row(s: &Session) -> Option<crate::activity::TaskInfo> {
    s.activity.list().into_iter().find(|t| t.kind == "parseAhead")
}

/// Open photo `id` in the Edit panel and edit it (as the user does before the photos ahead are parsed): one pump sees
/// the photo, then its exposure changes.
fn edit_open(s: &mut Session, id: PhotoId) {
    open(s, id);
    pump(s, json!({"pace": "pause"}));
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.3})).unwrap();
}

#[test]
fn the_open_photo_goes_first_and_once_the_user_edits_the_photos_ahead_never_those_behind() {
    let (mut s, _dir, ids) = library("order", 10);
    for id in &ids {
        add_face(&mut s, *id, None);
    }
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    let pixels = first_pixels(&mut s, &ids);
    // looking at a photo in the Edit panel: that photo alone
    open(&mut s, ids[4]);
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(order_seen(&seen, &pixels), vec![ids[4]]);
    assert_eq!(st["editing"], false, "{st}");
    // the user edits it: the photos ahead, nearest first; none behind (they were stepped past)
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.3})).unwrap();
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["editing"], true, "{st}");
    let want: Vec<PhotoId> = [4, 5, 6, 7, 8].iter().map(|i| ids[*i]).collect();
    assert_eq!(order_seen(&seen, &pixels), want);
    // the user idle: nothing else, they kept none of the others
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!((st["parsed"].as_u64(), st["queued"].as_u64()), (Some(5), Some(0)), "{st}");
    // still editing, the next photo: the photos ahead of it (one new); stepping backward, "ahead" is before
    open(&mut s, ids[5]);
    settle(&mut s, json!({"pace": "light"}));
    assert_eq!(order_seen(&seen, &pixels).last(), Some(&ids[9]));
    open(&mut s, ids[4]);
    settle(&mut s, json!({"pace": "light", "backward": true}));
    let got = order_seen(&seen, &pixels);
    assert_eq!(got[got.len() - 4..], [ids[3], ids[2], ids[1], ids[0]]);
    assert_eq!(s.parsing.runs(), 10);
}

#[test]
fn while_culling_no_photo_stepped_past_is_parsed() {
    let (mut s, _dir, ids) = library("culling", 6);
    for id in &ids {
        add_face(&mut s, *id, None);
    }
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    // stepping through the photos outside the Edit panel, rating some, waiting on each as long as it takes
    for (k, id) in ids.iter().enumerate() {
        open(&mut s, *id);
        if k % 2 == 0 {
            s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.2})).unwrap();
        }
        let st = settle(&mut s, json!({"pace": "light", "culling": true}));
        assert_eq!((st["state"].as_str(), st["culling"].as_bool()), (Some("done"), Some(true)), "{st}");
    }
    let st = settle(&mut s, json!({"pace": "normal", "culling": true}));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(s.parsing.runs(), 0, "a photo stepped past was parsed: {st}");
    // to the Edit panel: the photo open there; the edits made while culling don't count as editing in it
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!((st["parsed"].as_u64(), st["editing"].as_bool()), (Some(1), Some(false)), "{st}");
}

#[test]
fn stepping_through_photos_in_the_edit_view_parses_none_and_drops_the_run_of_one_stepped_past() {
    let (mut s, _dir, ids) = library("dwell", 4);
    for id in &ids {
        add_face(&mut s, *id, None);
    }
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    // each photo shorter than the dwell: none parsed, no row, frames keep coming ("paused")
    s.parse_ahead_test_dwell(Duration::from_secs(3600));
    for id in &ids {
        open(&mut s, *id);
        for _ in 0..20 {
            let st = pump(&mut s, json!({"pace": "light"}));
            assert_eq!(st["state"], "paused", "{st}");
        }
    }
    assert_eq!(gate.entered.load(Ordering::SeqCst), 0, "a photo stepped past was parsed");
    assert!(row(&s).is_none());
    // a photo that stays open is parsed; stepping on before it is done stops its run, which is not run again
    s.parse_ahead_test_dwell(Duration::ZERO);
    open(&mut s, ids[0]);
    pump_until(&mut s, json!({"pace": "light"}), "the open photo's run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    s.parse_ahead_test_dwell(Duration::from_secs(3600));
    open(&mut s, ids[1]);
    pump(&mut s, json!({"pace": "light"}));
    until("the run of the photo stepped past stops", || gate.stopped.load(Ordering::SeqCst) == 1);
    gate.open(5);
    for _ in 0..40 {
        pump(&mut s, json!({"pace": "light"}));
        std::thread::sleep(POLL);
    }
    assert_eq!(gate.entered.load(Ordering::SeqCst), 1, "the photo stepped past ran again");
    assert_eq!(s.parse_ahead.counts().parsed, 0);
    // an edit counts as staying: parsed at once, the photos ahead too
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.3})).unwrap();
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!((st["state"].as_str(), st["parsed"].as_u64()), (Some("done"), Some(3)), "{st}");
}

#[test]
fn photos_with_portrait_settings_go_while_the_user_is_around_and_kept_photos_only_while_idle() {
    let (mut s, _dir, ids) = library("kept", 7);
    for id in &ids {
        add_face(&mut s, *id, None);
    }
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    let pixels = first_pixels(&mut s, &ids);
    // 1: portrait settings; 2: picked; 3: two stars; 4: three stars but rejected; 6: picked, no faces
    retouch_on(&mut s, ids[1]);
    s.apply_system(lightcraft_catalog::Op::SetFlag { id: ids[2], flag: lightcraft_catalog::Flag::Pick }).unwrap();
    s.apply_system(lightcraft_catalog::Op::SetRating { id: ids[3], rating: 2 }).unwrap();
    s.apply_system(lightcraft_catalog::Op::SetRating { id: ids[4], rating: 3 }).unwrap();
    s.apply_system(lightcraft_catalog::Op::SetFlag { id: ids[4], flag: lightcraft_catalog::Flag::Reject }).unwrap();
    s.apply_system(lightcraft_catalog::Op::SetFlag { id: ids[6], flag: lightcraft_catalog::Flag::Pick }).unwrap();
    let mut meta = s.catalog.photo(ids[6]).unwrap().meta.clone();
    meta.regions.clear();
    s.apply_system(lightcraft_catalog::Op::SetMeta { id: ids[6], meta: Box::new(meta) }).unwrap();
    s.selection = crate::Selection::default();
    // the user around (culling, nothing open in the Edit panel): the photo with portrait settings
    let st = settle(&mut s, json!({"pace": "light", "culling": true}));
    assert_eq!(order_seen(&seen, &pixels), vec![ids[1]], "{st}");
    // idle: the kept photos with faces, newest first; never the rejected one, one without faces or one not kept
    let st = settle(&mut s, json!({"pace": "normal", "culling": true}));
    assert_eq!(st["state"], "done", "{st}");
    let got = order_seen(&seen, &pixels);
    assert_eq!(got.first(), Some(&ids[1]), "{st}");
    let mut kept: Vec<PhotoId> = got[1..].to_vec();
    kept.sort_by_key(|id| id.0);
    let mut want = vec![ids[2], ids[3]];
    want.sort_by_key(|id| id.0);
    assert_eq!(kept, want, "{st}");
}

#[test]
fn photos_without_faces_wait_for_the_face_scan_except_near_the_open_photo_while_editing() {
    let (mut s, _dir, ids) = library("faces", 4);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    add_face(&mut s, ids[3], None);
    // the face scan looked at photo 1 and found no face: not parsed even beside the open photo
    s.faces.scanned.insert(ids[1].0);
    edit_open(&mut s, ids[0]);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["state"], "done", "{st}");
    let pixels = first_pixels(&mut s, &ids);
    // 0 (open, not scanned yet), 2 (near, not scanned yet), 3 (near, with a face); never 1
    let mut got = order_seen(&seen, &pixels);
    got.sort_by_key(|id| id.0);
    let mut want = vec![ids[0], ids[2], ids[3]];
    want.sort_by_key(|id| id.0);
    assert_eq!(got, want);
}

#[test]
fn the_user_working_pauses_the_queue_and_stops_the_model_run_which_runs_again_after() {
    let (mut s, _dir, ids) = library("pause", 3);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    edit_open(&mut s, ids[0]);
    pump_until(&mut s, json!({"pace": "light"}), "the model run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    let st = s.execute("parsing.ahead.status", &json!({})).unwrap();
    assert_eq!(st["working"]["id"], ids[0].0, "{st}");
    assert!(row(&s).is_some(), "one row: {:?}", s.activity.list());
    assert_eq!(s.activity.list().iter().filter(|t| t.kind == "parsing").count(), 0, "the run has no row of its own");
    // the user drags a slider: the run stops, nothing new starts
    let st = pump(&mut s, json!({"pace": "pause"}));
    assert_eq!(st["state"], "paused", "{st}");
    until("the model run stops", || gate.stopped.load(Ordering::SeqCst) == 1);
    for _ in 0..20 {
        pump(&mut s, json!({"pace": "pause"}));
    }
    assert_eq!(gate.entered.load(Ordering::SeqCst), 1, "nothing ran while paused");
    assert_eq!(s.parse_ahead.counts().parsed, 0);
    until("a stop is not news", || s.parsing.message().is_none());
    // the user lets go: the same photo runs again and finishes, then the next
    gate.open(3);
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(gate.entered.load(Ordering::SeqCst), 4, "the stopped photo ran again: {st}");
    assert_eq!(st["parsed"], 3);
    assert!(row(&s).is_none(), "the row goes when the queue is done");
}

#[test]
fn the_photo_on_screen_being_made_and_a_request_go_first() {
    let (mut s, _dir, ids) = library("foreground", 2);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    edit_open(&mut s, ids[0]);
    {
        // the loupe waits for its render: nothing starts
        let _hold = crate::prep::TestHold::new();
        let st = pump(&mut s, json!({"pace": "normal"}));
        assert_eq!(st["state"], "paused", "{st}");
        assert!(st["working"].is_null(), "{st}");
    }
    // a request for photo 1 runs while the queue holds back
    let parsing = s.parsing.clone();
    let img = lightcraft_parsing::RgbImage::new(2, 2, vec![7; 12]).unwrap();
    let request = std::thread::spawn(move || parsing.parse(img, Vec::new(), true));
    until("the request's run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    let st = pump(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["state"], "paused", "another parse goes first: {st}");
    gate.open(1);
    assert!(matches!(request.join().unwrap(), crate::people_parsing::Outcome::Ready { .. }));
    gate.open(2);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["parsed"], 2, "{st}");
}

#[test]
fn the_rows_cross_stops_the_run_and_empties_the_queue_until_another_photo_is_opened() {
    let (mut s, _dir, ids) = library("cancel", 3);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    edit_open(&mut s, ids[0]);
    pump_until(&mut s, json!({"pace": "light"}), "the model run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    let r = row(&s).expect("a row");
    assert!(r.cancellable);
    assert_eq!(r.label, "Finding people parts ahead");
    s.activity.cancel(r.id).unwrap();
    let st = pump(&mut s, json!({"pace": "light"}));
    assert_eq!(st["state"], "stopped", "{st}");
    assert_eq!(st["queued"], 0);
    until("the model run stops", || gate.stopped.load(Ordering::SeqCst) == 1);
    assert!(row(&s).is_none());
    gate.open(5);
    for _ in 0..20 {
        pump(&mut s, json!({"pace": "normal"}));
    }
    assert_eq!(gate.entered.load(Ordering::SeqCst), 1, "nothing runs after ✕");
    // another photo opened: parsing ahead starts again (it and the one ahead; the stopped one is behind)
    open(&mut s, ids[1]);
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["parsed"], 2);
}

#[test]
fn the_setting_off_and_low_power_stop_everything() {
    let (mut s, _dir, ids) = library("off", 2);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    edit_open(&mut s, ids[0]);
    let st = pump(&mut s, json!({"pace": "normal", "enabled": false}));
    assert_eq!(st["state"], "off", "{st}");
    assert_eq!(st["enabled"], false);
    for _ in 0..20 {
        pump(&mut s, json!({"pace": "normal"}));
    }
    assert_eq!(gate.entered.load(Ordering::SeqCst), 0);
    assert!(row(&s).is_none());
    // on again, then off while a run goes: it stops
    pump_until(&mut s, json!({"pace": "normal", "enabled": true}), "the model run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    let st = pump(&mut s, json!({"pace": "normal", "enabled": false}));
    assert_eq!(st["state"], "off");
    until("the model run stops", || gate.stopped.load(Ordering::SeqCst) == 1);
    // Low Power Mode and a low battery hold it back; mains power lets it run
    s.parse_ahead.power.fix(Some(Power { low_power: true, ..Power::default() }));
    assert_eq!(pump(&mut s, json!({"pace": "normal", "enabled": true}))["state"], "lowPower");
    s.parse_ahead.power.fix(Some(Power { on_battery: true, percent: Some(20), low_power: false }));
    assert_eq!(pump(&mut s, json!({"pace": "normal"}))["state"], "battery");
    assert_eq!(gate.entered.load(Ordering::SeqCst), 1);
    s.parse_ahead.power.fix(Some(Power { on_battery: true, percent: Some(80), low_power: false }));
    gate.open(2);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["parsed"], 2, "{st}");
}

#[test]
fn a_parsed_photo_opens_without_a_model_run_and_cached_photos_are_skipped_after_a_restart() {
    let (mut s, dir, ids) = library("skip", 3);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    edit_open(&mut s, ids[0]);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["parsed"], 3, "{st}");
    assert_eq!(s.parsing.runs(), 3);
    // opening a photo (a request for its masks) finds the parse ready
    for id in &ids {
        let r = s.execute("parsing.run", &json!({"ids": [id.0], "wait": true})).unwrap();
        assert_eq!(r["photos"][0]["cached"], true, "{r}");
    }
    assert_eq!(s.parsing.runs(), 3, "no model ran");
    // a restart: the index says what is parsed, so nothing is rendered or run again
    drop(s);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.join("photo-00.png").to_string_lossy(), dir.join("photo-01.png").to_string_lossy(), dir.join("photo-02.png").to_string_lossy()]})).unwrap();
    attach(&mut s, &dir);
    s.parsing.set_runner(Some(recording_runner(&seen)));
    let ids = s.visible_cloned();
    edit_open(&mut s, ids[0]);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!((st["parsed"].as_u64(), st["skipped"].as_u64()), (Some(0), Some(3)), "{st}");
    assert_eq!(s.parsing.runs(), 0);
    // a photo whose faces change is looked at again: its parse (the same picture, no YuNet) is found in the cache
    add_face(&mut s, ids[0], Some("Ann"));
    s.parse_ahead_test_rebuild();
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["cached"], 1, "{st}");
    assert_eq!(s.parsing.runs(), 0);
    // a parse that left the cache is made again
    let key = s.execute("parsing.ahead.status", &json!({})).unwrap();
    assert!(key["indexFile"].as_str().is_some_and(|f| Path::new(f).is_file()), "{key}");
    for e in std::fs::read_dir(dir.join("cache")).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == cache::EXT) {
            std::fs::remove_file(e.path()).unwrap();
        }
    }
    drop(s);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.join("photo-00.png").to_string_lossy()]})).unwrap();
    attach(&mut s, &dir);
    s.parsing.set_runner(Some(recording_runner(&seen)));
    let id = s.visible_cloned()[0];
    open(&mut s, id);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["parsed"], 1, "{st}");
}

#[test]
fn the_cache_keeps_the_parses_used_last_and_the_idle_sweep_leaves_room() {
    let (mut s, dir, ids) = library("evict", 7);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    let cache_dir = dir.join("cache");
    // three parses, made long ago in the order a, b, c
    let pics: Vec<_> = (0..3u8).map(|i| lightcraft_parsing::RgbImage::new(2, 1, vec![i; 6]).unwrap()).collect();
    let mut files = Vec::new();
    for (i, img) in pics.iter().enumerate() {
        assert!(matches!(s.parsing.parse(img.clone(), Vec::new(), true), crate::people_parsing::Outcome::Ready { .. }));
        let key = cache::key(img, &[], &s.parsing.version());
        let path = cache::path(&cache_dir, &key);
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(300 - 100 * i as u64)).unwrap();
        files.push(path);
    }
    // a is used again (opened): it becomes the newest
    assert!(matches!(s.parsing.parse(pics[0].clone(), Vec::new(), true), crate::people_parsing::Outcome::Ready { cached: true, .. }));
    let one = std::fs::metadata(&files[0]).unwrap().len();
    // room for two: the one used least recently (b) goes
    assert_eq!(cache::prune(&cache_dir, 2 * one), 1);
    assert!(files[0].is_file() && !files[1].is_file() && files[2].is_file());
    // a cache nearly full: the idle sweep queues nothing, the photos near the open one still go
    let big = cache_dir.join("filler.lcparse");
    std::fs::File::create(&big).unwrap().set_len((crate::people_parsing::cache_budget() as f64 * 0.95) as u64).unwrap();
    for id in &ids {
        add_face(&mut s, *id, None);
    }
    // (5 and 6 kept: photos for the idle sweep)
    for id in &ids[5..] {
        s.apply_system(lightcraft_catalog::Op::SetRating { id: *id, rating: 3 }).unwrap();
    }
    edit_open(&mut s, ids[0]);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["state"], "done", "{st}");
    // 0 open, 1 to 4 ahead; 5 and 6 are only for the idle sweep
    assert_eq!(st["parsed"], 5, "{st}");
    assert_eq!(st["queued"], 0);
    let _ = std::fs::remove_file(big);
}

#[test]
fn a_photo_a_request_parsed_is_skipped_and_opens_again_without_a_render() {
    let (mut s, _dir, ids) = library("request", 2);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    // the user opened photo 0 before parsing ahead ran: a request parsed it
    let r = s.execute("parsing.run", &json!({"ids": [ids[0].0], "wait": true})).unwrap();
    assert_eq!(r["photos"][0]["cached"], false, "{r}");
    edit_open(&mut s, ids[0]);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!((st["parsed"].as_u64(), st["skipped"].as_u64()), (Some(1), Some(1)), "{st}");
    assert_eq!(s.parsing.runs(), 2);
    // opening it again reads the parse by the photo's identity; a photo that changed is parsed as usual
    assert!(crate::parse_ahead::ready(&mut s, ids[0]).is_some());
    add_face(&mut s, ids[1], Some("Ann"));
    assert!(crate::parse_ahead::ready(&mut s, ids[1]).is_none(), "its faces changed");
    let r = s.execute("parsing.run", &json!({"ids": [ids[1].0], "wait": true})).unwrap();
    assert_eq!(r["photos"][0]["state"], "ready", "{r}");
    assert!(crate::parse_ahead::ready(&mut s, ids[1]).is_some(), "the request filed it");
    // another model set: nothing made by the first is taken for it
    s.parsing.set_model_set(ModelSet::BiSeNet).unwrap();
    assert!(crate::parse_ahead::ready(&mut s, ids[0]).is_none());
}

#[test]
fn a_parse_made_before_the_people_edge_changed_is_not_taken_for_the_new_edge() {
    use crate::hair_edges::{Edge, EdgeKind, Matte};
    let (mut s, _dir, ids) = library("edge", 1);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    let plain = s.parsing.version();
    // a plain parse, filed by a request; parsing ahead finds it
    let r = s.execute("parsing.run", &json!({"ids": [ids[0].0], "wait": true})).unwrap();
    assert_eq!(r["photos"][0]["parse"]["version"], plain.as_str(), "{r}");
    open(&mut s, ids[0]);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["skipped"], 1, "{st}");
    // the People edge becomes BiRefNet Matting: the plain parse is not the photo's parse any more
    s.hair_edges.set_runner(Some(Arc::new(|_, _, _, _| Ok(Matte { side: 8, logits: vec![9.0; 64] }))));
    s.hair_edges.set_edge(EdgeKind::People, Edge::Matting).unwrap();
    let matted = s.parsing.ahead_version();
    assert_eq!(matted, format!("{}+{}+{}", s.parsing.model_set().version(), crate::hair_edges::VERSION, crate::hair_edges::refine::VERSION));
    assert!(crate::parse_ahead::ready(&mut s, ids[0]).is_none(), "the plain parse is not ready for the matted request");
    // parsing ahead parses it again for the new edge, and a request then reads that parse without a render
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["parsed"], 1, "{st}");
    let r = s.execute("parsing.run", &json!({"ids": [ids[0].0], "wait": true})).unwrap();
    assert_eq!((r["photos"][0]["cached"].as_bool(), r["photos"][0]["parse"]["version"].as_str()), (Some(true), Some(matted.as_str())), "{r}");
    assert_eq!(s.parsing.runs(), 2);
}

#[test]
fn an_export_print_book_or_slideshow_holds_the_work_back_and_stops_the_model_run() {
    let (mut s, _dir, ids) = library("export", 2);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    edit_open(&mut s, ids[0]);
    {
        // (print, book and slideshow output share the export rows' kind)
        let _export = s.activity.start("export", "Printing to PDF", crate::activity::Cancel::No);
        for _ in 0..20 {
            let st = pump(&mut s, json!({"pace": "normal"}));
            assert_eq!(st["state"], "paused", "{st}");
        }
        assert_eq!(gate.entered.load(Ordering::SeqCst), 0, "nothing starts during an export");
    }
    pump_until(&mut s, json!({"pace": "normal"}), "the model run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    let export = s.activity.start("export", "Exporting", crate::activity::Cancel::No);
    pump(&mut s, json!({"pace": "normal"}));
    until("the model run stops", || gate.stopped.load(Ordering::SeqCst) == 1);
    drop(export);
    gate.open(2);
    let st = settle(&mut s, json!({"pace": "normal"}));
    assert_eq!(st["parsed"], 2, "{st}");
}

#[test]
fn labels_leave_the_queue_as_it_is_and_face_changes_rebuild_it_but_not_while_the_user_works() {
    let (mut s, _dir, ids) = library("aspects", 3);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    for id in &ids {
        add_face(&mut s, *id, None);
    }
    open(&mut s, ids[0]);
    settle(&mut s, json!({"pace": "light"}));
    let built = s.parse_ahead.rebuilds();
    // a colour label changes nothing the queue reads: no rebuild, however long ago it was built
    s.apply_system(lightcraft_catalog::Op::SetLabel { id: ids[1], label: Some(lightcraft_catalog::ColorLabel::Red) }).unwrap();
    s.parse_ahead_test_age();
    pump(&mut s, json!({"pace": "light"}));
    assert_eq!(s.parse_ahead.rebuilds(), built, "an edit rebuilt the queue");
    // a face named: rebuilt, once the user lets go
    add_face(&mut s, ids[2], Some("Ann"));
    s.parse_ahead_test_age();
    pump(&mut s, json!({"pace": "pause"}));
    assert_eq!(s.parse_ahead.rebuilds(), built, "rebuilt while the user works");
    pump(&mut s, json!({"pace": "light"}));
    assert_eq!(s.parse_ahead.rebuilds(), built + 1);
}

#[test]
fn a_request_for_another_photo_stops_the_queues_model_run_and_goes_first() {
    let (mut s, _dir, ids) = library("first", 2);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    edit_open(&mut s, ids[0]);
    pump_until(&mut s, json!({"pace": "light"}), "the queue's run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    // a synchronous request (the UI thread waits, so nothing pumps): the queue's run gives way at once
    let parsing = s.parsing.clone();
    let img = lightcraft_parsing::RgbImage::new(2, 2, vec![3; 12]).unwrap();
    let request = std::thread::spawn(move || parsing.parse(img, Vec::new(), true));
    until("the queue's run stops", || gate.stopped.load(Ordering::SeqCst) == 1);
    until("the request's run starts", || gate.entered.load(Ordering::SeqCst) == 2);
    gate.open(1);
    assert!(matches!(request.join().unwrap(), crate::people_parsing::Outcome::Ready { cached: false, .. }));
    assert!(s.parsing.message().is_none(), "a stopped queue run is not news: {:?}", s.parsing.message());
    // the queue runs its photo again afterwards
    gate.open(2);
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!(st["parsed"], 2, "{st}");
}

#[test]
fn a_request_that_joined_the_queues_run_gets_its_parse_when_that_run_is_stopped() {
    let (mut s, _dir, ids) = library("joined", 1);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    open(&mut s, ids[0]);
    pump_until(&mut s, json!({"pace": "light"}), "the queue's run starts", || gate.entered.load(Ordering::SeqCst) == 1);
    let key = s.parse_ahead_test_run().and_then(|r| r.key()).expect("the run's key");
    // a request for the same photo joins the run going
    let (img, hints) = crate::cmd::parsing::prepare_job(&mut s, ids[0]).unwrap().run().unwrap();
    let parsing = s.parsing.clone();
    let request = std::thread::spawn(move || parsing.parse(img, hints, true));
    until("the request waits on the queue's run", || s.parsing.waited_on(&key));
    // ✕ on the queue's row stops the queue's run: the request runs the photo itself
    let r = row(&s).expect("a row");
    s.activity.cancel(r.id).unwrap();
    pump(&mut s, json!({"pace": "light"}));
    until("the request's own run starts", || gate.entered.load(Ordering::SeqCst) == 2);
    gate.open(1);
    assert!(matches!(request.join().unwrap(), crate::people_parsing::Outcome::Ready { .. }), "the request failed");
    let st = s.execute("parsing.status", &json!({})).unwrap();
    assert!(st["message"].is_null(), "{st}");
}

#[test]
fn a_run_that_finished_a_moment_ago_is_not_run_again() {
    let dir = temp_dir("ahead-finished");
    let p = crate::people_parsing::PeopleParsing::new(crate::activity::Activity::default());
    p.set_paths(Some(dir.join("models")), None, None);
    let seen: Seen = Arc::default();
    p.set_runner(Some(recording_runner(&seen)));
    let img = lightcraft_parsing::RgbImage::new(2, 2, vec![5; 12]).unwrap();
    // (between a caller's cache lookup and its taking the runs' lock, the run of the same picture ends)
    let key = cache::key(&img, &[], &p.ahead_version());
    p.test_finished(&key, fake_parse(p.model_set()));
    assert!(matches!(p.parse(img, Vec::new(), true), crate::people_parsing::Outcome::Ready { cached: true, .. }));
    assert_eq!(p.runs(), 0);
}

#[test]
fn a_hold_before_the_model_run_has_started_still_stops_it() {
    let (mut s, _dir, ids) = library("early-stop", 1);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    open(&mut s, ids[0]);
    // the photo is handed over and the user drags at once: the worker is still rendering, the run not started
    let mut going = None;
    for _ in 0..POLLS {
        pump(&mut s, json!({"pace": "light"}));
        going = s.parse_ahead_test_run();
        if going.is_some() {
            break;
        }
        // (another test may hold every background build for a moment)
        std::thread::sleep(POLL);
    }
    let run = going.expect("a photo going");
    pump(&mut s, json!({"pace": "pause"}));
    assert!(run.stop_pending() || gate.stopped.load(Ordering::SeqCst) == 1, "the stop was lost");
    // a stop meant for a hold that ended does not stop the run after it
    gate.open(1);
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!((st["parsed"].as_u64(), gate.stopped.load(Ordering::SeqCst)), (Some(1), 0), "{st}");
}

/// A YuNet folder whose model file is damaged (it does not load).
fn damaged_yunet(s: &mut Session, dir: &Path) {
    let home = dir.join("faces").join(lightcraft_faces::known::YUNET_ID);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("model.onnx"), b"not a model").unwrap();
    std::fs::write(home.join("face-model.json"), b"{}").unwrap();
    s.face_models_dir = Some(dir.join("faces"));
    s.face_shared_dir = None;
}

#[test]
fn the_pump_never_reads_the_face_detector_and_a_parsed_photos_mask_needs_no_parse() {
    let (mut s, dir, ids) = library("yunet", 2);
    damaged_yunet(&mut s, &dir);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    edit_open(&mut s, ids[0]);
    let reads = || crate::cmd::face_detect::READS.with(std::cell::Cell::get);
    let before = reads();
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!(st["parsed"], 2, "{st}");
    for _ in 0..10 {
        pump(&mut s, json!({"pace": "light"}));
    }
    assert_eq!(reads(), before, "the pump read the detector on the UI thread");
    // a mask of a photo parsed ahead: read by the photo's identity, no detection, no model run
    let path = dir.join("mask.png");
    let r = s.execute("parsing.mask", &json!({"ids": [ids[1].0], "part": "hair", "path": path.to_string_lossy()})).unwrap();
    assert!(path.is_file(), "{r}");
    assert_eq!(reads(), before, "the mask rendered and looked for faces to find the parse");
    assert_eq!(s.parsing.runs(), 2);
}

/// Portrait retouching on (Skin Tone's Face Light), so a render of the photo finds its faces from the parse.
fn retouch_on(s: &mut Session, id: PhotoId) {
    s.selection = crate::Selection::single(id);
    s.execute("develop.set", &json!({"control": "retouch.skinTone.faceLight", "value": 100.0})).unwrap();
}

#[test]
fn portrait_retouching_reads_the_parse_the_index_knows_and_files_the_one_it_makes() {
    let (mut s, dir, ids) = library("retouch", 2);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    // photo 0 was parsed (ahead of time, or by `parsing.run`) under a key this search would not make from its own
    // picture and faces (a detector on another device finds them a hair apart): read through the index, no model run
    let version = s.parsing.version();
    let other = cache::key(&lightcraft_parsing::RgbImage::new(1, 1, vec![9; 3]).unwrap(), &[], &version);
    let mut two = fake_parse(s.parsing.model_set());
    // (filed as a parse is: its edges refined, #240)
    two.model_version.clone_from(&version);
    two.people.push(Person { face: Some(NormRect::new(0.05, 0.1, 0.25, 0.4)), face_score: 0.8, ..Person::default() });
    cache::store(&dir.join("cache"), &other, &two).unwrap();
    crate::parse_ahead::remember(&mut s, ids[0], crate::people_parsing::Filed { version: version.clone(), key: other });
    retouch_on(&mut s, ids[0]);
    s.render_now(ids[0], 48, 48).unwrap();
    assert_eq!(s.retouch_faces_of(ids[0]).found.map(|f| f.input.faces.len()), Some(2), "the faces of the filed parse");
    assert_eq!(s.parsing.runs(), 0, "a parsed photo opened for retouching ran the model");
    // photo 1, parsed by the retouch search, is filed: a request reads it without a render, the queue skips it
    retouch_on(&mut s, ids[1]);
    s.render_now(ids[1], 48, 48).unwrap();
    assert_eq!(s.parsing.runs(), 1);
    assert!(crate::parse_ahead::ready(&mut s, ids[1]).is_some(), "the retouch search's parse is not in the index");
    let st = settle(&mut s, json!({"pace": "light"}));
    assert_eq!(st["skipped"], 2, "{st}");
    assert_eq!(s.parsing.runs(), 1);
}

#[test]
fn the_retouch_face_search_never_loads_the_face_detector_on_the_thread_that_sets_it_up() {
    let (mut s, dir, ids) = library("retouch-yunet", 1);
    damaged_yunet(&mut s, &dir);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    retouch_on(&mut s, ids[0]);
    let reads = || crate::cmd::face_detect::READS.with(std::cell::Cell::get);
    let before = reads();
    // a preview without the faces starts the search on its worker
    let job = s.render_job(ids[0], 48, 48, false, true).unwrap();
    assert!(job.info.retouch.is_none());
    until("the faces arriving", || s.retouch_faces_of(ids[0]).found.is_some());
    assert_eq!(reads(), before, "the search's set-up read the detector here");
    assert_eq!(s.parsing.runs(), 1);
}
