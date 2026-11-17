//! The fork's own catalog format name (`fork_format`): every older header the fork or an official build wrote reads
//! exactly, is rewritten as `lightcraft-catalog-fork` (keeping the first older snapshot as `catalog.snap.before-upgrade`),
//! and what this build can't read is refused untouched. Synthetic fixtures for each era.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, SegMask};

use crate::exif_override::ExifOverrides;
use crate::fork_format::{BEFORE_UPGRADE, FORK_FORMAT, OFFICIAL_FORMAT, OFFICIAL_VERSION, VERSION};
use crate::journal::{LOG, SNAPSHOT, encode_record};
use crate::*;

fn header(m: &MemStore) -> (String, u64) {
    let v: serde_json::Value = serde_json::from_slice(&m.get(SNAPSHOT).unwrap()).unwrap();
    (v["format"].as_str().unwrap().to_string(), v["version"].as_u64().unwrap())
}

fn snapshot(format: &str, version: u32, seq: u64, body: &str) -> Vec<u8> {
    format!("{{\"format\":\"{format}\",\"version\":{version},\"seq\":{seq},\"catalog\":{body}}}\n").into_bytes()
}

fn photo(c: &mut Catalog, settings: DevelopSettings, exif: Option<ExifOverrides>) -> PhotoId {
    let id = c.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: format!("/p/{}.arw", id.0) }, "p.arw", "ARW", 60, 40, "2026-01-01");
    p.kind = MediaKind::Raw;
    if let Some(e) = exif {
        p.meta.exif = e;
    }
    let settings = Arc::new(settings);
    p.develop = settings.clone();
    // (two steps over the same settings: a snapshot shares their grids)
    p.history = vec![HistoryStep { label: "Profile".into(), settings: settings.clone() }, HistoryStep { label: "Exposure".into(), settings }];
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

/// Settings with a camera colour (`local` / `macos`: the fork's camera profiles).
fn profiled(colour: &str, profile: Option<&str>) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.profile.camera_colour = serde_json::from_value(serde_json::json!(colour)).unwrap();
    s.profile.camera_profile = profile.map(Into::into);
    s
}

/// Settings with one AI mask over a segmentation.
fn masked(seg: SegMask) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.4;
    s.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Object {
                hint: vec![lightcraft_geom::Point::new(0.5, 0.5)],
                exclude: vec![],
                seg: Some(seg),
                detail: vec![],
                edge: 0.0,
            },
        }],
        ..Default::default()
    });
    s
}

fn seg() -> SegMask {
    SegMask::from_logits(32, &vec![0.5f32; 32 * 32])
}

/// What an edited camera value looks like (`Meta.exif`, the fork's 13 before 2026-11-08).
fn edited_camera() -> ExifOverrides {
    ExifOverrides { camera: Some("Sony A7 V".into()), iso: Some(800), ..Default::default() }
}

/// The body a snapshot writer of this build gives `c` (shared grid table included), for fixtures of builds that
/// wrote the same body under another header.
fn body_with_shared_grids(c: &Catalog) -> String {
    let m = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(m.clone())).unwrap();
    j.snapshot(c).unwrap();
    drop(j);
    let bytes = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    assert!(v["catalog"]["grids"].as_array().is_some_and(|g| !g.is_empty()), "the fixture shares its grids");
    let start = bytes.find("\"catalog\":").unwrap() + "\"catalog\":".len();
    bytes[start..bytes.len() - 2].to_string()
}

/// An older snapshot opens exactly as `want`, is rewritten as the fork's format, keeps its first copy, and reopens
/// as it was without another upgrade.
fn assert_upgrades_exactly(m: &MemStore, want: &Catalog, from: u32) {
    let original = m.get(SNAPSHOT).unwrap();
    let (j, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, Some(from));
    assert_eq!(loaded.to_snapshot(), want.to_snapshot(), "read exactly");
    assert_eq!(header(m), (FORK_FORMAT.to_string(), u64::from(VERSION)), "rewritten under the fork's own name");
    assert_eq!(m.get(BEFORE_UPGRADE), Some(original.clone()), "the snapshot as it was is kept");
    drop(j);
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    assert_eq!(again.to_snapshot(), want.to_snapshot(), "the rewrite holds the same library");
    assert_eq!(m.get(BEFORE_UPGRADE), Some(original), "the copy stays");
}

