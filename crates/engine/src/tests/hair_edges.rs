//! Fork feature (`fork/hair-edges.md`, issue #103): BiRefNet-matting edges for Entire Person / Background and Subject
//! masks. The default leaves every mask as it was; the matte decides only near a mask's edge; a missing or damaged
//! model is a message; the commands refuse what they should. A fake matte stands in for the model; the test that runs
//! the real one reads it from the shared models folder and skips without it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_parsing::{ModelSet, NormRect, Parsing, Part, Person, Plane, RgbImage};
use serde_json::json;

use super::find_removals::temp_dir;
use crate::Session;
use crate::activity::Activity;
use crate::hair_edges::{Edge, EdgeKind, HairEdges, Matte, Runner};
use crate::people_parsing::{Outcome, PeopleParsing};

const SIDE: usize = 64;

/// A disc of `radius` (fractions of the side) at `(cx, cy)` as ±`v` logits on a `side²` grid.
fn disc(side: usize, cx: f32, cy: f32, radius: f32, v: f32) -> Vec<f32> {
    (0..side * side)
        .map(|i| {
            let (x, y) = (((i % side) as f32 + 0.5) / side as f32, ((i / side) as f32 + 0.5) / side as f32);
            if ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < radius { v } else { -v }
        })
        .collect()
}

/// The matte a portrait gets: the person's disc (r 0.3) a little bigger than the parse's (r 0.27, the strands the parse
/// cut off), and a lamp far from the person.
fn portrait_matte() -> Matte {
    let mut logits = disc(SIDE, 0.5, 0.5, 0.3, 9.0);
    for (v, lamp) in logits.iter_mut().zip(disc(SIDE, 0.08, 0.08, 0.05, 9.0)) {
        if lamp > 0.0 {
            *v = 9.0;
        }
    }
    Matte { side: SIDE, logits }
}

fn fake_matte() -> Runner {
    Arc::new(|_, _, _, _| Ok(portrait_matte()))
}

fn edges(tag: &str) -> (HairEdges, PathBuf, Activity) {
    let dir = temp_dir(&format!("hair-{tag}"));
    let activity = Activity::default();
    let h = HairEdges::new(activity.clone());
    h.set_paths(Some(dir.join("models")), Some(dir.join("mask-edges.json")));
    (h, dir, activity)
}

fn picture() -> RgbImage {
    let (w, h) = (32, 32);
    RgbImage::new(w, h, (0..w * h * 3).map(|i| (i as u8).wrapping_mul(13)).collect()).unwrap()
}

/// One person: a disc of radius 0.27 at the centre (Entire Person, one 32² plane over the photo) with Hair on its top.
fn person_parse(set: ModelSet) -> Parsing {
    let n = 32;
    let entire = disc(n, 0.5, 0.5, 0.27, 1.0).iter().map(|v| v.max(0.0)).collect();
    let mut p = Person { face: Some(NormRect::new(0.4, 0.35, 0.6, 0.55)), face_score: 0.9, ..Person::default() };
    p.bounds = Some(NormRect::new(0.23, 0.23, 0.77, 0.77));
    p.parts.insert(Part::EntirePerson, vec![Plane { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), width: n, height: n, data: entire }]);
    p.parts.insert(Part::Hair, vec![Plane { rect: NormRect::new(0.3, 0.23, 0.7, 0.36), width: 8, height: 4, data: vec![1.0; 32] }]);
    Parsing { model_set: set.id().into(), model_version: set.version(), width: 32, height: 32, people: vec![p] }
}

fn parsing_with(h: &HairEdges, dir: &Path) -> PeopleParsing {
    let p = PeopleParsing::new(Activity::default()).with_hair_edges(h.clone());
    p.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    p.set_runner(Some(Arc::new(|set, _, _, _, _| Ok(person_parse(set)))));
    p
}

fn ready(o: Outcome) -> Arc<Parsing> {
    match o {
        Outcome::Ready { parsing, .. } => parsing,
        other => panic!("no parse: {other:?}"),
    }
}

