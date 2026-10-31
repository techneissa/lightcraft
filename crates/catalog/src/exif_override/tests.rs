use super::*;
use crate::journal::{LOG, SNAPSHOT, VERSION};
use crate::{Catalog, Filter, Journal, MemStore, Op, Photo, PhotoId, Rule, Source};

fn meta() -> Meta {
    Meta {
        camera: "SONY ILCE-7M5".into(),
        lens: "FE 24-70mm F2.8 GM II".into(),
        focal_mm: Some(35.0),
        aperture: Some(2.8),
        shutter: "1/250".into(),
        iso: Some(400),
        ..Default::default()
    }
}

#[test]
fn an_override_shows_and_the_file_value_stays() {
    let mut m = meta();
    m.set_exif(ExifField::Camera, Some("  Canon   EOS R5 ")).unwrap();
    m.set_exif(ExifField::Iso, Some("ISO 1600")).unwrap();
    assert_eq!((m.effective_camera(), m.effective_iso()), ("Canon EOS R5", Some(1600)));
    assert_eq!((m.camera.as_str(), m.iso), ("SONY ILCE-7M5", Some(400)), "the file's values are untouched");
    assert!(m.exif_overridden(ExifField::Camera) && !m.exif_overridden(ExifField::Lens));
    let shown = m.shown();
    assert_eq!((shown.camera.as_str(), shown.iso, shown.lens.as_str()), ("Canon EOS R5", Some(1600), "FE 24-70mm F2.8 GM II"));
}

#[test]
fn reset_one_field_and_all() {
    let mut m = meta();
    for (f, t) in
        [(ExifField::Camera, "X"), (ExifField::Lens, "Y"), (ExifField::Focal, "50"), (ExifField::Aperture, "f/4"), (ExifField::Shutter, "1/60")]
    {
        m.set_exif(f, Some(t)).unwrap();
    }
    m.reset_exif(ExifField::Lens);
    assert_eq!(m.effective_lens(), "FE 24-70mm F2.8 GM II");
    assert_eq!(m.effective_camera(), "X");
    m.set_exif(ExifField::Camera, None).unwrap();
    assert_eq!(m.effective_camera(), "SONY ILCE-7M5");
    m.set_exif(ExifField::Focal, Some("   ")).unwrap();
    assert_eq!(m.effective_focal_mm(), Some(35.0), "an empty text resets");
    for f in ExifField::ALL {
        m.reset_exif(f);
    }
    assert!(m.exif.is_empty());
    assert_eq!(m.shown().as_ref(), &meta(), "nothing edited shows the file as it is");
    assert!(matches!(m.shown(), Cow::Borrowed(_)), "and costs no copy");
}

#[test]
fn a_value_equal_to_the_file_is_no_override() {
    let mut m = meta();
    m.set_exif(ExifField::Iso, Some("400")).unwrap();
    m.set_exif(ExifField::Shutter, Some("1/250 s")).unwrap();
    m.set_exif(ExifField::Aperture, Some("F2.8")).unwrap();
    m.set_exif(ExifField::Camera, Some("SONY  ILCE-7M5")).unwrap();
    m.set_exif(ExifField::Camera, Some("Sony ILCE-7M5")).unwrap();
    m.set_exif(ExifField::Lens, Some("fe 24-70mm f2.8 gm ii")).unwrap();
    assert!(m.exif.is_empty(), "{:?}", m.exif);
}

#[test]
fn a_bad_value_changes_nothing() {
    let mut m = meta();
    m.set_exif(ExifField::Iso, Some("800")).unwrap();
    let before = m.clone();
    for (f, t) in [
        (ExifField::Iso, "abc"),
        (ExifField::Iso, "0"),
        (ExifField::Iso, "-5"),
        (ExifField::Iso, "400.5"),
        (ExifField::Iso, "99999999999999999999"),
        (ExifField::Focal, "0"),
        (ExifField::Focal, "nan"),
        (ExifField::Focal, "inf"),
        (ExifField::Focal, "-35"),
        (ExifField::Focal, "9999999"),
        (ExifField::Aperture, "f/0"),
        (ExifField::Aperture, "f/"),
        (ExifField::Aperture, "1e999"),
        (ExifField::Shutter, "1/0"),
        (ExifField::Shutter, "0"),
        (ExifField::Shutter, "-1"),
        (ExifField::Shutter, "1/"),
        (ExifField::Shutter, "/"),
        (ExifField::Shutter, "1/2/3"),
        (ExifField::Shutter, "999999999999"),
        (ExifField::Shutter, "NaN"),
        (ExifField::Camera, "bad\u{0}name"),
    ] {
        assert!(m.set_exif(f, Some(t)).is_err(), "{f:?} `{t}` should be refused");
        assert_eq!(m, before, "{f:?} `{t}` changed something");
    }
    let long = "x".repeat(10_000);
    for f in ExifField::ALL {
        assert!(m.set_exif(f, Some(&long)).is_err());
    }
    assert_eq!(m, before);
}