/// An official 12 (shared segmentation grids, `"format":"lightcraft-catalog"`): the masks get their grids back.
#[test]
fn an_official_v12_library_with_shared_grids_opens_exactly() {
    let mut c = Catalog::new();
    let id = photo(&mut c, masked(seg()), None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(OFFICIAL_FORMAT, 12, 1, &body_with_shared_grids(&c)));
    assert_upgrades_exactly(&m, &c, 12);
    let (_, loaded, _) = Journal::open(Box::new(m)).unwrap();
    let MaskShape::Object { seg: Some(s), .. } = &loaded.photo(id).unwrap().history[1].settings.masks[0].components[0].shape else {
        panic!("an Object mask")
    };
    assert_eq!((s.data.as_str(), s.grid), (seg().data.as_str(), None));
}

/// The fork's 12 before 2026-11-08 (local camera profiles and macOS Colour, `"format":"lightcraft-catalog"`).
#[test]
fn a_fork_v12_library_with_camera_profiles_opens_exactly() {
    let mut c = Catalog::new();
    let local = photo(&mut c, profiled("local", Some("A7 V Chart")), None);
    let macos = photo(&mut c, profiled("macos", None), None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(OFFICIAL_FORMAT, 12, 2, &c.to_snapshot()));
    assert_upgrades_exactly(&m, &c, 12);
    let (_, loaded, _) = Journal::open(Box::new(m)).unwrap();
    assert_eq!(loaded.photo(local).unwrap().develop.profile.camera_profile.as_deref(), Some("A7 V Chart"));
    let colour = |id| serde_json::to_value(loaded.photo(id).unwrap().develop.profile.camera_colour).unwrap();
    assert_eq!((colour(local), colour(macos)), (serde_json::json!("local"), serde_json::json!("macos")));
}

/// One 12 holding both meanings' fields (no build wrote it, but a reader by name reads it whole).
#[test]
fn a_v12_library_with_both_meanings_opens_exactly() {
    let mut c = Catalog::new();
    let mut s = masked(seg());
    s.profile = profiled("local", Some("A7 V Chart")).profile;
    photo(&mut c, s, None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(OFFICIAL_FORMAT, 12, 1, &body_with_shared_grids(&c)));
    assert_upgrades_exactly(&m, &c, 12);
}

/// The fork's 13 before 2026-11-08 (edited camera values on top of camera profiles, inline grids,
/// `"format":"lightcraft-catalog"`), with a log record on top: techneissa's library is one.
#[test]
fn a_fork_v13_library_with_a_log_opens_exactly() {
    let mut c = Catalog::new();
    let a = photo(&mut c, profiled("local", Some("A7 V Chart")), Some(edited_camera()));
    photo(&mut c, masked(seg()), None);
    let album = c.alloc_album_id();
    c.apply(Op::AddAlbum { album: Album { photos: vec![a], ..Album::new(album, "Travel") } }).unwrap();
    let body = c.to_snapshot();
    let op = Op::SetRating { id: a, rating: 4 };
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(OFFICIAL_FORMAT, 13, 3, &body));
    m.set(LOG, format!("{}\n", encode_record(4, &op)).into_bytes());
    c.apply(op).unwrap();
    assert_upgrades_exactly(&m, &c, 13);
    let (_, loaded, _) = Journal::open(Box::new(m)).unwrap();
    assert_eq!(loaded.photo(a).unwrap().meta.exif, edited_camera());
    assert_eq!(loaded.photo(a).unwrap().rating, 4);
}

/// An official 13 (Lens Blur in develop settings and a mask's adjustments, `"format":"lightcraft-catalog"`, official
/// update of 2026-11-12): its Lens Blur reads whole, beside the fork's own 13 that means camera profiles.
#[test]
fn an_official_v13_library_with_lens_blur_opens_exactly() {
    let mut s = masked(seg());
    s.lens_blur.enabled = true;
    s.lens_blur.amount = 60.0;
    s.lens_blur.focus_mask = Some(1);
    s.masks[0].adjust.lens_blur = 40.0;
    let mut c = Catalog::new();
    let id = photo(&mut c, s, None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(OFFICIAL_FORMAT, 13, 1, &body_with_shared_grids(&c)));
    assert_upgrades_exactly(&m, &c, 13);
    let (_, loaded, _) = Journal::open(Box::new(m)).unwrap();
    let d = &loaded.photo(id).unwrap().develop;
    assert_eq!((d.lens_blur.enabled, d.lens_blur.amount, d.lens_blur.focus_mask), (true, 60.0, Some(1)));
    assert_eq!(d.masks[0].adjust.lens_blur, 40.0);
}

/// This build's format opens as it is: no upgrade, no copy.
#[test]
fn the_fork_format_opens_without_an_upgrade() {
    let mut c = Catalog::new();
    photo(&mut c, profiled("local", Some("A7 V Chart")), Some(edited_camera()));
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, VERSION, 1, &c.to_snapshot()));
    let before = m.files.lock().unwrap().clone();
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (None, c.to_snapshot()));
    assert_eq!(*m.files.lock().unwrap(), before, "nothing rewritten, no copy");
    // a new library is written in it too
    let m = MemStore::new();
    Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
}

