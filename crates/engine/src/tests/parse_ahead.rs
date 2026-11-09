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

fn fake_parse(set: ModelSet) -> Parsing {
    let mut person = Person { face: Some(NormRect::new(0.4, 0.2, 0.6, 0.5)), face_score: 0.9, ..Person::default() };
    person.parts.insert(Part::Hair, vec![Plane { rect: NormRect::new(0.3, 0.1, 0.7, 0.3), width: 4, height: 2, data: vec![1.0; 8] }]);
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
fn library(tag: &str, n: usize) -> (Session, PathBuf, Vec<PhotoId>) {
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

fn open(s: &mut Session, id: PhotoId) {
    s.selection = crate::Selection::single(id);
}

fn pump(s: &mut Session, p: Value) -> Value {
    s.execute("parsing.ahead.pump", &p).unwrap()
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

#[test]
fn the_open_photos_neighbours_go_first_then_new_imports_then_the_rest_while_idle() {
    let (mut s, dir, ids) = library("order", 10);
    for id in &ids {
        add_face(&mut s, *id, None);
    }
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    let mut pixels = first_pixels(&mut s, &ids);
    open(&mut s, ids[4]);
    // the user is around: only the photos near the open one, nearest first, ahead before behind
    let st = pump(&mut s, json!({"pace": "light", "wait": true}));
    assert_eq!(st["state"], "done", "{st}");
    let near: Vec<PhotoId> = [4, 5, 3, 6, 2, 7, 8].iter().map(|i| ids[*i]).collect();
    assert_eq!(order_seen(&seen, &pixels), near);
    assert_eq!(st["parsed"], 7);
    assert!(st["queued"].as_u64().unwrap() > 0, "the rest wait for the user to be idle: {st}");
    // a photo imported now, with a face, goes before the rest, also while the user is around
    let new = write_photo(&dir, 10);
    import(&mut s, &[new]);
    let new_id = *s.visible_cloned().iter().find(|id| !ids.contains(id)).unwrap();
    add_face(&mut s, new_id, None);
    pixels.extend(first_pixels(&mut s, &[new_id]));
    // (an import opens the new photo; the user goes back to the one they were on)
    open(&mut s, ids[4]);
    let st = pump(&mut s, json!({"pace": "light", "wait": true}));
    assert_eq!(order_seen(&seen, &pixels).last(), Some(&new_id), "{st} {:?}", s.active());
    // idle: every other photo with faces, newest first
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
    assert_eq!(st["state"], "done", "{st}");
    let mut rest: Vec<PhotoId> = [0, 1, 9].iter().map(|i| ids[*i]).collect();
    let date = |s: &Session, id: PhotoId| {
        let p = s.catalog.photo(id).unwrap();
        p.captured.clone().unwrap_or_else(|| p.imported.clone())
    };
    rest.sort_by(|a, b| date(&s, *b).cmp(&date(&s, *a)).then(b.0.cmp(&a.0)));
    assert_eq!(order_seen(&seen, &pixels)[8..], rest[..]);
    assert_eq!(st["queued"], 0);
    // all parsed once
    assert_eq!(s.parsing.runs(), 11);
    // stepping backward: the photo before the open one is "ahead"
    let fresh = crate::parse_ahead::queue::near(&ids, ids[4], true);
    assert_eq!(fresh[1], ids[3]);
}

#[test]
fn photos_without_faces_wait_for_the_face_scan_except_near_the_open_photo() {
    let (mut s, _dir, ids) = library("faces", 4);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    add_face(&mut s, ids[3], None);
    // the face scan looked at photo 1 and found no face: not parsed even beside the open photo
    s.faces.scanned.insert(ids[1].0);
    open(&mut s, ids[0]);
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
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
    open(&mut s, ids[0]);
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
    let st = pump(&mut s, json!({"pace": "light", "wait": true}));
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
    open(&mut s, ids[0]);
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
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
    assert_eq!(st["parsed"], 2, "{st}");
}

#[test]
fn the_rows_cross_stops_the_run_and_empties_the_queue_until_another_photo_is_opened() {
    let (mut s, _dir, ids) = library("cancel", 3);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    open(&mut s, ids[0]);
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
    // another photo opened: parsing ahead starts again
    open(&mut s, ids[2]);
    let st = pump(&mut s, json!({"pace": "light", "wait": true}));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["parsed"], 3);
}

#[test]
fn the_setting_off_and_low_power_stop_everything() {
    let (mut s, _dir, ids) = library("off", 2);
    let gate = Gate::default();
    s.parsing.set_runner(Some(gate.runner()));
    open(&mut s, ids[0]);
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
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
    assert_eq!(st["parsed"], 2, "{st}");
}

#[test]
fn a_parsed_photo_opens_without_a_model_run_and_cached_photos_are_skipped_after_a_restart() {
    let (mut s, dir, ids) = library("skip", 3);
    let seen: Seen = Arc::default();
    s.parsing.set_runner(Some(recording_runner(&seen)));
    open(&mut s, ids[1]);
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
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
    open(&mut s, ids[1]);
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
    assert_eq!((st["parsed"].as_u64(), st["skipped"].as_u64()), (Some(0), Some(3)), "{st}");
    assert_eq!(s.parsing.runs(), 0);
    // a photo whose faces change is looked at again: its parse (the same picture, no YuNet) is found in the cache
    add_face(&mut s, ids[0], Some("Ann"));
    s.parse_ahead_test_rebuild();
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
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
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
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
        let key = cache::key(img, &[], &s.parsing.model_set().version());
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
    open(&mut s, ids[0]);
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
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
    open(&mut s, ids[0]);
    let st = pump(&mut s, json!({"pace": "normal", "wait": true}));
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
