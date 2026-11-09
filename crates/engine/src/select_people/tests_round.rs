//! Select People, second round (PR #123's reviews): a mask carried to another photo (Paste, Sync, Auto Sync) is drawn
//! from that photo's own people, and selects nothing when its face is not there; edge-model marks; Entire Person with
//! Background fills the photo; a command waits for the search already running; a renamed face is found again; a click
//! that changes nothing is no step. A fake runner stands in for the parser.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::MaskShape;
use lightcraft_geom::Orientation;
use lightcraft_parsing::{Parsing, Part};
use serde_json::json;

use super::grid::{Selection, build, model_id};
use super::tests::{at, centre, parse, person, session, temp_dir};
use super::{Pick, current};
use crate::Session;

/// Photo A (landscape): Ann at the left, someone at the right. Photo B (portrait): Ann lower right, someone at the top
/// left, nobody where A's second person is.
fn people_of_a() -> Parsing {
    parse(vec![person(0.08, 0.2, 0.3, Some("Ann")), person(0.6, 0.2, 0.3, None)])
}

fn people_of_b() -> Parsing {
    parse(vec![person(0.05, 0.05, 0.3, None), person(0.55, 0.55, 0.3, Some("Ann"))])
}

fn png(dir: &std::path::Path, name: &str, w: usize, h: usize) -> String {
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| if (i % w) * 2 < w { [60, 50, 40, 255] } else { [200, 190, 180, 255] }).collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
        &Default::default(),
    )
    .unwrap();
    let path = dir.join(name);
    std::fs::write(&path, png).unwrap();
    path.to_string_lossy().into_owned()
}

/// A session with photos A and B (A active, both selected) and a runner that parses each as its own.
fn two(tag: &str) -> (Session, PhotoId, PhotoId, std::path::PathBuf) {
    let dir = temp_dir(&format!("select-people-two-{tag}"));
    let (a, b) = (png(&dir, "a.png", 1200, 800), png(&dir, "b.png", 800, 1200));
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("library"), false).unwrap();
    s.execute("library.import", &json!({"paths": [a, b]})).unwrap();
    let mut ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).collect();
    ids.sort();
    let (a, b) = (ids[0], ids[1]);
    assert!(s.catalog.photo(a).unwrap().width > s.catalog.photo(a).unwrap().height, "A is the landscape one");
    s.execute("library.select", &json!({"ids": [a.0, b.0], "active": a.0})).unwrap();
    s.parsing.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    s.parsing.set_runner(Some(Arc::new(|_, _, img, _, _| Ok(if img.width > img.height { people_of_a() } else { people_of_b() }))));
    (s, a, b, dir)
}

/// The first part of photo `id`'s mask `mid`.
fn part_of(s: &Session, id: PhotoId, mid: u32) -> MaskShape {
    let d = s.develop_of(id).unwrap();
    d.masks.iter().find(|m| m.id == mid).unwrap().components[0].shape.clone()
}

/// How much of the part's place the grids of `shape` select.
fn covers(shape: &MaskShape, x: f32, y: f32, s: f32, part: Part) -> f32 {
    let MaskShape::People { seg: Some(main), detail, .. } = shape else { panic!("{shape:?}") };
    let (cx, cy) = centre(x, y, s, part);
    at(main, detail, cx, cy)
}