/// What an official build does with a snapshot (a mirror of its `Journal::open` header check): it reads `format`
/// and `version` first and refuses a format name it doesn't know. A fork library is refused, never read in part.
#[test]
fn an_official_build_refuses_the_fork_format() {
    #[derive(serde::Deserialize)]
    struct Head {
        format: String,
        version: u32,
    }
    let official_reads = |bytes: &[u8]| {
        let h: Head = serde_json::from_slice(bytes).unwrap();
        h.format == OFFICIAL_FORMAT && h.version <= OFFICIAL_VERSION
    };
    let m = MemStore::new();
    Journal::open(Box::new(m.clone())).unwrap();
    assert!(!official_reads(&m.get(SNAPSHOT).unwrap()));
    assert!(official_reads(&snapshot(OFFICIAL_FORMAT, 12, 0, "{}")), "the mirror reads an official 12");
}

fn assert_untouched(m: &MemStore, newer: bool) {
    let before = m.files.lock().unwrap().clone();
    let e = Journal::open(Box::new(m.clone())).err().expect("refused");
    assert_eq!(matches!(e, CatalogError::Newer(_)), newer, "{e:?}");
    assert_eq!(*m.files.lock().unwrap(), before, "nothing modified, no copy");
}

/// A newer fork format, an official format past what this build knows and an unknown format name are refused, and
/// the library is left as it was.
#[test]
fn what_this_build_cannot_read_is_refused_untouched() {
    let c = Catalog::new();
    for (format, version, newer) in [(FORK_FORMAT, VERSION + 1, true), (OFFICIAL_FORMAT, 15, true), ("lightcraft-catalog-other", 1, false)] {
        let m = MemStore::new();
        m.set(SNAPSHOT, snapshot(format, version, 0, &c.to_snapshot()));
        assert_untouched(&m, newer);
    }
}

/// A copy kept by an earlier upgrade stays: the first older snapshot is the one worth keeping.
#[test]
fn a_later_upgrade_keeps_the_first_copy() {
    let mut c = Catalog::new();
    photo(&mut c, profiled("macos", None), None);
    let m = MemStore::new();
    m.set(BEFORE_UPGRADE, b"the first one".to_vec());
    m.set(SNAPSHOT, snapshot(OFFICIAL_FORMAT, 11, 1, &c.to_snapshot()));
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(11), c.to_snapshot()));
    assert_eq!(m.get(BEFORE_UPGRADE).as_deref(), Some(&b"the first one"[..]));
}

/// A store that can't write the copy.
struct NoCopy(MemStore);

impl Store for NoCopy {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.0.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if name == BEFORE_UPGRADE {
            return Err(std::io::Error::other("disk full"));
        }
        self.0.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.0.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.0.truncate(name, len)
    }
    fn describe(&self) -> String {
        "no copy".into()
    }
}

/// When the copy can't be written, the older library isn't upgraded: it is left as it was, with an error that says so.
#[test]
fn an_upgrade_without_its_copy_leaves_the_library_unchanged() {
    let mut c = Catalog::new();
    photo(&mut c, profiled("local", Some("A7 V Chart")), Some(edited_camera()));
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(OFFICIAL_FORMAT, 13, 1, &c.to_snapshot()));
    let before = m.files.lock().unwrap().clone();
    let e = Journal::open(Box::new(NoCopy(m.clone()))).err().expect("refused");
    assert!(matches!(&e, CatalogError::Io(msg) if msg.contains("left unchanged")), "{e:?}");
    assert_eq!(*m.files.lock().unwrap(), before);
}

/// Fork 3 (#90): a library with a residual Look is written as fork 3 or later, so a fork 2 build refuses it instead of
/// dropping the Look; a fork 2 library opens and is upgraded with nothing lost.
#[test]
fn a_residual_look_is_fork_format_3() {
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.look.cast[1] = [0.01, -0.02];
    photo(&mut c, d, None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 2, 1, &c.to_snapshot()));
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(2), c.to_snapshot()));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
}

/// Fork 4 (#115): a Background mask decontaminated at 100 is saved and read back with its amount, written as fork 4 so a
/// fork 3 build refuses it instead of dropping the amount; a fork 3 library opens and is upgraded with nothing lost.
#[test]
fn a_decontaminated_mask_is_fork_format_4() {
    const { assert!(VERSION >= 4) };
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    let shape = MaskShape::background();
    d.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
        decontaminate: 100.0,
        ..Default::default()
    });
    let id = photo(&mut c, d, None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 3, 1, &c.to_snapshot()));
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(3), c.to_snapshot()));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
    // and the upgraded library reads back the amount
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    assert_eq!(again.photo(id).unwrap().develop.masks[0].decontaminate, 100.0);
}