#[test]
fn hostile_text_never_panics() {
    let mut m = meta();
    for t in ["ƒ/", "ƒ", "\"", "s", "sec", "1//2", "ℵ", "🦀/🦀", "1/١٢", "0x10", "1_000", "+", "-", ".", "e", "1e", "\u{202e}1/250", "∞"]
    {
        for f in ExifField::ALL {
            let _ = m.set_exif(f, Some(t));
            m.reset_exif(f);
        }
    }
}

#[test]
fn parsing_each_field() {
    assert_eq!(parse_focal("35"), Ok(35.0));
    assert_eq!(parse_focal("35 mm"), Ok(35.0));
    assert_eq!(parse_focal("70.5MM"), Ok(70.5));
    assert_eq!(parse_aperture("f/2.8"), Ok(2.8));
    assert_eq!(parse_aperture("2.8"), Ok(2.8));
    assert_eq!(parse_aperture("F 1.4"), Ok(1.4));
    assert_eq!(parse_aperture("ƒ/0.95"), Ok(0.95));
    assert_eq!(parse_iso("100"), Ok(100));
    assert_eq!(parse_iso("iso 51200"), Ok(51200));
    assert_eq!(parse_shutter("1/250").as_deref(), Ok("1/250"));
    assert_eq!(parse_shutter("1/250 s").as_deref(), Ok("1/250"));
    assert_eq!(parse_shutter("0.004").as_deref(), Ok("1/250"));
    assert_eq!(parse_shutter("2.5\"").as_deref(), Ok("2.5"));
    assert_eq!(parse_shutter("2.5 s").as_deref(), Ok("2.5"));
    assert_eq!(parse_shutter("30 sec").as_deref(), Ok("30"));
    assert_eq!(parse_shutter("1").as_deref(), Ok("1"));
    assert_eq!(parse_shutter("0.3").as_deref(), Ok("0.3"));
    assert_eq!(parse_shutter("1/8000").as_deref(), Ok("1/8000"));
    assert_eq!(parse_shutter("2.5″").as_deref(), Ok("2.5"), "a typographic inch mark");
    // the text reads back to the same value
    let mut m = meta();
    m.set_exif(ExifField::Shutter, Some("2.5\"")).unwrap();
    let t = m.exif_text(ExifField::Shutter, false);
    assert_eq!(parse_shutter(&t).as_deref(), Ok("2.5"));
    assert_eq!(m.exif_text(ExifField::Shutter, true), "1/250");
}

fn photo() -> Photo {
    let mut p = Photo::new(PhotoId(1), Source::File { path: "/x/a.arw".into() }, "a.arw", "ARW", 6000, 4000, "2026-10-30T00:00:00");
    p.meta = meta();
    p
}

/// Applies `op` to `c` and logs it, as the engine's commit does.
fn commit(j: &mut Journal, c: &mut Catalog, op: Op) {
    c.apply(op.clone()).unwrap();
    j.append(&[op]).unwrap();
}

fn edited_library() -> (MemStore, PhotoId) {
    let store = MemStore::new();
    let (mut j, mut c, _) = Journal::open(Box::new(store.clone())).unwrap();
    let mut p = photo();
    p.id = c.alloc_photo_id();
    let id = p.id;
    commit(&mut j, &mut c, Op::AddPhoto { photo: Box::new(p) });
    let mut m = c.photo(id).unwrap().meta.clone();
    m.set_exif(ExifField::Camera, Some("Canon EOS R5")).unwrap();
    m.set_exif(ExifField::Shutter, Some("2.5\"")).unwrap();
    m.set_exif(ExifField::Aperture, Some("f/4")).unwrap();
    commit(&mut j, &mut c, Op::SetMeta { id, meta: Box::new(m) });
    (store, id)
}

#[test]
fn overrides_persist_through_the_log_and_a_snapshot() {
    let (store, id) = edited_library();
    // reopened from the log alone
    let (mut j, c, _) = Journal::open(Box::new(store.clone())).unwrap();
    let m = &c.photo(id).unwrap().meta;
    assert_eq!((m.effective_camera(), m.effective_shutter(), m.effective_aperture()), ("Canon EOS R5", "2.5", Some(4.0)));
    assert_eq!((m.camera.as_str(), m.shutter.as_str(), m.aperture), ("SONY ILCE-7M5", "1/250", Some(2.8)), "the file's values are kept beside them");
    // and from a snapshot
    j.snapshot(&c).unwrap();
    drop(j);
    let (_, c, _) = Journal::open(Box::new(store)).unwrap();
    let m = &c.photo(id).unwrap().meta;
    assert_eq!((m.effective_camera(), m.effective_shutter()), ("Canon EOS R5", "2.5"));
    assert!(!m.exif_overridden(ExifField::Lens));
}