#[test]
fn the_default_edge_is_current_and_leaves_masks_as_they_were() {
    let (h, dir, _) = edges("default");
    h.set_runner(Some(fake_matte()));
    for k in EdgeKind::ALL {
        assert_eq!(h.chosen(k), Edge::Current);
        assert_eq!(h.edge(k), Edge::Current);
    }
    // people parsing: the parse exactly as the parser made it, and no matte made
    let p = parsing_with(&h, &dir);
    let parsed = ready(p.parse(picture(), Vec::new(), true));
    assert_eq!(*parsed, person_parse(ModelSet::Segformer));
    assert_eq!(h.runs(), 0);
    // Subject and Background: no matting step
    #[cfg(feature = "sam")]
    {
        let mut s = Session::with_demo();
        s.hair_edges = h.clone();
        for kind in ["subject", "background", "sky"] {
            assert!(s.subject_matting(kind, crate::segment::birefnet::SelectionModel::BiRefNet).is_none(), "{kind}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn matting_draws_the_entire_person_edge_and_keeps_far_things_out() {
    let (h, dir, activity) = edges("people");
    h.set_runner(Some(fake_matte()));
    h.set_edge(EdgeKind::People, Edge::Matting).unwrap();
    let p = parsing_with(&h, &dir);
    let parsed = ready(p.parse(picture(), Vec::new(), true));
    let person = &parsed.people[0];
    assert!(parsed.model_version.ends_with("+matting1"), "{}", parsed.model_version);
    // a strand 0.285 from the centre: out in the parse, in the matte, inside the band → in
    assert!(person.sample(Part::EntirePerson, 0.5, 0.5 - 0.285) > 0.5, "the strand comes in");
    assert!(person.sample(Part::EntirePerson, 0.5, 0.5) > 0.9, "the inside stays in");
    // the lamp the matte also cut out, far from the person, never comes in
    assert!(parsed.background(0.08, 0.08) > 0.99, "the lamp stays background");
    // the strand above the hair joins the Hair part
    assert!(person.sample(Part::Hair, 0.5, 0.5 - 0.285) > 0.5, "the strand is hair");
    assert_eq!(h.runs(), 1);
    // the activity row came and went
    assert!(activity.list().iter().all(|t| t.kind != "matting"));
    // the same photo again: cached, no new matte; back to Current: the parse as it was (its own cache entry)
    assert!(matches!(p.parse(picture(), Vec::new(), true), Outcome::Ready { cached: true, .. }));
    h.set_edge(EdgeKind::People, Edge::Current).unwrap();
    assert_eq!(*ready(p.parse(picture(), Vec::new(), true)), person_parse(ModelSet::Segformer));
    assert_eq!(h.runs(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_person_never_grows_into_the_next_person() {
    let matte = Matte { side: SIDE, logits: vec![9.0; SIDE * SIDE] };
    let mut parse = person_parse(ModelSet::Segformer);
    // a second person right next to the first, overlapping the band
    let mut other = parse.people[0].clone();
    let n = 32;
    let data = disc(n, 0.5 + 0.56, 0.5, 0.27, 1.0).iter().map(|v| v.max(0.0)).collect();
    other.parts.insert(Part::EntirePerson, vec![Plane { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), width: n, height: n, data }]);
    other.parts.remove(&Part::Hair);
    parse.people.push(other);
    crate::hair_edges::people::refine(&mut parse, &matte);
    // 0.78 is inside the second person and within the first's band: it stays the second's only
    assert!(parse.people[0].sample(Part::EntirePerson, 0.785, 0.5) < 0.5);
    assert!(parse.people[0].sample(Part::EntirePerson, 0.5, 0.5 + 0.285) > 0.5, "free strands still come in");
}

#[cfg(feature = "sam")]
#[test]
fn a_subject_selection_gets_the_matte_at_its_edge_only() {
    let base = lightcraft_develop::SegMask::from_logits(32, &disc(32, 0.5, 0.5, 0.27, 6.0)).made_by("birefnet");
    let out = crate::hair_edges::subject_edge(&base, &portrait_matte(), Edge::Matting).expect("a selection");
    assert_eq!(out.model.as_deref(), Some("birefnet+matting"));
    let l = out.logits().unwrap();
    let at = |x: f32, y: f32| l[(y * SIDE as f32) as usize * SIDE + (x * SIDE as f32) as usize];
    assert!(at(0.5, 0.5 - 0.29) > 0.0, "the strand comes in");
    assert!(at(0.08, 0.08) < 0.0, "the lamp stays out");
    let sam = lightcraft_develop::SegMask::from_logits(32, &disc(32, 0.5, 0.5, 0.27, 6.0));
    assert_eq!(crate::hair_edges::subject_edge(&sam, &portrait_matte(), Edge::Matting).unwrap().model.as_deref(), Some("sam3+matting"));
    assert_eq!(crate::segment::birefnet::label_of(Some("sam3+matting")), "SAM 3, BiRefNet Matting Edges");
    assert_eq!(crate::segment::birefnet::label_of(Some("sam3+birefnet+matting")), "SAM 3 + BiRefNet Edges, BiRefNet Matting Edges");
    // a damaged base is no selection, not a panic
    let broken = lightcraft_develop::SegMask { data: "!!".into(), ..sam };
    assert!(crate::hair_edges::subject_edge(&broken, &portrait_matte(), Edge::Matting).is_none());
}

#[cfg(feature = "sam")]
#[test]
fn the_subject_edge_applies_to_subject_and_background_not_sky() {
    let (h, dir, _) = edges("subject");
    h.set_runner(Some(fake_matte()));
    h.set_edge(EdgeKind::Subject, Edge::Matting).unwrap();
    let mut s = Session::with_demo();
    s.hair_edges = h;
    use crate::segment::birefnet::SelectionModel;
    assert!(s.subject_matting("subject", SelectionModel::BiRefNet).is_some());
    assert!(s.subject_matting("background", SelectionModel::BiRefNet).is_some());
    assert!(s.subject_matting("sky", SelectionModel::Sam3).is_none());
    // SAM 3 without the local model (none here): no matting step it could not run
    assert!(s.subject_matting("subject", SelectionModel::Sam3).is_none());
    let _ = std::fs::remove_dir_all(dir);
}

// Without `sam` the build has no BiRefNet Matting at all, so the choice is refused for that reason instead.
#[cfg(feature = "sam")]
#[test]
fn a_missing_or_damaged_model_is_a_message_not_a_panic() {
    let (h, dir, _) = edges("damaged");
    // missing: the choice is refused, and a matte asked for anyway says how to get the model
    let e = h.set_edge(EdgeKind::People, Edge::Matting).unwrap_err();
    assert!(e.contains("not downloaded") && e.contains("MB"), "{e}");
    assert_eq!(h.chosen(EdgeKind::People), Edge::Current);
    let flag = Arc::new(AtomicBool::new(false));
    let img = picture();
    assert!(h.matte(&img.data, img.width, img.height, &flag).unwrap_err().contains("not downloaded"));
    // a damaged picture is an error too
    assert!(h.matte(&[1, 2, 3], 5, 5, &flag).is_err());
    if !HairEdges::AVAILABLE {
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    // damaged: short garbage, a file of the right size holding zeros, and a header that claims too much
    let model = dir.join("models").join("birefnet-matting");
    std::fs::create_dir_all(&model).unwrap();
    let file = model.join("model.safetensors");
    let right_size_zeros = |f: &Path| std::fs::File::create(f).and_then(|f| f.set_len(crate::hair_edges::MODEL_BYTES)).unwrap();
    // a short file (a download cut off) is not installed: Settings offers the download again
    for len in [0, 22, crate::hair_edges::MODEL_BYTES - 1, crate::hair_edges::MODEL_BYTES + 1] {
        std::fs::File::create(&file).and_then(|f| f.set_len(len)).unwrap();
        assert!(!h.installed(), "{len} bytes");
        assert!(h.set_edge(EdgeKind::People, Edge::Matting).is_err(), "{len} bytes");
    }
    for (name, write) in [
        ("zeros", Box::new(right_size_zeros) as Box<dyn Fn(&Path)>),
        (
            "huge-header",
            Box::new(|f: &Path| {
                std::fs::write(f, [0xffu8; 16]).unwrap();
                std::fs::OpenOptions::new().write(true).open(f).and_then(|h| h.set_len(crate::hair_edges::MODEL_BYTES)).unwrap();
            }),
        ),
    ] {
        write(&file);
        assert!(h.installed(), "{name}");
        h.set_edge(EdgeKind::People, Edge::Matting).unwrap();
        let e = h.matte(&img.data, img.width, img.height, &flag).unwrap_err();
        assert!(e.contains("download it again"), "{name}: {e}");
        // people parsing with the damaged model: a failed parse naming the hair edges, never a panic
        let p = parsing_with(&h, &dir);
        match p.parse(picture(), Vec::new(), true) {
            Outcome::Failed(e) => assert!(e.contains("hair edges"), "{name}: {e}"),
            other => panic!("{name}: {other:?}"),
        }
        assert!(p.message().is_some_and(|m| m.contains("hair edges")), "{name}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_panicking_or_invalid_matte_is_a_message() {
    let (h, dir, _) = edges("panic");
    let flag = Arc::new(AtomicBool::new(false));
    let img = picture();
    #[allow(clippy::panic)]
    h.set_runner(Some(Arc::new(|_, _, _, _| panic!("a test panic"))));
    assert!(h.matte(&img.data, img.width, img.height, &flag).unwrap_err().contains("unexpectedly"));
    h.set_runner(Some(Arc::new(|_, _, _, _| Ok(Matte { side: 4, logits: vec![f32::NAN; 16] }))));
    assert!(h.matte(&img.data, img.width, img.height, &flag).unwrap_err().contains("invalid"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_commands_read_set_and_refuse() {
    let (h, dir, _) = edges("cmd");
    let mut s = Session::with_demo();
    s.hair_edges = h.clone();
    let st = s.execute("maskEdges.status", &json!({})).unwrap();
    assert_eq!(st["installed"], false);
    assert_eq!(st["sizeBytes"], 884_878_856u64);
    assert_eq!(st["kinds"][0]["edge"], "current");
    assert_eq!(st["kinds"][1]["kind"], "subject");
    // not installed: refused
    assert!(s.execute("maskEdges.edge", &json!({"kind": "people", "edge": "birefnet-matting"})).is_err());
    for bad in
        [json!({}), json!({"kind": 3}), json!({"kind": "sky"}), json!({"kind": "people", "edge": "lasso"}), json!({"kind": "x".repeat(10_000)})]
    {
        let e = s.execute("maskEdges.edge", &bad).unwrap_err().to_string();
        assert!(e.len() < 400, "{e}");
    }
    h.set_runner(Some(fake_matte()));
    let r = s.execute("maskEdges.edge", &json!({"kind": "subject", "edge": "birefnet-matting"})).unwrap();
    assert_eq!((r["chosen"].as_str(), r["edge"].as_str()), (Some("birefnet-matting"), Some("birefnet-matting")));
    // kept across sessions
    let again = HairEdges::new(Activity::default());
    again.set_paths(Some(dir.join("models")), Some(dir.join("mask-edges.json")));
    assert_eq!(again.chosen(EdgeKind::Subject), Edge::Matting);
    assert_eq!(again.edge(EdgeKind::Subject), Edge::Current, "chosen, but the model is not here: Current");
    // the download needs `confirm: true`
    for bad in [json!({}), json!({"confirm": "yes"}), json!({"confirm": false})] {
        let e = s.execute("maskEdges.download", &bad).unwrap_err().to_string();
        assert!(e.contains("confirm") && e.contains("885 MB"), "{e}");
    }
    assert_eq!(s.execute("maskEdges.cancel", &json!({})).unwrap()["cancelled"], false);
    let _ = std::fs::remove_dir_all(dir);
}

/// The download goes to `<models>/birefnet-matting`, with its row; a server that sends the wrong file is an error
/// (the size and SHA-256 are checked), never an installed model.
#[cfg(feature = "sam")]
#[test]
fn a_download_of_the_wrong_file_is_refused() {
    use std::io::{Read, Write};
    let (h, dir, activity) = edges("download");
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = server.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in server.incoming().take(8) {
            let Ok(mut c) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf);
            let body = b"not the model";
            let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = c.write_all(body);
        }
    });
    let target = h.model_dir().unwrap();
    assert!(h.start_download_from(&activity, target, vec![format!("http://{addr}")]).unwrap());
    let t = std::time::Instant::now();
    while h.download_status().running {
        assert!(t.elapsed() < std::time::Duration::from_secs(60), "the download never ended");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let st = h.download_status();
    assert!(st.error.is_some() && !st.finished, "{st:?}");
    assert!(!h.installed());
    let _ = std::fs::remove_dir_all(dir);
}

/// The real model, from the shared models folder: a portrait's matte, its time, and a parse refined by it.
#[cfg(feature = "sam")]
#[test]
fn the_real_model_mattes_a_portrait() {
    let Some(models) = crate::shared_models::default_shared_models_dir().filter(|d| d.join("birefnet-matting/model.safetensors").is_file()) else {
        lightcraft_testkit::skip!(Model, "BiRefNet Matting is not in the shared models folder");
        return;
    };
    let h = HairEdges::new(Activity::default());
    h.set_paths(Some(models), None);
    // a bright disc (a head) on grey, with thin dark lines (strands) leaving it
    let (w, hh) = (256, 256);
    let mut rgb = vec![128u8; w * hh * 3];
    for y in 0..hh {
        for x in 0..w {
            let (dx, dy) = (x as f32 - 128.0, y as f32 - 140.0);
            let r = (dx * dx + dy * dy).sqrt();
            let strand = r < 100.0 && (dx.atan2(dy) * 12.0).sin().abs() < 0.05;
            if r < 70.0 || strand {
                let v = if r < 70.0 { 220 } else { 40 };
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[v, v.saturating_sub(30), v.saturating_sub(60)]);
            }
        }
    }
    let flag = Arc::new(AtomicBool::new(false));
    let m = h.matte(&rgb, w, hh, &flag).unwrap();
    assert_eq!(m.side, 1024);
    assert!(m.is_valid());
    assert!(m.coverage(0.5, 140.0 / 256.0) > 0.5, "the head is in the matte");
    // a second call is the cached matte
    let again = h.matte(&rgb, w, hh, &flag).unwrap();
    assert!(Arc::ptr_eq(&m, &again));
    assert_eq!(h.runs(), 1);
}

/// One model on the GPU at a time: a matte asked for while another photo's parse runs waits for it (it overlapped the
/// parse before, and a Subject matte overlapped parses and LaMa fills).
#[test]
fn a_matte_never_runs_beside_a_parse() {
    use std::sync::atomic::Ordering;
    let (h, dir, _) = edges("gpu-turn");
    let busy = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(AtomicBool::new(false));
    let overlap = Arc::new(AtomicBool::new(false));
    let p = PeopleParsing::new(Activity::default());
    p.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    let (b, g) = (busy.clone(), gate.clone());
    p.set_runner(Some(Arc::new(move |set, _, _, _, _| {
        b.store(true, Ordering::SeqCst);
        while !g.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        b.store(false, Ordering::SeqCst);
        Ok(person_parse(set))
    })));
    let (b, o) = (busy.clone(), overlap.clone());
    h.set_runner(Some(Arc::new(move |_, _, _, _| {
        if b.load(Ordering::SeqCst) {
            o.store(true, Ordering::SeqCst);
        }
        Ok(portrait_matte())
    })));
    assert!(matches!(p.parse(picture(), Vec::new(), false), Outcome::Running));
    let t = std::time::Instant::now();
    while !busy.load(Ordering::SeqCst) {
        assert!(t.elapsed() < std::time::Duration::from_secs(30), "the parse never started");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let matting = {
        let h = h.clone();
        std::thread::spawn(move || {
            let img = RgbImage::new(8, 8, vec![7; 192]).unwrap();
            h.matte(&img.data, 8, 8, &Arc::new(AtomicBool::new(false))).map(|_| ())
        })
    };
    // (the order only: give the matte time to start beside the parse, as it did before)
    std::thread::sleep(std::time::Duration::from_millis(300));
    gate.store(true, Ordering::SeqCst);
    matting.join().unwrap().unwrap();
    assert!(!overlap.load(Ordering::SeqCst), "the matte ran while the parse held the GPU");
    let _ = std::fs::remove_dir_all(dir);
}

/// A panic on the GPU still reaches the CPU attempt; a panic on the CPU is the final error; after the GPU failed once,
/// the next photo goes straight to the CPU.
#[test]
#[allow(clippy::panic)]
fn a_gpu_panic_falls_back_to_the_cpu() {
    use crate::hair_edges::{Attempt, on_devices};
    let refused = AtomicBool::new(false);
    let mut tried = Vec::new();
    let m = on_devices(&refused, |cpu| {
        tried.push(cpu);
        if !cpu {
            panic!("a Metal kernel panic");
        }
        Attempt::Done(Matte { side: 1, logits: vec![1.0] })
    });
    assert_eq!(m.unwrap().side, 1);
    assert_eq!(tried, [false, true]);
    tried.clear();
    let e = on_devices(&refused, |cpu| {
        tried.push(cpu);
        panic!("a CPU panic")
    });
    assert!(e.unwrap_err().contains("unexpectedly"));
    assert_eq!(tried, [true], "the GPU that failed is not tried again");
    // a GPU error (not a panic) also falls back, and a damaged file is not retried on the CPU
    let fresh = AtomicBool::new(false);
    assert!(
        on_devices(&fresh, |cpu| if cpu { Attempt::Done(Matte { side: 1, logits: vec![0.0] }) } else { Attempt::Gpu("out of memory".into()) })
            .is_ok()
    );
    let fresh = AtomicBool::new(false);
    let mut n = 0;
    assert!(
        on_devices(&fresh, |_| {
            n += 1;
            Attempt::Fail("damaged".into())
        })
        .is_err()
    );
    assert_eq!(n, 1);
}

/// The choice is kept only once it is written; two callers at once never lose one another's kind.
#[test]
fn the_choice_is_written_before_it_counts() {
    let (h, dir, _) = edges("choice");
    h.set_runner(Some(fake_matte()));
    // a choice file that cannot be written (a folder in its place): an error, and the choice as it was
    let blocked = dir.join("blocked.json");
    std::fs::create_dir_all(blocked.with_extension("json.part")).unwrap();
    h.set_paths(Some(dir.join("models")), Some(blocked));
    assert!(h.set_edge(EdgeKind::People, Edge::Matting).is_err());
    assert_eq!(h.chosen(EdgeKind::People), Edge::Current, "not chosen when it could not be kept");
    // two threads, each flipping its own kind
    h.set_paths(Some(dir.join("models")), Some(dir.join("mask-edges.json")));
    let threads: Vec<_> = EdgeKind::ALL
        .into_iter()
        .map(|kind| {
            let h = h.clone();
            std::thread::spawn(move || {
                for i in 0..50 {
                    h.set_edge(kind, if i % 2 == 0 { Edge::Matting } else { Edge::Current }).unwrap();
                }
                h.set_edge(kind, Edge::Matting).unwrap();
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let again = HairEdges::new(Activity::default());
    again.set_paths(None, Some(dir.join("mask-edges.json")));
    for k in EdgeKind::ALL {
        assert_eq!(h.chosen(k), Edge::Matting, "{k:?}");
        assert_eq!(again.chosen(k), Edge::Matting, "{k:?} in the file");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A download asked for with a kind chooses BiRefNet Matting for it when it ends, with no Settings window drawn.
#[test]
fn the_kind_asked_for_is_chosen_when_the_download_ends() {
    let (h, dir, _) = edges("pending");
    h.set_runner(Some(fake_matte()));
    // (no download runs: it has "finished")
    h.choose_when_downloaded(EdgeKind::Subject);
    let t = std::time::Instant::now();
    while h.chosen(EdgeKind::Subject) != Edge::Matting {
        assert!(t.elapsed() < std::time::Duration::from_secs(30), "never chosen");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(h.chosen(EdgeKind::People), Edge::Current);
    // not installed when it ends (a failed download): nothing is chosen
    let (h, dir2, _) = edges("pending-failed");
    h.choose_when_downloaded(EdgeKind::People);
    assert_eq!(h.settle_download(), None);
    assert_eq!(h.chosen(EdgeKind::People), Edge::Current);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

/// A part's model id comes from the document: a hostile one with many suffixes is read once, never recursively.
#[cfg(feature = "sam")]
#[test]
fn a_hostile_model_id_is_read_once() {
    let id = format!("birefnet{}", "+matting".repeat(100_000));
    let label = crate::segment::birefnet::label_of(Some(&id));
    assert!(label.len() < 120, "{}", label.len());
    assert!(label.ends_with("BiRefNet Matting Edges"));
}