/// Fork 5 (#123): a Select People mask (its face, its grids and its Edge) is saved and read back whole, written as fork 5
/// so a fork 4 build refuses it instead of reading it as the old People shape; a fork 4 library opens and is upgraded
/// with nothing lost. Its grids go in the shared table, once for both History steps.
#[test]
fn a_select_people_mask_is_fork_format_5() {
    const { assert!(VERSION >= 5) };
    let seg = SegMask::from_logits(8, &[3.0; 64]).made_by("people:segformer");
    let fine = SegMask::from_logits_in(8, &[-3.0; 64], [0.1, 0.1, 0.3, 0.3]).made_by("people:segformer");
    let face = lightcraft_develop::retouch::FaceKey {
        person: Some("ann".into()),
        bounds: lightcraft_develop::retouch::NormRect::new(0.1, 0.1, 0.3, 0.3),
        frame: lightcraft_develop::retouch::KeyFrame::Photo,
    };
    let shape =
        MaskShape::People { person: 0, parts: vec!["hair".into()], face: Some(face), seg: Some(seg.clone()), detail: vec![fine.clone()], edge: 25.0 };
    let mut d = DevelopSettings::default();
    d.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: shape.clone() }],
        ..Default::default()
    });
    let mut c = Catalog::new();
    let id = photo(&mut c, d, None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 4, 1, &c.to_snapshot()));
    let (mut j, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(4), c.to_snapshot()));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
    j.snapshot(&loaded).unwrap();
    drop(j);
    let body = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    assert_eq!(body.match_indices(seg.data.as_str()).count(), 1, "the grid once, in the shared table");
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    let p = again.photo(id).unwrap();
    assert_eq!(p.develop.masks[0].components[0].shape, shape);
    assert!(p.history.iter().all(|h| h.settings.masks[0].components[0].shape == shape), "every History step");
}

/// Fork 6 (`fork/retouch-reshape.md`): Face Reshape's sliders, for every face and for a face of its own, are saved and
/// read back, written as fork 6 so a fork 5 build refuses them instead of dropping them; a fork 5 library opens and is
/// upgraded with nothing lost.
#[test]
fn a_face_reshape_is_fork_format_6() {
    const { assert!(VERSION >= 6) };
    let mut d = DevelopSettings::default();
    lightcraft_develop::controls::set(&mut d, "retouch.reshape.slimFace", 40.0);
    lightcraft_develop::controls::set(&mut d, "retouch.reshape.eyeSize", -25.0);
    let face = lightcraft_develop::retouch::Face::boxed(lightcraft_develop::retouch::NormRect::new(0.2, 0.2, 0.4, 0.5));
    if let Some(own) = d.retouch.override_mut(std::slice::from_ref(&face), 0) {
        own.reshape.chin = 30.0;
    }
    let mut c = Catalog::new();
    let id = photo(&mut c, d.clone(), None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 5, 1, &c.to_snapshot()));
    let (mut j, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(5), c.to_snapshot()));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
    j.snapshot(&loaded).unwrap();
    drop(j);
    let body = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    assert!(body.contains("slimFace") && body.contains("eyeSize"), "written by name");
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    let got = &again.photo(id).unwrap().develop.retouch;
    assert_eq!(got, &d.retouch);
    assert_eq!((got.global.reshape.slim_face, got.faces[0].settings.reshape.chin), (40.0, 30.0));
}

/// Fork 7 (#141): Portrait Background's sliders and what Swap puts behind the person are saved and read back, written
/// as fork 7 so a fork 6 build refuses them instead of dropping them; a fork 6 library opens and is upgraded with
/// nothing lost.
#[test]
fn a_portrait_background_is_fork_format_7() {
    use lightcraft_develop::retouch::background::{Background, Swap};
    const { assert!(VERSION >= 7) };
    let mut d = DevelopSettings::default();
    let b = Background { blur: 40.0, swap: 70.0, clean: 25.0, with: Swap::Picture { hash: "ab".repeat(32) } };
    d.retouch.global.background = b.clone();
    let mut c = Catalog::new();
    let id = photo(&mut c, d, None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 6, 1, &c.to_snapshot()));
    let (mut j, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(6), c.to_snapshot()));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
    j.snapshot(&loaded).unwrap();
    drop(j);
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    assert_eq!(again.photo(id).unwrap().develop.retouch.global.background, b);
}

