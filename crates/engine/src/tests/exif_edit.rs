//! Edited camera / lens / exposure values: `photo.setMeta` keys and `photo.resetExif` store them beside the
//! file's values (one undo step, all targets, a bad value changes nothing), search, filters, rename tokens and
//! export use them, and anything that renders keeps reading the file's camera.

use lightcraft_catalog::{ExifField, MediaKind, PhotoId};
use serde_json::json;

use crate::Session;

/// The demo library with photos 0..3 given known file values.
fn library() -> (Session, Vec<PhotoId>) {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.visible_cloned().into_iter().take(3).collect();
    for (i, id) in ids.iter().enumerate() {
        let mut m = s.catalog.photo(*id).unwrap().meta.clone();
        m.camera = format!("SONY ILCE-7M{}", i + 3);
        m.lens = "FE 24-70mm F2.8 GM".into();
        m.focal_mm = Some(24.0 + i as f32);
        m.aperture = Some(2.8);
        m.shutter = "1/250".into();
        m.iso = Some(100 * (i as u32 + 1));
        s.apply_system(lightcraft_catalog::Op::SetMeta { id: *id, meta: Box::new(m) }).unwrap();
    }
    (s, ids)
}

fn meta(s: &Session, id: PhotoId) -> lightcraft_catalog::Meta {
    s.catalog.photo(id).unwrap().meta.clone()
}

#[test]
fn set_meta_edits_every_target_in_one_undo_step() {
    let (mut s, ids) = library();
    let before: Vec<_> = ids.iter().map(|id| meta(&s, *id)).collect();
    let undo_depth = s.undo.len();
    let ids_json: Vec<u64> = ids.iter().map(|i| i.0).collect();
    s.execute(
        "photo.setMeta",
        &json!({"ids": ids_json, "camera": "Canon EOS R5", "lens": "RF24-70mm F2.8", "focal": "35 mm", "aperture": "f/4", "shutter": "2.5\"", "iso": 1600}),
    )
    .unwrap();
    assert_eq!(s.undo.len(), undo_depth + 1, "one undo step");
    for (id, old) in ids.iter().zip(&before) {
        let m = meta(&s, *id);
        assert_eq!(
            (m.effective_camera(), m.effective_lens(), m.effective_focal_mm(), m.effective_aperture(), m.effective_shutter(), m.effective_iso()),
            ("Canon EOS R5", "RF24-70mm F2.8", Some(35.0), Some(4.0), "2.5", Some(1600))
        );
        assert_eq!(
            (&m.camera, &m.lens, m.focal_mm, m.aperture, &m.shutter, m.iso),
            (&old.camera, &old.lens, old.focal_mm, old.aperture, &old.shutter, old.iso)
        );
    }
    s.undo_step().unwrap();
    for (id, old) in ids.iter().zip(&before) {
        assert_eq!(&meta(&s, *id), old, "undo gives the file values back with no override");
    }
}

#[test]
fn null_and_empty_reset_a_field_and_a_bad_value_changes_nothing() {
    let (mut s, ids) = library();
    let id = ids[0];
    s.execute("photo.setMeta", &json!({"ids": [id.0], "iso": 800, "lens": "Mine"})).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [id.0], "iso": null})).unwrap();
    let m = meta(&s, id);
    assert_eq!((m.effective_iso(), m.effective_lens()), (Some(100), "Mine"));
    s.execute("photo.setMeta", &json!({"ids": [id.0], "lens": ""})).unwrap();
    assert!(meta(&s, id).exif.is_empty());
    // a bad value in one field refuses the call: nothing of it, and no other photo, changes
    s.execute("photo.setMeta", &json!({"ids": [id.0], "camera": "Keep"})).unwrap();
    let (snapshot, depth) = (s.catalog.to_snapshot(), s.undo.len());
    for bad in [
        json!({"ids": [ids[0].0, ids[1].0], "title": "T", "iso": "abc"}),
        json!({"ids": [ids[0].0, ids[1].0], "shutter": "1/0"}),
        json!({"ids": [ids[0].0], "focal": -3}),
        json!({"ids": [ids[0].0], "aperture": "f/0"}),
        json!({"ids": [ids[0].0], "iso": 1.5}),
        json!({"ids": [ids[0].0], "iso": true}),
        json!({"ids": [ids[0].0], "camera": 12}),
        json!({"ids": [ids[0].0], "lens": ["x"]}),
        json!({"ids": [ids[0].0], "camera": "x".repeat(100_000)}),
    ] {
        assert!(s.execute("photo.setMeta", &bad).is_err(), "{bad:.80}");
        assert_eq!(s.catalog.to_snapshot(), snapshot, "{bad:.80}");
        assert_eq!(s.undo.len(), depth);
    }
}

