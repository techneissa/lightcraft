//! AI mask parts found again on each photo (`fork/adaptive-masks.md`): presets, an old preset that carries a selection,
//! Paste to several photos in the background as one undo step, Update AI Masks. A finder stands in for SAM 3.

use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use serde_json::json;

use crate::Session;

/// Each photo's own subject: one cell of a 4 × 4 grid, a different one per photo.
fn subject_of(id: PhotoId) -> SegMask {
    let cell = (id.0 % 16) as usize;
    let logits: Vec<f32> = (0..16).map(|i| if i == cell { 5.0 } else { -5.0 }).collect();
    SegMask::from_logits(4, &logits)
}

/// A session with three photos (A active, all selected) and a finder that gives each its own subject.
fn session() -> (Session, [PhotoId; 3]) {
    let mut s = Session::with_demo();
    let mut ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).collect();
    ids.sort();
    let ids = [ids[0], ids[1], ids[2]];
    s.execute("library.select", &json!({"ids": [ids[0].0, ids[1].0, ids[2].0], "active": ids[0].0})).unwrap();
    s.adaptive.set_finder(Some(Arc::new(|id, shape| match shape {
        MaskShape::Subject { .. } | MaskShape::Background { .. } => Ok(Some(subject_of(id))),
        _ => Ok(None),
    })));
    (s, ids)
}

fn part(op: MaskOp, shape: MaskShape) -> MaskComponent {
    MaskComponent { name: None, op, invert: false, shape }
}

/// Photo `a`'s look: Exposure +1, a Subject mask (minus a brush stroke) and a Background mask, found on `a`.
fn give_look(s: &mut Session, a: PhotoId) {
    let mut d = (*s.develop_of(a).unwrap()).clone();
    d.light.exposure = 1.0;
    let subject = MaskShape::Subject { seg: Some(subject_of(a)), detail: vec![], edge: 0.0 };
    let brush = MaskShape::Brush { strokes: vec![] };
    let bg = MaskShape::Background { seg: Some(subject_of(a).invert().unwrap()), detail: vec![], edge: 0.0 };
    d.masks = vec![
        Mask {
            id: 1,
            name: "Subject Mask 1".into(),
            components: vec![part(MaskOp::Add, subject), part(MaskOp::Subtract, brush)],
            ..Default::default()
        },
        Mask { id: 2, name: "Background Mask 1".into(), components: vec![part(MaskOp::Add, bg)], ..Default::default() },
    ];
    d.masks[0].adjust.exposure = 0.5;
    s.set_develop(a, d, "Look").unwrap();
}

/// The selection of the part named `kind` on photo `id` (in the mask it is the first part of).
fn found(d: &DevelopSettings, first: fn(&MaskShape) -> bool) -> SegMask {
    let m = d.masks.iter().find(|m| first(&m.components[0].shape)).expect("the mask");
    m.components[0].shape.seg().cloned().expect("a selection")
}

fn is_subject(s: &MaskShape) -> bool {
    matches!(s, MaskShape::Subject { .. })
}

fn is_background(s: &MaskShape) -> bool {
    matches!(s, MaskShape::Background { .. })
}

/// The issue's test: a preset with a Subject mask applied to a second photo gets that photo's subject.
#[test]
fn a_preset_with_a_subject_mask_gets_each_photos_own_subject() {
    let (mut s, [a, b, _]) = session();
    give_look(&mut s, a);
    let pid = s.execute("preset.create", &json!({"name": "Portrait", "groups": ["light", "masks"]})).unwrap()["id"].as_str().unwrap().to_string();
    let saved = &s.presets.iter().find(|p| p.id == pid).unwrap().settings;
    assert!(saved["masks"][0]["components"][0]["shape"].get("seg").is_none(), "the preset keeps what it selects, not A's selection");
    s.execute("preset.apply", &json!({"id": pid, "ids": [b.0]})).unwrap();
    let d = s.develop_of(b).unwrap();
    assert_eq!(found(&d, is_subject), subject_of(b), "B's own subject");
    assert_ne!(found(&d, is_subject), subject_of(a));
    assert_eq!(found(&d, is_background), subject_of(b).invert().unwrap(), "and its own background");
    assert!(d.masks.iter().all(Mask::renders));
    // the brush part and the edits came as they were
    let m = d.masks.iter().find(|m| is_subject(&m.components[0].shape)).unwrap();
    assert_eq!(m.components[1], part(MaskOp::Subtract, MaskShape::Brush { strokes: vec![] }));
    assert_eq!((d.light.exposure, m.adjust.exposure), (1.0, 0.5));
}