/// Fork 8 (#99): Refine strokes, for every face and a face's own, are saved and read back whole, written as fork 8 so
/// an older build refuses them instead of dropping them; a fork 7 library opens and is upgraded with nothing lost.
#[test]
fn refine_strokes_are_fork_format_8() {
    use lightcraft_develop::retouch::{Face, NormRect, RefineMode, RefineStroke};
    const { assert!(VERSION >= 8) };
    let mut d = DevelopSettings::default();
    let stroke = |mode| RefineStroke { mode, points: vec![(0.2, 0.3), (0.4, 0.35)], pressure: vec![0.5, 0.9], ..RefineStroke::default() };
    d.retouch.global.set("retouch.skin.smoothness", 40.0);
    assert!(d.retouch.refine.push("skin", stroke(RefineMode::Reduce)));
    let face = Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.3));
    assert!(d.retouch.own_mut_in(std::slice::from_ref(&face), 0, None).unwrap().refine.push("teethLips", stroke(RefineMode::Add)));
    let mut c = Catalog::new();
    let id = photo(&mut c, d.clone(), None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 7, 1, &c.to_snapshot()));
    let (mut j, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(7), c.to_snapshot()));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
    j.snapshot(&loaded).unwrap();
    drop(j);
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    let p = again.photo(id).unwrap();
    assert_eq!(p.develop.retouch, d.retouch);
    assert!(p.history.iter().all(|h| h.settings.retouch == d.retouch), "every History step");
}

/// Fork 9 (`fork/ai-culling.md`): Assisted Culling's eyes are saved with the photo's other culling scores and read
/// back, written as fork 9 so an older fork build refuses them instead of dropping them; a fork 8 library, whose
/// scores have no eyes, opens and is upgraded with nothing lost.
#[test]
fn culling_eyes_are_fork_format_9() {
    const { assert!(VERSION >= 9) };
    let mut c = Catalog::new();
    let id = photo(&mut c, DevelopSettings::default(), None);
    let old = Analysis { sharpness: 61.0, clipped: 0.01, group: Some(3), best: true, eyes: None };
    c.apply(Op::SetAnalysis { id, analysis: Some(old) }).unwrap();
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 8, 1, &c.to_snapshot()));
    let (mut j, mut loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.photo(id).unwrap().analysis), (Some(8), Some(old)));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), u64::from(VERSION)));
    let eyes = Eyes { faces: 2, main: 1, closed: Some(true), focus: Some(22.5) };
    let op = Op::SetAnalysis { id, analysis: Some(Analysis { eyes: Some(eyes), ..old }) };
    loaded.apply(op.clone()).unwrap();
    j.append(std::slice::from_ref(&op)).unwrap();
    j.snapshot(&loaded).unwrap();
    drop(j);
    let body = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    assert!(body.contains("\"closed\":true") && body.contains("\"focus\":22.5"), "written by name");
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    assert_eq!(again.photo(id).unwrap().analysis.and_then(|a| a.eyes), Some(eyes));
    assert_eq!(eyes.out_of_focus(), Some(true));
    assert_eq!(Eyes { focus: Some(f32::NAN), ..eyes }.out_of_focus(), None, "a NaN score is no score");
}

