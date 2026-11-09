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
    assert!(VERSION >= 5);
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

/// Fork 6 (#141): Portrait Background's sliders and what Swap puts behind the person are saved and read back, written
/// as fork 6 so a fork 5 build refuses them instead of dropping them; a fork 5 library opens and is upgraded with
/// nothing lost.
#[test]
fn a_portrait_background_is_fork_format_6() {
    use lightcraft_develop::retouch::background::{Background, Swap};
    assert_eq!(VERSION, 6);
    let mut d = DevelopSettings::default();
    let b = Background { blur: 40.0, swap: 70.0, clean: 25.0, with: Swap::File { path: "/backdrops/grey paper.jpg".into() } };
    d.retouch.global.background = b.clone();
    let mut c = Catalog::new();
    let id = photo(&mut c, d, None);
    let m = MemStore::new();
    m.set(SNAPSHOT, snapshot(FORK_FORMAT, 5, 1, &c.to_snapshot()));
    let (mut j, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!((r.upgraded_from, loaded.to_snapshot()), (Some(5), c.to_snapshot()));
    assert_eq!(header(&m), (FORK_FORMAT.to_string(), 6));
    j.snapshot(&loaded).unwrap();
    drop(j);
    let (_, again, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, None);
    assert_eq!(again.photo(id).unwrap().develop.retouch.global.background, b);
}