/// A preset saved before this carries the first photo's selection: applied to another photo, it is found there.
#[test]
fn an_old_preset_that_carries_a_selection_is_found_on_the_photo() {
    let (mut s, [a, b, _]) = session();
    give_look(&mut s, a);
    let d = s.develop_of(a).unwrap();
    let old = lightcraft_develop::Preset {
        id: "user.old".into(),
        name: "Old".into(),
        group: "User Presets".into(),
        settings: json!({"masks": serde_json::to_value(&d.masks).unwrap()}),
        favorite: false,
        builtin: false,
    };
    assert!(old.settings["masks"][0]["components"][0]["shape"]["seg"].is_object());
    s.presets.push(old);
    s.execute("preset.apply", &json!({"id": "user.old", "ids": [b.0]})).unwrap();
    assert_eq!(found(&s.develop_of(b).unwrap(), is_subject), subject_of(b));
}

/// Paste to two photos in the app: the edit lands at once with the masks waiting (drawing nothing), they are found
/// in the background under one row in the ring, and the whole paste is one undo step whose redo brings back what
/// was found.
#[test]
fn a_paste_to_several_photos_is_found_in_the_background_as_one_undo_step() {
    let (mut s, [a, b, c]) = session();
    s.segmenter.background = true;
    give_look(&mut s, a);
    let before = (s.develop_of(b).unwrap(), s.develop_of(c).unwrap());
    s.execute("develop.copy", &json!({"groups": ["light", "masks"]})).unwrap();
    s.execute("develop.paste", &json!({"ids": [b.0, c.0]})).unwrap();
    for id in [b, c] {
        let d = s.develop_of(id).unwrap();
        assert_eq!(d.light.exposure, 1.0, "the edit lands at once");
        assert!(d.masks.iter().all(|m| m.is_waiting() && !m.renders()), "never A's selection meanwhile");
    }
    let row = s.activity.list().into_iter().find(|t| t.kind == "adaptiveMasks").expect("a row");
    assert!(row.background && row.batch && row.total == 2, "{row:?}");
    for _ in 0..10 {
        s.segment_poll();
    }
    assert!(!s.adaptive_busy() && s.activity.list().iter().all(|t| t.kind != "adaptiveMasks"), "done, the row gone");
    for id in [b, c] {
        assert_eq!(found(&s.develop_of(id).unwrap(), is_subject), subject_of(id), "{id:?} has its own subject");
    }
    let found_b = s.develop_of(b).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((s.develop_of(b).unwrap(), s.develop_of(c).unwrap()), before, "one step undoes the paste on both");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.develop_of(b).unwrap(), found_b, "redo brings what was found, not waiting parts");
    assert!(!s.adaptive_busy());
}