/// Fork 12 (#299): a fork 11 library whose log holds patches of Relight's block in its first form (one light's numbers
/// and `darken` in one object) opens with them applied to the first light, for a develop edit and for a History step.
#[test]
fn a_fork_11_log_patch_of_relight_lands_on_the_first_light() {
    use lightcraft_develop::relight::first_form;
    fn first_forms(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(o) => {
                if let Some(flat) = o.get("fork_relight").and_then(first_form) {
                    o.insert("fork_relight".into(), flat);
                }
                o.values_mut().for_each(first_forms);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(first_forms),
            _ => {}
        }
    }
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.relight.lights[0].amount = 60.0;
    let id = photo(&mut c, d, None);
    // the snapshot as fork 11 wrote it
    let mut body: serde_json::Value = serde_json::from_str(&c.to_snapshot()).unwrap();
    first_forms(&mut body);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 11, 1, &body.to_string()));
    let line = |seq: u64, op: &str| format!("{{\"seq\":{seq},\"crc\":{},\"op\":{op}}}\n", crc32fast::hash(op.as_bytes()));
    let amount = |v: f64| format!(r#"{{"o":{{"s":{{"fork_relight":{{"o":{{"s":{{"amount":{{"v":{v:?}}}}}}}}}}}}}}}"#);
    let edit = format!(r#"{{"op":"patchDevelop","id":{},"patch":{},"label":"Relight","edited":null}}"#, id.0, amount(70.0));
    let push = format!(r#"{{"op":"patchPush","id":{},"label":"Relight","base":{{"patch":{}}}}}"#, id.0, amount(80.0));
    m.set(LOG, [line(2, &edit), line(3, &push)].concat().into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.replayed, r.failed), (2, 0), "{r:?}");
    let p = loaded.photo(id).unwrap();
    assert_eq!(p.develop.relight.lights[0].amount, 70.0);
    assert_eq!(p.history.last().unwrap().settings.relight.lights[0].amount, 80.0);
    // and a fork 11 snapshot read without a log is one light, as it was
    assert_eq!(r.upgraded_from, Some(11));
}

/// Fork 13 (#302) made a fork 12 library an older one, but fork 12 already wrote the lights list: its log's patches name
/// `lights` and replay as they are, not against Relight's first (one-light) form. A fork 12 snapshot with two lights and a
/// log patch on the second opens with the patch applied and nothing set aside.
#[test]
fn a_fork_12_log_patch_of_a_second_light_is_applied_and_nothing_is_set_aside() {
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.relight.lights[0].amount = 60.0;
    d.relight.lights.push(lightcraft_develop::relight::LightSettings { amount: 10.0, ..Default::default() });
    let id = photo(&mut c, d.clone(), None);
    let m = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(m.clone())).unwrap();
    j.snapshot(&c).unwrap();
    d.relight.lights[1].amount = 70.0;
    j.append(&[Op::SetDevelop { id, settings: Arc::new(d), label: "Relight".into(), edited: None }]).unwrap();
    drop(j);
    // the library as fork 12 left it: this snapshot's header, and the log written under it
    let snap = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    assert!(snap.contains(&format!("\"version\":{}", crate::fork_format::VERSION)), "{}", &snap[..snap.len().min(120)]);
    m.set(SNAPSHOT, snap.replacen(&format!("\"version\":{}", crate::fork_format::VERSION), "\"version\":12", 1).into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.replayed, r.failed), (1, 0), "{r:?}");
    assert_eq!(r.upgraded_from, Some(12));
    let lights = &loaded.photo(id).unwrap().develop.relight.lights;
    assert_eq!((lights.len(), lights[0].amount, lights[1].amount), (2, 60.0, 70.0), "the patch on the second light was lost");
    assert!(
        !m.files.lock().unwrap().keys().any(|k| k.contains("damaged")),
        "a log was set aside: {:?}",
        m.files.lock().unwrap().keys().collect::<Vec<_>>()
    );
}

/// Fork 14 (#300) made a fork 13 library an older one, but fork 13 already wrote the lights list with gels: its log's
/// patches name `lights` and replay as they are. A fork 13 snapshot with a gel on its second light and a log patch on that
/// light opens with the patch applied, the gel kept, and nothing set aside.
#[test]
fn a_fork_13_log_patch_of_a_light_with_a_gel_is_applied_and_nothing_is_set_aside() {
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.relight.lights[0].amount = 60.0;
    let mut second = lightcraft_develop::relight::LightSettings { amount: 10.0, ..Default::default() };
    (second.effects.gel_hue, second.effects.gel_strength) = (178.0, 65.0);
    d.relight.lights.push(second);
    let id = photo(&mut c, d.clone(), None);
    let m = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(m.clone())).unwrap();
    j.snapshot(&c).unwrap();
    d.relight.lights[1].amount = 70.0;
    j.append(&[Op::SetDevelop { id, settings: Arc::new(d), label: "Relight".into(), edited: None }]).unwrap();
    drop(j);
    let snap = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    let now = format!("\"version\":{}", crate::fork_format::VERSION);
    assert!(snap.contains(&now), "{}", &snap[..snap.len().min(120)]);
    m.set(SNAPSHOT, snap.replacen(&now, "\"version\":13", 1).into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.replayed, r.failed), (1, 0), "{r:?}");
    assert_eq!(r.upgraded_from, Some(13));
    let lights = &loaded.photo(id).unwrap().develop.relight.lights;
    assert_eq!((lights.len(), lights[0].amount, lights[1].amount), (2, 60.0, 70.0), "the patch on the second light was lost");
    assert_eq!((lights[1].effects.gel_hue, lights[1].effects.gel_strength, lights[1].kind.name()), (178.0, 65.0, "point"));
    assert!(
        !m.files.lock().unwrap().keys().any(|k| k.contains("damaged")),
        "a log was set aside: {:?}",
        m.files.lock().unwrap().keys().collect::<Vec<_>>()
    );
}