#[test]
fn reset_exif_resets_a_field_or_all_and_leaves_the_file_values() {
    let (mut s, ids) = library();
    let file: Vec<_> = ids.iter().map(|id| meta(&s, *id)).collect();
    let all: Vec<u64> = ids.iter().map(|i| i.0).collect();
    s.execute("photo.setMeta", &json!({"ids": all, "camera": "X", "lens": "Y", "iso": 3200, "shutter": "1/30"})).unwrap();
    let r = s.execute("photo.resetExif", &json!({"ids": all, "fields": ["lens", "ISO"]})).unwrap();
    assert_eq!(r["changed"], 3);
    let m = meta(&s, ids[1]);
    assert_eq!((m.effective_camera(), m.effective_lens(), m.effective_iso(), m.effective_shutter()), ("X", "FE 24-70mm F2.8 GM", Some(200), "1/30"));
    assert!(m.exif_overridden(ExifField::Camera) && !m.exif_overridden(ExifField::Iso));
    // only the photos with something to reset count; an unknown field is refused
    let r = s.execute("photo.resetExif", &json!({"ids": [ids[0].0]})).unwrap();
    assert_eq!(r["changed"], 1);
    assert_eq!(s.execute("photo.resetExif", &json!({"ids": [ids[0].0]})).unwrap()["changed"], 0);
    assert!(s.execute("photo.resetExif", &json!({"fields": ["nope"]})).is_err());
    assert!(s.execute("photo.resetExif", &json!({"fields": "iso"})).is_err());
    s.execute("photo.resetExif", &json!({"ids": all})).unwrap();
    for (id, f) in ids.iter().zip(&file) {
        assert_eq!(&meta(&s, *id), f, "the file's values were never touched");
    }
}

#[test]
fn search_filters_and_rename_tokens_use_the_edited_values() {
    let (mut s, ids) = library();
    s.execute("photo.setMeta", &json!({"ids": [ids[0].0], "camera": "Leica Q3", "iso": 6400})).unwrap();
    let p = s.catalog.photo(ids[0]).unwrap().clone();
    assert_eq!(crate::rename::expand_tokens("{camera}_{iso}", &p, 1, 1), "Leica Q3_6400");
    let q = s.catalog.photo(ids[1]).unwrap().clone();
    assert_eq!(crate::rename::expand_tokens("{camera}_{iso}", &q, 1, 1), "SONY ILCE-7M4_200", "an unedited photo reads its file");
    s.execute("library.filter", &json!({"camera": "Leica Q3"})).unwrap();
    assert_eq!(s.visible_cloned(), vec![ids[0]]);
    s.execute("library.filter", &json!({"camera": null})).unwrap();
    s.execute("library.filter", &json!({"text": "iso:6400"})).unwrap();
    assert_eq!(s.visible_cloned(), vec![ids[0]]);
    s.execute("library.filter", &json!({"text": "ILCE-7M3"})).unwrap();
    assert!(s.visible_cloned().is_empty(), "the file's camera no longer matches the edited photo");
    let listed = s.execute("catalog.query", &json!({"filter": {}})).unwrap();
    let summary = listed["photos"].as_array().and_then(|a| a.iter().find(|v| v["id"] == ids[0].0)).unwrap().clone();
    assert_eq!(
        (summary["camera"].as_str(), summary["fileCamera"].as_str(), summary["exifEdited"].as_bool()),
        (Some("Leica Q3"), Some("SONY ILCE-7M3"), Some(true))
    );
}

#[test]
fn export_writes_the_edited_values_and_honours_the_policy() {
    use crate::export::{ExportOptions, MetadataPolicy, export_metadata};
    let (mut s, ids) = library();
    s.execute("photo.setMeta", &json!({"ids": [ids[0].0], "camera": "Canon EOS R5", "lens": "RF50mm F1.2 L USM", "focal": 50, "aperture": "f/1.2", "shutter": "1/8000", "iso": 64})).unwrap();
    let p = s.catalog.photo(ids[0]).unwrap();
    let m = export_metadata(p, &s.catalog, &ExportOptions::default()).unwrap();
    assert_eq!(
        (m.model.as_deref(), m.lens_model.as_deref(), m.focal_length, m.f_number, m.iso),
        (Some("Canon EOS R5"), Some("RF50mm F1.2 L USM"), Some(50.0), Some(1.2f32 as f64), Some(64))
    );
    assert!((m.exposure_time.unwrap() - 1.0 / 8000.0).abs() < 1e-9);
    // read back from the encoded file
    let img = lightcraft_raster::Rgba8::from_fn(8, 8, |x, y| [x as u8, y as u8, 0, 255]);
    let jpg = crate::export::encode_with_metadata(&img, &ExportOptions::default(), Some(&m)).unwrap();
    let back = lightcraft_meta::extract(&jpg);
    assert_eq!((back.model.as_deref(), back.lens_model.as_deref(), back.iso), (Some("Canon EOS R5"), Some("RF50mm F1.2 L USM"), Some(64)));
    // "all except camera" and "copyright only" leave them out, as they do the file's
    let o = ExportOptions { metadata: MetadataPolicy::AllExceptCamera, ..Default::default() };
    let m = export_metadata(p, &s.catalog, &o).unwrap();
    assert!(m.model.is_none() && m.lens_model.is_none() && m.iso.is_none());
    // an unedited photo still exports what its file said
    let q = s.catalog.photo(ids[1]).unwrap();
    let m = export_metadata(q, &s.catalog, &ExportOptions::default()).unwrap();
    assert_eq!((m.model.as_deref(), m.iso), (Some("SONY ILCE-7M4"), Some(200)));
}