/// Update AI Masks finds a photo's own AI masks again (after an edit moved the subject), one undo step.
#[test]
fn update_ai_masks_finds_a_photos_own_masks_again() {
    let (mut s, [a, _, _]) = session();
    give_look(&mut s, a);
    // the subject moved: the finder now finds it one cell on
    s.adaptive.set_finder(Some(Arc::new(|id, _| Ok(Some(subject_of(PhotoId(id.0 + 1)))))));
    let before = s.develop_of(a).unwrap();
    let out = s.execute("mask.updateAi", &json!({"ids": [a.0]})).unwrap();
    assert_eq!(out, json!({"photos": 1, "parts": 2}));
    assert_eq!(found(&s.develop_of(a).unwrap(), is_subject), subject_of(PhotoId(a.0 + 1)));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.develop_of(a).unwrap(), before);
    // nothing to find on a photo without AI masks
    let none = s.execute("mask.updateAi", &json!({"ids": [s.selection.ids[1].0]}));
    assert!(none.is_err());
}

/// What a model finds nothing for selects nothing, and a photo that has the same part found keeps its own.
#[test]
fn nothing_found_selects_nothing_and_a_photos_own_part_is_kept() {
    let (mut s, [a, b, _]) = session();
    give_look(&mut s, a);
    s.execute("develop.copy", &json!({"groups": ["masks"]})).unwrap();
    s.execute("develop.paste", &json!({"ids": [b.0]})).unwrap();
    let on_b = s.develop_of(b).unwrap();
    // pasted again with a finder that finds nothing: B's own parts stay
    s.adaptive.set_finder(Some(Arc::new(|_, _| Ok(None))));
    s.execute("develop.paste", &json!({"ids": [b.0]})).unwrap();
    assert_eq!(s.develop_of(b).unwrap().masks, on_b.masks);
    // on a new photo nothing is found: the parts select nothing, never A's selection
    let c = s.selection.ids[2];
    s.execute("develop.paste", &json!({"ids": [c.0]})).unwrap();
    let d = s.develop_of(c).unwrap();
    let seg = found(&d, is_subject);
    assert!(!seg.is_waiting() && seg.logits().unwrap().iter().all(|l| *l < 0.0));
}

