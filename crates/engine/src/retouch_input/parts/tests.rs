//! Faces with part masks from the people parse reach the retouch step: 1, 3 and 50 faces, none, a face cut by the
//! edge, a 1 × 1 and a flat grey photo, a rotated photo, a missing parse (original pixels, the parse asked for), an
//! export waiting for the parse, a draft and a bigger render agreeing, pixels outside the mask unchanged, and a
//! face's own settings staying with its person when the parse comes back in another order. A fake runner stands
//! in for the models.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::Part as RetouchPart;
use lightcraft_geom::Orientation;
use lightcraft_parsing::{Landmark, NormRect, Parsing, Part, Person, Plane};
use lightcraft_raster::Rgba8;
use serde_json::json;

use crate::Session;
use crate::people_parsing::Runner;

/// A face with a Facial Skin disc filling its box (32 × 32 samples) and a 478-point mesh inside it.
fn person(r: NormRect) -> Person {
    let disc = (0..32 * 32)
        .map(|i| {
            let (dx, dy) = ((i % 32) as f32 - 15.5, (i / 32) as f32 - 15.5);
            if dx * dx + dy * dy < 15.0 * 15.0 { 1.0 } else { 0.0 }
        })
        .collect();
    let mesh =
        (0..478).map(|i| Landmark { x: r.x0 + r.width() * (i % 22) as f32 / 22.0, y: r.y0 + r.height() * (i / 22) as f32 / 22.0, z: 0.0 }).collect();
    let mut p = Person { face: Some(r), face_score: 0.9, landmarks: mesh, landmark_score: 0.9, bounds: Some(r), ..Person::default() };
    p.parts.insert(Part::FacialSkin, vec![Plane { rect: r, width: 32, height: 32, data: disc, split: false }]);
    p
}

fn parse_of(rects: &[NormRect]) -> Parsing {
    Parsing { model_set: "fake".into(), model_version: "1".into(), width: 1, height: 1, people: rects.iter().map(|r| person(*r)).collect() }
}

/// The parse the runner gives, and a gate it waits on.
#[derive(Clone)]
struct Fake {
    rects: Arc<Mutex<Vec<NormRect>>>,
    open: Arc<AtomicBool>,
}

impl Fake {
    fn new(rects: Vec<NormRect>) -> Fake {
        Fake { rects: Arc::new(Mutex::new(rects)), open: Arc::new(AtomicBool::new(true)) }
    }

    fn runner(&self) -> Runner {
        let me = self.clone();
        Arc::new(move |_, _, _, _, _| {
            // (a hang guard, not a timing check)
            let until = Instant::now() + Duration::from_secs(30);
            while !me.open.load(Ordering::SeqCst) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(parse_of(&me.rects.lock().unwrap()))
        })
    }
}

fn install(s: &Session, fake: &Fake, dir: &std::path::Path) {
    s.parsing.set_paths(Some(dir.join("models")), Some(dir.join("cache")), None);
    s.parsing.set_runner(Some(fake.runner()));
}