#[test]
fn a_pasted_mask_is_drawn_from_the_other_photos_people() {
    let (mut s, a, b, dir) = two("paste");
    let mid = s.execute("selectPeople.add", &json!({"parts": ["hair"], "face": 0})).unwrap()["mask"].as_u64().unwrap() as u32;
    assert!(covers(&part_of(&s, a, mid), 0.08, 0.2, 0.3, Part::Hair) > 0.9, "Ann's hair on A");
    s.execute("develop.copy", &json!({"groups": ["masks"]})).unwrap();
    s.execute("develop.paste", &json!({"ids": [b.0]})).unwrap();
    let on_b = part_of(&s, b, mid);
    assert!(covers(&on_b, 0.55, 0.55, 0.3, Part::Hair) > 0.9, "Ann's hair where she is on B");
    assert!(covers(&on_b, 0.08, 0.2, 0.3, Part::Hair) < 0.1, "not where she was on A");
    // the picker finds her on B
    let parsed = s.people_now(b).unwrap();
    assert_eq!(current(&on_b, Some(&parsed)).map(|(_, person, everyone)| (person, everyone)), Some((Some(1), false)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_synced_face_not_in_the_other_photo_selects_nothing_there() {
    let (mut s, a, b, dir) = two("sync");
    // A's second person: nobody is at their place on B
    let mid = s.execute("selectPeople.add", &json!({"parts": ["entirePerson"], "face": 1})).unwrap()["mask"].as_u64().unwrap() as u32;
    s.execute("develop.sync", &json!({"groups": ["masks"]})).unwrap();
    let on_b = part_of(&s, b, mid);
    let MaskShape::People { seg: Some(main), detail, .. } = &on_b else { panic!("{on_b:?}") };
    for (x, y) in [(0.75, 0.35), (0.2, 0.2), (0.7, 0.7), (0.5, 0.5)] {
        assert!(at(main, detail, x, y) < 0.1, "({x}, {y}) selected on B");
    }
    // the picker shows the face as missing: a Select People part with no person and not everyone's
    let parsed = s.people_now(b).unwrap();
    assert_eq!(current(&on_b, Some(&parsed)).map(|(_, person, everyone)| (person, everyone)), Some((None, false)));
    // A keeps its own
    assert!(covers(&part_of(&s, a, mid), 0.6, 0.2, 0.3, Part::EntirePerson) > 0.9);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn auto_sync_draws_each_photos_own_parts() {
    let (mut s, _, b, dir) = two("auto");
    s.set_auto_sync(true);
    let mid = s.execute("selectPeople.add", &json!({"parts": ["hair"], "face": 0})).unwrap()["mask"].as_u64().unwrap() as u32;
    s.execute("selectPeople.set", &json!({"parts": ["lips"]})).unwrap();
    let on_b = part_of(&s, b, mid);
    assert!(covers(&on_b, 0.55, 0.55, 0.3, Part::Lips) > 0.9, "Ann's lips on B");
    assert!(covers(&on_b, 0.08, 0.2, 0.3, Part::Lips) < 0.1, "not A's place");
    // a mask slider moved on A keeps B's own grids
    let grids = |shape: &MaskShape| match shape {
        MaskShape::People { seg, detail, .. } => (seg.clone(), detail.clone()),
        _ => panic!(),
    };
    let before = grids(&on_b);
    s.execute("mask.adjust", &json!({"values": {"exposure": 1.0}})).unwrap();
    assert_eq!(grids(&part_of(&s, b, mid)), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn edge_model_marks_follow_the_parts_it_drew() {
    let mut p = parse(vec![person(0.2, 0.2, 0.4, None)]);
    p.model_version = "1+matting1".into();
    let mark = |p: &Parsing, picks: &[Pick]| model_id(p, &Selection::of(None, picks));
    assert_eq!(mark(&p, &[Pick::EntirePerson]), "people:segformer+matting");
    assert_eq!(mark(&p, &[Pick::Hair]), "people:segformer+matting");
    assert_eq!(mark(&p, &[Pick::Background]), "people:segformer+matting");
    assert_eq!(mark(&p, &[Pick::Hair, Pick::Lips]), "people:segformer+matting");
    assert_eq!(mark(&p, &[Pick::Lips]), "people:segformer");
    p.model_version = "1+vision1".into();
    assert_eq!(mark(&p, &[Pick::Background, Pick::Hat]), "people:segformer+vision");
    p.model_version = "1".into();
    assert_eq!(mark(&p, &[Pick::EntirePerson]), "people:segformer");
    // the grids carry it
    p.model_version = "1+matting1".into();
    let (main, detail) = build(&p, &Selection::of(None, &[Pick::Hair, Pick::Background]), Orientation::Normal).unwrap();
    assert!(std::iter::once(&main).chain(&detail).all(|g| g.model.as_deref() == Some("people:segformer+matting")));
}

#[test]
fn entire_person_and_background_select_the_whole_photo() {
    // a person with a soft outline, as a parse draws one: 1 inside, 0.5 on the outline, 0 outside
    let mut soft = person(0.2, 0.2, 0.4, None);
    let n = 32;
    let data = (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f32 / (n - 1) as f32 - 0.5, (i / n) as f32 / (n - 1) as f32 - 0.5);
            (1.0 - ((x * x + y * y).sqrt() - 0.3) / 0.15).clamp(0.0, 1.0)
        })
        .collect();
    soft.parts.insert(
        Part::EntirePerson,
        vec![lightcraft_parsing::Plane { rect: lightcraft_parsing::NormRect::new(0.2, 0.2, 0.6, 0.6), width: n, height: n, data, split: false }],
    );
    let p = parse(vec![soft, person(0.65, 0.3, 0.2, None)]);
    let (main, detail) = build(&p, &Selection::of(None, &[Pick::EntirePerson, Pick::Background]), Orientation::Normal).unwrap();
    // across the outline of the first person, and everywhere else
    let low = (0..=300).map(|i| at(&main, &detail, 0.1 + f64::from(i) * 0.001, 0.4)).fold(1.0, f32::min);
    assert!(low > 0.99, "{low}");
    assert!(at(&main, &detail, 0.95, 0.95) > 0.99 && at(&main, &detail, 0.4, 0.4) > 0.99);
}

#[test]
fn a_command_waits_for_the_search_already_running() {
    let (mut s, dir) = session("wait");
    let id = s.active().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    {
        let (calls, gate) = (calls.clone(), gate.clone());
        s.parsing.set_runner(Some(Arc::new(move |_, _, _, _, _| {
            // the first parse (the picker's search) waits at the gate; another one is a second parse
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let (open, cv) = &*gate;
                let mut o = open.lock().unwrap();
                while !*o {
                    o = cv.wait(o).unwrap();
                }
            }
            Ok(super::tests::fake_people())
        })));
    }
    s.people_find(id, false);
    // open the gate once a second parse runs, or after a moment (then the command is already waiting)
    let opener = {
        let (calls, gate) = (calls.clone(), gate.clone());
        std::thread::spawn(move || {
            for _ in 0..50 {
                if calls.load(Ordering::SeqCst) > 1 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
        })
    };
    let got = s.people_now(id).unwrap();
    opener.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the command used the running search, not a parse of its own");
    let kept = s.people_of(id).ready.unwrap();
    assert!(Arc::ptr_eq(&got, &kept), "the picker and the command share one parse");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_face_named_since_is_found_again() {
    let (mut s, dir) = session("rename");
    let id = s.active().unwrap();
    s.people_now(id).unwrap();
    assert!(s.people_of(id).ready.is_some());
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.regions = vec![lightcraft_meta::Region {
        rect: lightcraft_meta::Rect { x0: 0.6, y0: 0.2, x1: 0.9, y1: 0.5 },
        kind: lightcraft_meta::RegionKind::Face,
        name: Some("Bo".into()),
        description: None,
    }];
    s.commit("name", lightcraft_catalog::Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
    assert!(s.people_of(id).ready.is_none(), "the people found before the name are not reused");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_click_that_changes_nothing_is_no_step() {
    let (mut s, dir) = session("same");
    let id = s.active().unwrap();
    s.execute("selectPeople.add", &json!({"parts": ["hair", "lips"], "face": 0})).unwrap();
    let steps = s.catalog.photo(id).unwrap().history.len();
    let r = s.execute("selectPeople.set", &json!({"parts": ["lips", "hair"], "face": 0})).unwrap();
    assert_eq!(r["unchanged"], true, "{r}");
    assert_eq!(s.catalog.photo(id).unwrap().history.len(), steps);
    // a real change still is one
    s.execute("selectPeople.set", &json!({"parts": ["hair"]})).unwrap();
    assert_eq!(s.catalog.photo(id).unwrap().history.len(), steps + 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_the_search_started_last_files_its_result() {
    use super::{Entry, Parsed, SelectPeople};
    let store = SelectPeople::default();
    let parsed = |n: usize| {
        Arc::new(Parsed { parsing: Arc::new(parse((0..n).map(|i| person(0.1 * i as f32, 0.1, 0.1, None)).collect())), thumbs: Vec::new() })
    };
    let first = store.start("k");
    let second = store.start("k");
    store.finish("k", first, Entry::Ready(parsed(1)));
    assert!(matches!(store.get("k"), Some(Entry::Busy(t)) if t == second), "an older search does not overwrite the running one");
    // a command waiting for the running search gets its result
    let waiter = {
        let store = store.clone();
        std::thread::spawn(move || match store.settled("k") {
            Some(Entry::Ready(p)) => p.parsing.people.len(),
            _ => 0,
        })
    };
    store.finish("k", second, Entry::Ready(parsed(2)));
    assert_eq!(waiter.join().unwrap(), 2);
    // a finished search's late twin changes nothing
    store.finish("k", second, Entry::Failed("late".into()));
    assert!(matches!(store.get("k"), Some(Entry::Ready(_))));
}

#[test]
fn one_person_alone_gets_no_copy_of_the_main_grid() {
    // a person whose parts are finer than the largest grid can draw: the main grid is over them already, so a finer
    // grid over the same box would be the same grid again
    let mut q = person(0.1, 0.1, 0.8, None);
    for planes in q.parts.values_mut() {
        let r = planes[0].rect;
        planes[0] = lightcraft_parsing::Plane { rect: r, width: 1500, height: 1500, data: vec![1.0; 1500 * 1500], split: false };
    }
    let p = parse(vec![q]);
    let (main, detail) = build(&p, &Selection::of(Some(0), &[Pick::EntirePerson]), Orientation::Normal).unwrap();
    assert_eq!(main.side as usize, lightcraft_develop::segmask::MAX_SIDE);
    assert!(detail.is_empty(), "{} finer grids", detail.len());
}