/// A small PNG for a library on disk.
fn png(dir: &std::path::Path, name: &str) -> String {
    let (w, h) = (64, 48);
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

/// Library `name` in `dir` with one photo whose Subject mask waits (set without a command, so nothing finds it).
fn library_with_a_waiting_subject(s: &mut Session, dir: &std::path::Path, name: &str) -> PhotoId {
    s.open_library(dir.join(name), false).unwrap();
    s.execute("library.import", &json!({"paths": [png(dir, &format!("{name}.png"))]})).unwrap();
    let id = s.catalog.photos().map(|p| p.id).min().unwrap();
    let mut d = (*s.develop_of(id).unwrap()).clone();
    let mut subject = MaskShape::subject();
    subject.set_waiting();
    d.masks = vec![Mask { id: 1, name: "Subject Mask 1".into(), components: vec![part(MaskOp::Add, subject)], ..Default::default() }];
    s.set_develop(id, d, "Look").unwrap();
    id
}

/// Opening another library stops the masks being found in the one left: photo ids are per library, so an answer for
/// the old photo must never land on the new library's photo with the same id, mask and part.
#[test]
fn opening_another_library_stops_finding_the_old_ones_masks() {
    let dir = std::env::temp_dir().join(format!("lc-adaptive-libraries-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new().with_fs();
    s.segmenter.background = true;
    s.adaptive.set_finder(Some(Arc::new(|id, _| Ok(Some(subject_of(id))))));
    let old = library_with_a_waiting_subject(&mut s, &dir, "first");
    assert_eq!(s.adaptive_find(&[old]), 1);
    s.segment_poll(); // the old photo's search runs (its answer is in)
    assert!(s.adaptive_busy());
    let new = library_with_a_waiting_subject(&mut s, &dir, "second");
    assert_eq!(new, old, "the same id in the new library");
    assert!(!s.adaptive_busy(), "stopped");
    for _ in 0..5 {
        s.segment_poll();
    }
    assert!(s.develop_of(new).unwrap().masks[0].is_waiting(), "nothing of the old library's search landed here");
    let _ = std::fs::remove_dir_all(dir);
}

/// A part that changes while its photo's masks are being found (another paste, a new description) is found as it is
/// now, once the running search ends.
#[test]
fn a_part_changed_while_its_photo_is_being_found_is_found_as_it_is_now() {
    let (mut s, [a, b, _]) = session();
    s.segmenter.background = true;
    let describe = |id: PhotoId| subject_of(PhotoId(id.0 + 7));
    s.adaptive.set_finder(Some(Arc::new(move |id, shape| match shape {
        MaskShape::Prompt { .. } => Ok(Some(describe(id))),
        _ => Ok(Some(subject_of(id))),
    })));
    give_look(&mut s, a);
    // one AI mask only: nothing else of B's is put in when its search ends, so nothing else starts it again
    let mut d = (*s.develop_of(a).unwrap()).clone();
    d.masks.truncate(1);
    s.set_develop(a, d, "One mask").unwrap();
    s.execute("develop.copy", &json!({"groups": ["masks"]})).unwrap();
    s.execute("develop.paste", &json!({"ids": [b.0]})).unwrap();
    s.segment_poll(); // B's search runs
    assert!(s.adaptive_busy());
    // A's first mask now selects what a description names; pasted to B while B's search runs
    let mut d = (*s.develop_of(a).unwrap()).clone();
    d.masks[0].components[0].shape = MaskShape::Prompt { text: "dog".into(), seg: Some(describe(a)), detail: vec![], edge: 0.0, region: None };
    s.set_develop(a, d, "Describe").unwrap();
    s.execute("develop.copy", &json!({"groups": ["masks"]})).unwrap();
    s.execute("develop.paste", &json!({"ids": [b.0]})).unwrap();
    for _ in 0..10 {
        s.segment_poll();
    }
    assert!(!s.adaptive_busy());
    let on_b = s.develop_of(b).unwrap();
    assert!(on_b.masks.iter().all(|m| !m.is_waiting()), "nothing left waiting");
    let MaskShape::Prompt { seg, .. } = &on_b.masks[0].components[0].shape else { panic!("{:?}", on_b.masks[0]) };
    assert_eq!(seg.as_ref(), Some(&describe(b)), "B's own find of the description");
}

/// Update AI Masks that finds nothing (the model fails) keeps the selections the photo had; the ring names the photo.
#[test]
fn a_failed_update_keeps_the_selection_the_photo_had() {
    let (mut s, [a, _, _]) = session();
    s.segmenter.background = true;
    give_look(&mut s, a);
    let before = s.develop_of(a).unwrap();
    s.adaptive.set_finder(Some(Arc::new(|_, _| Err("the model failed".into()))));
    s.execute("mask.updateAi", &json!({"ids": [a.0]})).unwrap();
    for _ in 0..10 {
        s.segment_poll();
    }
    assert!(!s.adaptive_busy());
    assert_eq!(s.develop_of(a).unwrap().masks, before.masks, "the selections it had");
    let summary = s.activity.finished().into_iter().rev().find(|b| b.kind == "adaptiveMasks").expect("a summary");
    assert_eq!(summary.failed, 1);
    assert!(summary.reasons[0].contains("the model failed"), "{:?}", summary.reasons);
}

/// Update Preset saves what each AI part selects, never the photo's selection.
#[test]
fn updating_a_preset_keeps_no_selection() {
    let (mut s, [a, _, _]) = session();
    s.execute("preset.create", &json!({"name": "Look", "groups": ["masks"]})).unwrap();
    let pid = s.presets.last().unwrap().id.clone();
    give_look(&mut s, a);
    s.execute("preset.update", &json!({"id": pid})).unwrap();
    let saved = &s.presets.iter().find(|p| p.id == pid).unwrap().settings;
    assert!(saved["masks"][0]["components"][0]["shape"].is_object(), "{saved}");
    assert!(saved["masks"][0]["components"][0]["shape"].get("seg").is_none(), "{saved}");
}