/// Fork 16 (#315) made a fork 14 library an older one, but fork 14 already wrote the lights list with types: its log's
/// patches name `lights` and replay as they are. A fork 14 snapshot with a spot as the second light and a log patch on that
/// light opens with the patch applied, the type kept, no shadows, and nothing set aside.
#[test]
fn a_fork_14_log_patch_of_a_spot_light_is_applied_and_nothing_is_set_aside() {
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.relight.lights[0].amount = 60.0;
    let mut second = lightcraft_develop::relight::LightSettings { amount: 10.0, ..Default::default() };
    second.kind.which = 1.0;
    second.kind.spot_angle = 22.0;
    d.relight.lights.push(second);
    let id = photo(&mut c, d.clone(), None);
    let m = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(m.clone())).unwrap();
    j.snapshot(&c).unwrap();
    d.relight.lights[1].amount = 70.0;
    j.append(&[Op::SetDevelop { id, settings: Arc::new(d), label: "Relight".into(), edited: None }]).unwrap();
    drop(j);
    let snap = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    let now = format!("\"version\":{}", crate::fork_format::VERSION);
    assert!(snap.contains(&now), "{}", &snap[..snap.len().min(120)]);
    m.set(SNAPSHOT, snap.replacen(&now, "\"version\":14", 1).into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.replayed, r.failed), (1, 0), "{r:?}");
    assert_eq!(r.upgraded_from, Some(14));
    let lights = &loaded.photo(id).unwrap().develop.relight.lights;
    assert_eq!((lights.len(), lights[0].amount, lights[1].amount), (2, 60.0, 70.0), "the patch on the second light was lost");
    assert_eq!((lights[1].kind.name(), lights[1].kind.spot_angle, lights[1].effects.shadows), ("spot", 22.0, 0.0));
    assert!(
        !m.files.lock().unwrap().keys().any(|k| k.contains("damaged")),
        "a log was set aside: {:?}",
        m.files.lock().unwrap().keys().collect::<Vec<_>>()
    );
}

/// Fork 18 (#317) made a fork 17 library an older one, but fork 17 already wrote haze, shadows and a pattern's Openness per
/// light: its log's patches replay as they are. A fork 17 snapshot with a hazy, shadowed light through blinds at Openness 20
/// and a log patch on its haze opens with the patch applied, the Openness kept, Forward Scatter and Dust at their defaults,
/// and nothing set aside.
#[test]
fn a_fork_17_log_patch_of_a_hazy_patterned_light_is_applied_and_nothing_is_set_aside() {
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.relight.lights[0].amount = 60.0;
    (d.relight.lights[0].effects.haze, d.relight.lights[0].effects.shadows) = (30.0, 80.0);
    (d.relight.lights[0].effects.pattern, d.relight.lights[0].effects.pattern_open) = (1.0, 20.0);
    let id = photo(&mut c, d.clone(), None);
    let m = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(m.clone())).unwrap();
    j.snapshot(&c).unwrap();
    d.relight.lights[0].effects.haze = 55.0;
    j.append(&[Op::SetDevelop { id, settings: Arc::new(d), label: "Relight".into(), edited: None }]).unwrap();
    drop(j);
    let snap = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    let now = format!("\"version\":{}", crate::fork_format::VERSION);
    assert!(snap.contains(&now), "{}", &snap[..snap.len().min(120)]);
    m.set(SNAPSHOT, snap.replacen(&now, "\"version\":17", 1).into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.replayed, r.failed), (1, 0), "{r:?}");
    assert_eq!(r.upgraded_from, Some(17));
    let e = loaded.photo(id).unwrap().develop.relight.lights[0].effects;
    assert_eq!((e.haze, e.shadows, e.pattern_open), (55.0, 80.0, 20.0), "the patch on the haze was lost");
    assert_eq!((e.scatter, e.dust), (50.0, 0.0));
    assert!(
        !m.files.lock().unwrap().keys().any(|k| k.contains("damaged")),
        "a log was set aside: {:?}",
        m.files.lock().unwrap().keys().collect::<Vec<_>>()
    );
}

/// A store whose snapshot file can't be written while `full` is set (the log's small lines still fit).
#[derive(Clone, Default)]
struct FullForSnapshots {
    files: MemStore,
    full: Arc<std::sync::atomic::AtomicBool>,
}

impl Store for FullForSnapshots {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if name == SNAPSHOT && self.full.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "full for snapshots".into()
    }
}