/// A session with one flat grey photo of `w × h`, its faces parsed by `fake`.
fn grey(tag: &str, w: usize, h: usize, fake: &Fake) -> (Session, PhotoId, PathBuf) {
    let dir = crate::tests::xmp::temp_dir(&format!("retouch-parts-{tag}"));
    let png = dir.join("grey.png");
    let img = Rgba8::from_fn(w, h, |_, _| [110, 110, 110, 255]);
    std::fs::write(&png, lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap()).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [png.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    install(&s, fake, &dir);
    (s, id, dir)
}

fn light(s: &mut Session, amount: f64) {
    s.execute("develop.set", &json!({"control": "retouch.skinTone.faceLight", "value": amount})).unwrap();
}

fn render(s: &mut Session, id: PhotoId, edge: usize) -> Rgba8 {
    s.render_now(id, edge, edge).unwrap().image
}

fn at(img: &Rgba8, nx: f64, ny: f64) -> [u8; 4] {
    let (x, y) = (((nx * img.width as f64) as usize).min(img.width - 1), ((ny * img.height as f64) as usize).min(img.height - 1));
    img.data[y * img.width + x]
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(60);
    while !ok() {
        assert!(Instant::now() < until, "{what} never happened");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> NormRect {
    NormRect::new(x0, y0, x1, y1)
}

#[test]
fn masks_reach_the_step_for_one_and_three_faces() {
    let three = vec![r(0.05, 0.2, 0.3, 0.6), r(0.4, 0.1, 0.6, 0.45), r(0.7, 0.5, 0.95, 0.9)];
    for rects in [three[..1].to_vec(), three.clone()] {
        let fake = Fake::new(rects.clone());
        let (mut s, id, dir) = grey(&format!("n{}", rects.len()), 200, 150, &fake);
        light(&mut s, 0.0);
        let plain = render(&mut s, id, 200);
        light(&mut s, 100.0);
        let lit = render(&mut s, id, 200);
        let found = s.retouch_faces_of(id).found.expect("found by the synchronous render");
        assert!(found.input.on_photo);
        assert_eq!(found.input.faces.len(), rects.len());
        for f in &found.input.faces {
            assert!(f.mask(RetouchPart::FacialSkin).is_some(), "the mask arrived");
            assert_eq!(f.landmarks.len(), 5 + 478, "five, then the mesh");
            assert!(f.upright.is_some());
            let (cx, cy) = f.bounds.centre();
            assert!(at(&lit, cx, cy)[0] > at(&plain, cx, cy)[0] + 10, "face at {cx}, {cy} is lit");
        }
        assert_eq!(found.thumbs.len(), rects.len());
        assert_eq!(s.parsing.runs(), 1, "one parse for the photo");
        // the parse is the one `parsing.run` reads: from the cache, no second run
        let ran = s.execute("parsing.run", &json!({"wait": true})).unwrap();
        assert_eq!(ran["photos"][0]["cached"], true, "{ran}");
        assert_eq!(s.parsing.runs(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn no_faces_fifty_faces_and_a_face_cut_by_the_edge() {
    let fifty: Vec<NormRect> =
        (0..50).map(|i| r((i % 10) as f32 / 10.0, (i / 10) as f32 / 5.0, (i % 10) as f32 / 10.0 + 0.09, (i / 10) as f32 / 5.0 + 0.19)).collect();
    let cut = vec![r(-0.1, 0.3, 0.25, 0.8)];
    for (tag, rects) in [("none", Vec::new()), ("fifty", fifty), ("cut", cut)] {
        let fake = Fake::new(rects.clone());
        let (mut s, id, dir) = grey(tag, 160, 120, &fake);
        light(&mut s, 0.0);
        let plain = render(&mut s, id, 160);
        light(&mut s, 100.0);
        let lit = render(&mut s, id, 160);
        assert_eq!(s.retouch_faces_of(id).found.map(|f| f.input.faces.len()), Some(rects.len()), "{tag}");
        match tag {
            "none" => assert_eq!(lit, plain, "no faces: the original pixels"),
            "cut" => assert!(at(&lit, 0.02, 0.55)[0] > at(&plain, 0.02, 0.55)[0] + 10, "the part inside the photo is lit"),
            _ => assert!(at(&lit, 0.045, 0.095)[0] > at(&plain, 0.045, 0.095)[0] + 10),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_one_pixel_and_a_flat_grey_photo() {
    let fake = Fake::new(vec![r(0.0, 0.0, 1.0, 1.0)]);
    let (mut s, id, dir) = grey("dot", 1, 1, &fake);
    light(&mut s, 100.0);
    assert!(s.render_now(id, 16, 16).is_ok());
    assert_eq!(s.retouch_faces_of(id).found.map(|f| f.input.faces.len()), Some(1));
    let _ = std::fs::remove_dir_all(dir);
    let fake = Fake::new(vec![r(0.3, 0.3, 0.7, 0.7)]);
    let (mut s, id, dir) = grey("flat", 120, 120, &fake);
    light(&mut s, 100.0);
    let lit = render(&mut s, id, 120);
    assert!(at(&lit, 0.5, 0.5)[0] > 130 && at(&lit, 0.05, 0.05)[0] == 110, "{:?} {:?}", at(&lit, 0.5, 0.5), at(&lit, 0.05, 0.05));
    let _ = std::fs::remove_dir_all(dir);
}

/// Rotate Right turns the photo: the masks (made on the photo before the rotation) turn with it.
#[test]
fn the_masks_turn_with_the_photo() {
    let fake = Fake::new(vec![r(0.05, 0.05, 0.35, 0.45)]);
    let (mut s, id, dir) = grey("turn", 200, 100, &fake);
    s.execute("photo.rotateRight", &json!({})).unwrap();
    let o = s.develop_of(id).unwrap().orientation;
    assert_ne!(o, Orientation::Normal);
    light(&mut s, 100.0);
    let lit = render(&mut s, id, 200);
    assert!(lit.height > lit.width, "turned: {}×{}", lit.width, lit.height);
    let face = &s.retouch_faces_of(id).found.unwrap().input.faces[0];
    let (cx, cy) = face.bounds.centre();
    let (ux, uy) = o.map(0.2, 0.25, 1.0, 1.0);
    assert!((cx - ux).abs() < 1e-6 && (cy - uy).abs() < 1e-6, "the box turned: {:?}", face.bounds);
    assert!(at(&lit, cx, cy)[0] > 130, "the turned face is lit");
    assert_eq!(at(&lit, 1.0 - cx, 1.0 - cy)[0], 110, "the rest is not");
    let _ = std::fs::remove_dir_all(dir);
}

/// A preview finds no parse: it renders the original pixels, never runs a model itself, and asks the parse job to
/// run; once the parse is there the next preview has the masks. An export waits for the parse.
#[test]
fn a_missing_parse_renders_the_original_pixels_and_asks_for_the_parse() {
    let fake = Fake::new(vec![r(0.3, 0.2, 0.7, 0.8)]);
    fake.open.store(false, Ordering::SeqCst);
    let (mut s, id, dir) = grey("missing", 160, 120, &fake);
    let plain = render(&mut s, id, 160);
    light(&mut s, 100.0);
    let job = s.render_job(id, 160, 160, false, true).unwrap();
    assert!(job.info.retouch.is_none(), "no faces yet");
    assert_eq!(job.run().rendered.unwrap().image, plain, "the original pixels");
    wait_until("the parse job starting", || s.parsing.busy());
    assert!(s.activity.list().iter().any(|t| t.kind == "parsing"), "the parse runs under its own row");
    assert!(s.retouch_faces_of(id).busy);
    fake.open.store(true, Ordering::SeqCst);
    wait_until("the faces arriving", || s.retouch_faces_of(id).found.is_some());
    let job = s.render_job(id, 160, 160, false, true).unwrap();
    assert!(job.info.retouch.as_ref().is_some_and(|i| i.faces[0].mask(RetouchPart::FacialSkin).is_some()));
    assert!(at(&job.run().rendered.unwrap().image, 0.5, 0.5)[0] > 130);
    let _ = std::fs::remove_dir_all(dir);

    // an export (a synchronous render) waits for the parse going
    let fake = Fake::new(vec![r(0.3, 0.2, 0.7, 0.8)]);
    fake.open.store(false, Ordering::SeqCst);
    let (mut s, id, dir) = grey("export", 160, 120, &fake);
    light(&mut s, 100.0);
    let opener = {
        let (parsing, open) = (s.parsing.clone(), fake.open.clone());
        std::thread::spawn(move || {
            wait_until("the export's parse starting", || parsing.busy());
            open.store(true, Ordering::SeqCst);
        })
    };
    let lit = render(&mut s, id, 160);
    opener.join().unwrap();
    assert!(at(&lit, 0.5, 0.5)[0] > 130, "the export has the masks");
    let _ = std::fs::remove_dir_all(dir);
}

/// A draft and a render four times its size light the same share of the picture, by the same amount, and leave
/// every pixel away from the mask as it was.
#[test]
fn a_draft_and_a_bigger_render_agree_and_pixels_outside_the_mask_are_unchanged() {
    let area = r(0.2, 0.15, 0.55, 0.75);
    let fake = Fake::new(vec![area]);
    let (mut s, id, dir) = grey("scale", 400, 300, &fake);
    let mut shares = Vec::new();
    for edge in [100, 400] {
        light(&mut s, 0.0);
        let plain = render(&mut s, id, edge);
        light(&mut s, 70.0);
        let lit = render(&mut s, id, edge);
        let (w, h) = (lit.width, lit.height);
        let mut changed = 0;
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (plain.data[y * w + x], lit.data[y * w + x]);
                let (nx, ny) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let near =
                    nx > area.x0 - 2.0 / w as f32 && nx < area.x1 + 2.0 / w as f32 && ny > area.y0 - 2.0 / h as f32 && ny < area.y1 + 2.0 / h as f32;
                if !near {
                    assert_eq!(a, b, "({x}, {y}) of {w}×{h} is outside the mask");
                }
                changed += usize::from(a != b);
            }
        }
        shares.push((changed as f64 / (w * h) as f64, at(&lit, 0.375, 0.45)[0]));
    }
    let ((small, v_small), (big, v_big)) = (shares[0], shares[1]);
    assert!((small - big).abs() < 0.02, "the lit share: {small} at 100 px, {big} at 400 px");
    assert!(v_small.abs_diff(v_big) <= 1, "the same light: {v_small} and {v_big}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Per-face settings are found by person (a named face) or by place (an unnamed one), never by the parse's
/// order; a person's settings never go to someone else when the parse comes back without them.
#[test]
fn a_faces_own_settings_stay_with_their_person_when_the_parse_changes() {
    use crate::tests::face_recognize::set_regions;
    use lightcraft_geom::Rect;
    use lightcraft_meta::{Region, RegionKind};
    let (ann, bob, eve) = (r(0.05, 0.2, 0.3, 0.6), r(0.4, 0.2, 0.65, 0.6), r(0.72, 0.3, 0.9, 0.6));
    let fake = Fake::new(vec![ann, bob, eve]);
    let (mut s, id, dir) = grey("own", 200, 150, &fake);
    let region = |b: NormRect, name: &str| Region {
        rect: Rect { x0: f64::from(b.x0), y0: f64::from(b.y0), x1: f64::from(b.x1), y1: f64::from(b.y1) },
        kind: RegionKind::Face,
        name: Some(name.into()),
        description: None,
    };
    set_regions(&mut s, id, vec![region(ann, " Ann "), region(bob, "Bob")]);
    let listed = s.execute("retouch.detect", &json!({})).unwrap();
    let names: Vec<_> = listed["faces"].as_array().unwrap().iter().map(|f| f["person"].clone()).collect();
    assert_eq!(names, [json!("ann"), json!("bob"), json!(null)], "named from the faces catalog: {listed}");
    s.execute("retouch.set", &json!({"control": "retouch.skinTone.faceLight", "value": 30, "face": 1})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.skinTone.faceLight", "value": 90, "face": 2})).unwrap();
    let own_of =
        |listed: &serde_json::Value, who: &str| listed["faces"].as_array().unwrap().iter().find(|f| f["person"] == who).map(|f| f["own"].clone());
    let value_of = |s: &Session, own: &serde_json::Value| {
        s.develop_of(id).unwrap().retouch.faces[own.as_u64().unwrap() as usize].settings.get("retouch.skinTone.faceLight")
    };
    // the parse comes back largest first in another order, with boxes moved a little
    *fake.rects.lock().unwrap() = vec![r(0.73, 0.31, 0.91, 0.61), r(0.41, 0.21, 0.66, 0.61), r(0.06, 0.2, 0.31, 0.6)];
    let again = s.execute("retouch.detect", &json!({"again": true})).unwrap();
    assert_eq!(own_of(&again, "ann"), Some(json!(null)), "{again}");
    assert_eq!(value_of(&s, &own_of(&again, "bob").unwrap()), Some(30.0));
    let eve_own = again["faces"].as_array().unwrap().iter().find(|f| f["person"].is_null()).map(|f| f["own"].clone()).unwrap();
    assert_eq!(value_of(&s, &eve_own), Some(90.0), "the unnamed face keeps hers by place");
    // Bob is gone and Ann stands where he stood: his settings stay his
    *fake.rects.lock().unwrap() = vec![bob];
    set_regions(&mut s, id, vec![region(bob, "Ann")]);
    let gone = s.execute("retouch.detect", &json!({"again": true})).unwrap();
    assert_eq!(own_of(&gone, "ann"), Some(json!(null)), "{gone}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Another model set is another parse: the faces are found anew, not served from the other set's.
#[test]
fn another_model_set_finds_the_faces_anew() {
    let fake = Fake::new(vec![r(0.3, 0.2, 0.7, 0.8)]);
    let (mut s, id, dir) = grey("set", 120, 90, &fake);
    s.execute("retouch.detect", &json!({})).unwrap();
    assert!(s.retouch_faces_of(id).found.is_some());
    let other = if s.parsing.model_set() == lightcraft_parsing::ModelSet::BiSeNet { "segformer" } else { "bisenet" };
    s.execute("parsing.modelSet", &json!({"set": other})).unwrap();
    assert!(s.retouch_faces_of(id).found.is_none(), "the other set's faces are not these");
    s.execute("retouch.detect", &json!({})).unwrap();
    assert_eq!(s.parsing.runs(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn several_planes_of_a_part_merge_and_a_plane_turns_exactly() {
    let a = Plane { rect: r(0.0, 0.0, 0.5, 0.5), width: 2, height: 2, data: vec![1.0, 0.0, 0.0, 0.0], split: false };
    let b = Plane { rect: r(0.5, 0.5, 1.0, 1.0), width: 2, height: 2, data: vec![0.0, 0.0, 0.0, f32::NAN], split: false };
    let m = super::part_mask(&[a.clone(), b], Orientation::Normal).unwrap();
    assert_eq!(m.area, lightcraft_develop::retouch::NormRect::new(0.0, 0.0, 1.0, 1.0));
    assert_eq!((m.plane.width, m.plane.height), (4, 4));
    assert!(m.sample(0.1, 0.1) > 0.9 && m.sample(0.9, 0.9) == 0.0, "a value that is not a number is 0");
    // one plane turned a quarter: its samples move, none resampled
    let t = super::part_mask(std::slice::from_ref(&a), Orientation::Rotate90).unwrap();
    assert_eq!(t.plane.data, vec![0.0, 1.0, 0.0, 0.0], "the top-left sample goes top-right");
    assert_eq!(t.area, lightcraft_develop::retouch::NormRect::new(0.5, 0.0, 1.0, 0.5));
    assert!(super::part_mask(&[Plane { width: 3, ..a }], Orientation::Normal).is_none(), "a malformed plane is no mask");
    assert!(super::part_mask(&[], Orientation::Normal).is_none());
}

/// Faces from the parse are made on the photo with default settings and mapped through the lens corrections and
/// transform by the step: dragging Geometry, Lens Distortion or the crop angle keeps them, starts no search, and a
/// preset's thumbnail with another lens distortion is retouched too.
#[test]
fn geometry_lens_and_crop_angle_drags_keep_the_faces_and_start_no_search() {
    let fake = Fake::new(vec![r(0.3, 0.2, 0.7, 0.8)]);
    let (mut s, id, dir) = grey("drag", 160, 120, &fake);
    light(&mut s, 100.0);
    s.execute("retouch.detect", &json!({})).unwrap();
    for (control, value) in
        [("geometry.vertical", 20.0), ("geometry.vertical", 35.0), ("optics.distortion", 30.0), ("optics.distortion", -40.0), ("crop.angle", 3.0)]
    {
        s.execute("develop.set", &json!({"control": control, "value": value})).unwrap();
        let job = s.render_job(id, 160, 160, false, true).unwrap();
        assert!(job.info.retouch.as_ref().is_some_and(|i| i.faces.len() == 1), "{control} at {value}: the faces are kept");
    }
    let mut preset = (*s.develop_of(id).unwrap()).clone();
    preset.optics.distortion = 60.0;
    assert!(s.variant_job(id, &preset, 64).unwrap().info.retouch.is_some(), "a preset's thumbnail is retouched");
    assert_eq!(s.retouch_faces.started(), 0, "no search started");
    assert_eq!(s.parsing.runs(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// A search set up with one model set keeps that set's faces when the set is switched before it runs: the parse is
/// the one of the set its key names.
#[test]
fn a_model_set_switch_during_a_search_keeps_the_first_sets_faces() {
    let (one, other) = (r(0.1, 0.2, 0.4, 0.6), r(0.55, 0.3, 0.9, 0.7));
    let fake = Fake::new(vec![one]);
    let (mut s, id, dir) = grey("switch", 120, 90, &fake);
    let first = s.parsing.model_set();
    s.parsing.set_runner(Some(Arc::new(move |set, _, _, _, _| Ok(parse_of(&[if set == first { one } else { other }])))));
    let p = s.catalog.photo(id).unwrap().clone();
    let finder = s.retouch_finder(&p, crate::retouch_input::Source::Parts).unwrap();
    let next = if first == lightcraft_parsing::ModelSet::BiSeNet { "segformer" } else { "bisenet" };
    s.execute("parsing.modelSet", &json!({"set": next})).unwrap();
    assert_ne!(s.parsing.model_set(), first);
    let found = finder.run().unwrap();
    let (cx, _) = found.input.faces[0].bounds.centre();
    assert!((cx - 0.25).abs() < 1e-6, "the first set's face, not the other's: {:?}", found.input.faces[0].bounds);
    let _ = std::fs::remove_dir_all(dir);
}

/// A damaged parse with a sliver of a plane (thousands of samples across a hair's width of the photo) next to a
/// wide one can't size the merged mask past its cap.
#[test]
fn a_sliver_plane_keeps_the_merged_mask_within_its_cap() {
    let sliver = Plane { rect: r(0.0, 0.1, 1e-7, 0.9), width: 64, height: 4, data: vec![1.0; 256], split: false };
    let wide = Plane { rect: r(0.0, 0.0, 1.0, 1.0), width: 4, height: 4, data: vec![0.5; 16], split: false };
    let m = super::part_mask(&[sliver, wide], Orientation::Normal).expect("a mask from the valid planes");
    let (w, h) = (m.plane.width, m.plane.height);
    assert!(w <= 1536 && h <= 1536 && w * h <= 1 << 20, "{w}×{h}");
    assert_eq!(m.plane.data.len(), w * h);
}

/// The parse keeps only the masks some feature reads: no clothes in the faces portrait retouching keeps.
#[test]
fn only_the_parts_a_feature_reads_are_kept() {
    let mut p = person(r(0.3, 0.2, 0.7, 0.8));
    p.parts.insert(Part::ClothesTop, vec![Plane { rect: r(0.2, 0.6, 0.8, 1.0), width: 4, height: 4, data: vec![1.0; 16], split: false }]);
    let parsing = Parsing { people: vec![p], ..parse_of(&[]) };
    let faces = super::faces_of(&parsing, &[], Orientation::Normal);
    assert!(faces[0].mask(RetouchPart::FacialSkin).is_some());
    assert!(faces[0].mask(RetouchPart::ClothesTop).is_none(), "no feature reads the clothes");
}

/// A face's own settings made while YuNet found the faces (boxes after lens corrections) stay with that face once
/// the parse finds them (boxes before), across a strong lens distortion; the neighbour standing where the YuNet box
/// lay does not take them.
#[test]
fn an_override_made_under_yunet_stays_with_its_face_under_the_parse_with_a_lens_distortion() {
    use lightcraft_develop::retouch::NormRect as Box2;
    use lightcraft_geom::Point;
    struct Boxes(Vec<Box2>);
    impl crate::retouch_input::FaceProvider for Boxes {
        fn name(&self) -> &str {
            "Boxes"
        }
        fn faces(&self, _: &Rgba8) -> Result<Vec<lightcraft_develop::retouch::Face>, String> {
            Ok(self.0.iter().map(|b| lightcraft_develop::retouch::Face::boxed(*b)).collect())
        }
    }
    let fake = Fake::new(Vec::new());
    let (mut s, id, dir) = grey("frames", 200, 150, &fake);
    s.execute("develop.set", &json!({"control": "optics.distortion", "value": -100})).unwrap();
    let d = s.develop_of(id).unwrap();
    let frame = s.mask_frame(id, &d).unwrap();
    let to_edit = |b: Box2| {
        let f = frame.for_mask(true);
        let pts = [(b.x0, b.y0), (b.x1, b.y0), (b.x0, b.y1), (b.x1, b.y1), (b.centre().0, b.y0), (b.x0, b.centre().1)];
        let m: Vec<Point> = pts.iter().map(|(x, y)| f.mask_to_norm(Point::new(*x, *y))).collect();
        Box2::new(
            m.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
            m.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
            m.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
            m.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
        )
    };
    let a_photo = Box2::new(0.03, 0.04, 0.07, 0.09);
    let a_edit = to_edit(a_photo);
    let (dx, dy) = (a_edit.centre().0 - a_photo.centre().0, a_edit.centre().1 - a_photo.centre().1);
    assert!(dx.abs() > a_photo.width() || dy.abs() > a_photo.height(), "the distortion moves the face past its box: {a_photo:?} → {a_edit:?}");
    // the neighbour stands, on the photo, where A's box lies after the lens
    let b_photo = a_edit;
    let b_edit = to_edit(b_photo);
    s.retouch_faces.set_provider(Arc::new(Boxes(vec![a_edit, b_edit])));
    let listed = s.execute("retouch.detect", &json!({})).unwrap();
    let near = |listed: &serde_json::Value, b: Box2| {
        listed["faces"].as_array().unwrap().iter().position(|f| (f["bounds"][0].as_f64().unwrap() - b.x0).abs() < 1e-6).unwrap()
    };
    let a = near(&listed, a_edit);
    s.execute("retouch.set", &json!({"control": "retouch.skinTone.faceLight", "value": 90, "face": a})).unwrap();
    // the parse takes over
    s.retouch_faces.provider = None;
    *fake.rects.lock().unwrap() = [a_photo, b_photo].iter().map(|b| r(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32)).collect();
    let parsed = s.execute("retouch.detect", &json!({})).unwrap();
    let parsed_box = |b: Box2| Box2::new(f64::from(b.x0 as f32), f64::from(b.y0 as f32), f64::from(b.x1 as f32), f64::from(b.y1 as f32));
    let (pa, pb) = (near(&parsed, parsed_box(a_photo)), near(&parsed, parsed_box(b_photo)));
    assert_eq!(parsed["faces"][pa]["own"], json!(0), "A keeps its settings: {parsed}");
    assert_eq!(parsed["faces"][pb]["own"], json!(null), "the neighbour does not take them: {parsed}");
    let own = s.develop_of(id).unwrap().retouch.faces[0].settings.get("retouch.skinTone.faceLight");
    assert_eq!(own, Some(90.0));
    let _ = std::fs::remove_dir_all(dir);
}

/// Blemishes' spots are found once, with the parse, on its picture (in linear light, turned like the masks), and
/// kept with the faces: a red spot on the face's skin is there, and a face without a Facial Skin mask has none.
#[test]
fn the_parse_finds_each_faces_spots_once() {
    use lightcraft_develop::retouch::input::SpotKind;
    let (w, h) = (400usize, 300usize);
    let spot = (0.5f32, 0.45f32);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5) / w as f32 - spot.0).hypot(((y as f32 + 0.5) / h as f32 - spot.1) * h as f32 / w as f32);
            let g = (-(d / 0.004).powi(2)).exp();
            data.extend([200.0 - 10.0 * g, 150.0 - 60.0 * g, 130.0 - 45.0 * g].map(|v: f32| v as u8));
        }
    }
    let img = lightcraft_parsing::RgbImage { width: w, height: h, data };
    let parsing = parse_of(&[r(0.3, 0.15, 0.7, 0.8)]);
    for orient in [Orientation::Normal, Orientation::Rotate90] {
        let found = super::found(&parsing, &[], &img, orient, &[]);
        let face = &found.input.faces[0];
        let spots = face.spots.as_ref().expect("the parse's faces have their spots found");
        let want = orient.map(f64::from(spot.0), f64::from(spot.1), 1.0, 1.0);
        let hit = spots.list.iter().find(|s| (s.at.0 - want.0).hypot(s.at.1 - want.1) < 0.02);
        assert_eq!(hit.map(|s| s.kind), Some(SpotKind::Blemish), "{orient:?}: {:?}", spots.list);
        assert!(spots.skin > 0.0);
    }
    let mut bare = person(r(0.3, 0.15, 0.7, 0.8));
    bare.parts.clear();
    let found = super::found(&Parsing { people: vec![bare], ..parse_of(&[]) }, &[], &img, Orientation::Normal, &[]);
    assert_eq!(found.input.faces[0].spots, None, "no skin mask, no spots looked for");
}

/// Stray Hair's strands are found once, with the parse, on its picture (in linear light, turned like the masks), and
/// kept with the faces: a dark strand across the face's skin is there, in the photo's frame, and a face without a
/// Hair or Facial Skin mask is not searched.
#[test]
fn the_parse_finds_each_faces_strands_once() {
    let (w, h) = (400usize, 300usize);
    // a dark hair across the forehead, a pixel wide
    let on_strand = |x: usize, y: usize| (0.4..0.6).contains(&((x as f32 + 0.5) / w as f32)) && y == (0.35 * h as f32) as usize;
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            data.extend(if on_strand(x, y) { [40u8, 30, 25] } else { [200, 150, 130] });
        }
    }
    let img = lightcraft_parsing::RgbImage { width: w, height: h, data };
    let parsing = parse_of(&[r(0.3, 0.15, 0.7, 0.8)]);
    for orient in [Orientation::Normal, Orientation::Rotate90] {
        let found = super::found(&parsing, &[], &img, orient, &[]);
        let strands = found.input.faces[0].strands.as_ref().expect("the parse's faces have their strands found");
        assert!(!strands.is_empty(), "{orient:?}: the strand is found");
        // a cell on the strand's middle has weight
        let (nx, ny) = orient.map(0.5, (0.35 * h as f64 + 0.5) / h as f64, 1.0, 1.0);
        let (cx, cy) = (
            (nx - strands.area.x0) / strands.area.width() * strands.width as f64,
            (ny - strands.area.y0) / strands.area.height() * strands.height as f64,
        );
        let near = (-2..=2)
            .flat_map(|dy| (-2..=2).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| strands.cell((cx as i64 + dx) as usize, (cy as i64 + dy) as usize));
        assert!(near.map(|c| c[0]).max().unwrap_or(0) > 128, "{orient:?}: the strand's cells are where the strand is");
    }
    let mut bare = person(r(0.3, 0.15, 0.7, 0.8));
    bare.parts.clear();
    let found = super::found(&Parsing { people: vec![bare], ..parse_of(&[]) }, &[], &img, Orientation::Normal, &[]);
    assert_eq!(found.input.faces[0].strands, None, "no hair or skin mask, no search");
}