#[test]
fn undo_of_an_edit_gives_the_old_overrides_back() {
    let mut c = Catalog::new();
    let mut p = photo();
    p.id = c.alloc_photo_id();
    let id = p.id;
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let mut m = c.photo(id).unwrap().meta.clone();
    m.set_exif(ExifField::Iso, Some("1600")).unwrap();
    let inverse = c.apply(Op::SetMeta { id, meta: Box::new(m) }).unwrap();
    assert_eq!(c.photo(id).unwrap().meta.effective_iso(), Some(1600));
    c.apply(inverse).unwrap();
    assert_eq!(c.photo(id).unwrap().meta, meta());
}

#[test]
fn a_photo_without_overrides_writes_nothing_new() {
    let json = serde_json::to_string(&meta()).unwrap();
    assert!(!json.contains("exif"), "{json}");
    let mut m = meta();
    m.set_exif(ExifField::Lens, Some("Zeiss Otus 55mm f/1.4")).unwrap();
    assert!(serde_json::to_string(&m).unwrap().contains("\"exif\":{\"lens\":"));
}

/// A catalog written before the edit fields existed (format v10) opens unchanged and is upgraded.
#[test]
fn a_v10_library_opens_unchanged() {
    let mut c = Catalog::new();
    let mut p = photo();
    p.id = c.alloc_photo_id();
    let id = p.id;
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let snap = c.to_snapshot();
    assert!(!snap.contains("\"exif\""));
    let store = MemStore::new();
    store.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":10,\"seq\":1,\"catalog\":{snap}}}\n").into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(store.clone())).unwrap();
    assert_eq!(r.upgraded_from, Some(10));
    assert_eq!(loaded.photo(id).unwrap().meta, meta());
    assert_eq!(loaded.to_snapshot(), snap);
    let v: serde_json::Value = serde_json::from_slice(&store.get(SNAPSHOT).unwrap()).unwrap();
    assert_eq!(v["version"].as_u64(), Some(u64::from(VERSION)));
    let _ = LOG;
}

/// Smart albums, filters and search read the edited values.
#[test]
fn filters_and_rules_follow_the_edit() {
    let mut p = photo();
    p.meta.set_exif(ExifField::Camera, Some("Leica Q3")).unwrap();
    p.meta.set_exif(ExifField::Iso, Some("6400")).unwrap();
    let cat = Catalog::new();
    let f = |text: &str| Filter { text: text.into(), ..Default::default() };
    assert!(f("leica").matches(&p, &cat) && f("camera:leica").matches(&p, &cat) && f("iso:6400").matches(&p, &cat));
    assert!(!f("ilce").matches(&p, &cat) && !f("iso:400").matches(&p, &cat));
    assert!(Filter { camera: Some("leica q3".into()), ..Default::default() }.matches(&p, &cat));
    assert!(!Filter { camera: Some("SONY ILCE-7M5".into()), ..Default::default() }.matches(&p, &cat));
    let rule = |field: &str, op: &str, value: &str| Rule::Field { field: field.into(), op: op.into(), value: serde_json::json!(value) };
    assert!(rule("camera", "contains", "Leica").matches(&p, &cat));
    assert!(!rule("camera", "contains", "SONY").matches(&p, &cat));
}

/// A manual lens: the body writes F-Number 0, Focal Length 0 and Lens Model `----`. They are no values.
#[test]
fn placeholders_are_no_value() {
    let mut m = Meta { camera: "SONY ILCE-7M5".into(), lens: "----".into(), focal_mm: Some(0.0), aperture: Some(0.0), ..Default::default() };
    assert_eq!((m.effective_lens(), m.effective_focal_mm(), m.effective_aperture()), ("", None, None));
    assert_eq!((m.exif_text(ExifField::Lens, true), m.exif_text(ExifField::Aperture, false)), (String::new(), String::new()));
    let shown = m.shown();
    assert_eq!(shown.lens, "");
    // an edit over a placeholder is an edit; its reset gives "no value" back
    m.set_exif(ExifField::Lens, Some("Laowa 90mm f/2.8 2x Ultra Macro APO")).unwrap();
    m.set_exif(ExifField::Aperture, Some("2.8")).unwrap();
    assert_eq!((m.effective_lens(), m.effective_aperture()), ("Laowa 90mm f/2.8 2x Ultra Macro APO", Some(2.8)));
    m.reset_exif(ExifField::Lens);
    assert_eq!(m.effective_lens(), "");
    assert!(is_placeholder("--") && is_placeholder(" ") && !is_placeholder("FE 50mm"));
}