/// An upgrade that can't write its snapshot (a nearly full disk) leaves the old files as they are and takes no record
/// under them: a log of two formats can't be replayed. The edit stays with the caller; once a snapshot can be written it
/// carries the edit, and every edit that was saved is there on the next open.
#[test]
fn a_failed_upgrade_snapshot_leaves_no_log_of_two_formats() {
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.relight.lights[0].amount = 60.0;
    let id = photo(&mut c, d, None);
    let mut body: serde_json::Value = serde_json::from_str(&c.to_snapshot()).unwrap();
    fn flat(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(o) => {
                if let Some(f) = o.get("fork_relight").and_then(lightcraft_develop::relight::first_form) {
                    o.insert("fork_relight".into(), f);
                }
                o.values_mut().for_each(flat);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(flat),
            _ => {}
        }
    }
    flat(&mut body);
    let store = FullForSnapshots::default();
    store.files.set(SNAPSHOT, snapshot(FORK_FORMAT, 11, 1, &body.to_string()));
    store.full.store(true, std::sync::atomic::Ordering::SeqCst);
    let (mut j, mut live, r) = Journal::open(Box::new(store.clone())).unwrap();
    assert_eq!(r.upgraded_from, Some(11));
    assert!(j.upgrade_pending());
    // an edit made now is refused by the journal, and nothing of it is in the files
    let mut edited = (*live.photo(id).unwrap().develop).clone();
    edited.relight.lights[0].amount = 80.0;
    let op = Op::SetDevelop { id, settings: Arc::new(edited), label: "Relight".into(), edited: None };
    live.apply(op.clone()).unwrap();
    assert!(j.append(std::slice::from_ref(&op)).is_err());
    assert!(store.files.get(LOG).is_none_or(|l| l.is_empty()), "nothing is appended under the older snapshot");
    assert_eq!(header(&store.files).1, 11);
    // (closed here, the library opens as it was: the edit was never saved)
    let (_, back, r) = Journal::open(Box::new(store.clone())).unwrap();
    assert_eq!((r.failed, back.photo(id).unwrap().develop.relight.lights[0].amount), (0, 60.0));
    // the disk has room again: a snapshot of what is in memory carries the queued edit and the library is current
    store.full.store(false, std::sync::atomic::Ordering::SeqCst);
    j.snapshot_with_unlogged(&live, 1).unwrap();
    assert!(!j.upgrade_pending());
    assert_eq!(header(&store.files), (FORK_FORMAT.to_string(), u64::from(VERSION)));
    // later edits are appended as patches of this format, and everything saved is there on the next open
    let mut later = (*live.photo(id).unwrap().develop).clone();
    later.relight.lights[0].amount = 90.0;
    let op = Op::SetDevelop { id, settings: Arc::new(later), label: "Relight".into(), edited: None };
    live.apply(op.clone()).unwrap();
    j.append(&[op]).unwrap();
    let (_, again, r) = Journal::open(Box::new(store.clone())).unwrap();
    assert_eq!((r.failed, r.upgraded_from), (0, None));
    assert_eq!(again.photo(id).unwrap().develop.relight.lights[0].amount, 90.0);
}

/// Fork 22 (#331) made a fork 18 library an older one, but fork 18 already wrote haze, Forward Scatter and Dust per light: its
/// log's patches replay as they are. A fork 18 snapshot with a hazy light at Forward Scatter 80 and Dust 30 and a log patch on
/// its haze opens with the patch applied, Scatter and Dust kept, Low Mist, Mist Height and Distance Fade at their defaults, and
/// nothing set aside.
#[test]
fn a_fork_18_log_patch_of_a_hazy_light_is_applied_and_nothing_is_set_aside() {
    let mut c = Catalog::new();
    let mut d = DevelopSettings::default();
    d.relight.lights[0].amount = 60.0;
    (d.relight.lights[0].effects.haze, d.relight.lights[0].effects.scatter, d.relight.lights[0].effects.dust) = (30.0, 80.0, 30.0);
    let id = photo(&mut c, d.clone(), None);
    let m = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(m.clone())).unwrap();
    j.snapshot(&c).unwrap();
    d.relight.lights[0].effects.haze = 55.0;
    j.append(&[Op::SetDevelop { id, settings: Arc::new(d), label: "Relight".into(), edited: None }]).unwrap();
    drop(j);
    let snap = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    let now = format!("\"version\":{}", crate::fork_format::VERSION);
    assert!(snap.contains(&now), "{}", &snap[..snap.len().min(120)]);
    m.set(SNAPSHOT, snap.replacen(&now, "\"version\":18", 1).into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.replayed, r.failed), (1, 0), "{r:?}");
    assert_eq!(r.upgraded_from, Some(18));
    let e = loaded.photo(id).unwrap().develop.relight.lights[0].effects;
    assert_eq!((e.haze, e.scatter, e.dust), (55.0, 80.0, 30.0), "the patch on the haze was lost");
    assert_eq!((e.mist, e.mist_height, e.fade), (0.0, 30.0, 0.0));
    assert!(
        !m.files.lock().unwrap().keys().any(|k| k.contains("damaged")),
        "a log was set aside: {:?}",
        m.files.lock().unwrap().keys().collect::<Vec<_>>()
    );
}