#[test]
fn what_renders_keeps_reading_the_files_camera() {
    let (mut s, ids) = library();
    let id = ids[0];
    // a raw whose file camera is edited to one the colour tables know otherwise
    let mut ph = (**s.catalog.photo(id).unwrap()).clone();
    ph.kind = MediaKind::Raw;
    ph.format = "ARW".into();
    ph.meta.camera = "SONY ILCE-1".into();
    ph.as_shot_wb = Some((5200.0, 4.0));
    s.apply_system(lightcraft_catalog::Op::SetMeta { id, meta: Box::new(ph.meta.clone()) }).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [id.0], "camera": "SONY ILCE-7M3"})).unwrap();
    let edited = s.catalog.photo(id).unwrap();
    assert_eq!(edited.meta.effective_camera(), "SONY ILCE-7M3");
    assert_eq!(edited.meta.camera, "SONY ILCE-1", "the file's camera is what the renderer reads");
    let mut raw = edited.as_ref().clone();
    raw.kind = MediaKind::Raw;
    raw.format = "ARW".into();
    raw.preview_only = None;
    // ILCE-1 edits white balance relative to the as-shot look (no measured matrices); ILCE-7M3 would use Kelvin
    assert!(raw.relative_wb(), "relative white balance follows the file's camera, not the edit");
    // (the same photo with ILCE-7M3 as its file camera does edit in Kelvin, so the check above tells them apart)
    raw.meta.camera = "SONY ILCE-7M3".into();
    assert!(!raw.relative_wb());
}

#[test]
fn reload_keeps_the_edits_and_still_fills_the_files_missing_fields() {
    let (mut s, ids) = library();
    let id = ids[0];
    s.execute("photo.setMeta", &json!({"ids": [id.0], "camera": "Canon EOS R5", "iso": 1600})).unwrap();
    let mut ph = (**s.catalog.photo(id).unwrap()).clone();
    ph.meta.lens = String::new();
    ph.meta.iso = None;
    s.apply_system(lightcraft_catalog::Op::SetMeta { id, meta: Box::new(ph.meta.clone()) }).unwrap();
    let mut info = crate::media::ProbeInfo::default();
    info.meta.lens = "Probed lens".into();
    info.meta.iso = Some(400);
    info.meta.camera = "SONY ILCE-7M3".into();
    let (mut ops, mut changed) = (Vec::new(), Vec::new());
    crate::cmd::convert::reload_ops(&s.catalog, id, &info, &mut ops, &mut changed);
    for op in ops {
        s.apply_system(op).unwrap();
    }
    let m = meta(&s, id);
    assert_eq!((m.lens.as_str(), m.iso), ("Probed lens", Some(400)), "the file's missing values are read");
    assert_eq!((m.effective_camera(), m.effective_iso()), ("Canon EOS R5", Some(1600)), "the edits stay");
}

#[test]
fn reload_replaces_placeholders_but_not_edits() {
    let (mut s, ids) = library();
    let id = ids[0];
    let mut m = meta(&s, id);
    m.lens = "----".into();
    m.focal_mm = Some(0.0);
    m.aperture = Some(0.0);
    s.apply_system(lightcraft_catalog::Op::SetMeta { id, meta: Box::new(m) }).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [id.0], "lens": "Laowa 90mm"})).unwrap();
    let mut info = crate::media::ProbeInfo::default();
    info.meta.lens = "Real lens".into();
    info.meta.focal_mm = Some(90.0);
    info.meta.aperture = Some(2.8);
    let (mut ops, mut changed) = (Vec::new(), Vec::new());
    crate::cmd::convert::reload_ops(&s.catalog, id, &info, &mut ops, &mut changed);
    for op in ops {
        s.apply_system(op).unwrap();
    }
    let m = meta(&s, id);
    assert_eq!((m.lens.as_str(), m.focal_mm, m.aperture), ("Real lens", Some(90.0), Some(2.8)), "the file's placeholders are replaced");
    assert_eq!(m.effective_lens(), "Laowa 90mm", "the edit stays");
}
