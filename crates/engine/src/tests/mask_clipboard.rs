//! Copy Masks and Paste Masks (fork, issue #365).

use std::sync::Arc;

use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use serde_json::json;

use crate::Session;

/// A selection that differs from photo to photo.
fn subject_of(id: u64) -> SegMask {
    let logits: Vec<f32> = (0..16).map(|i| if i == id % 16 { 16.0 } else { -16.0 }).collect();
    SegMask::from_logits(4, &logits)
}

fn mask(id: u32, name: &str, shape: MaskShape) -> Mask {
    let mut m =
        Mask { id, name: name.into(), components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], ..Mask::default() };
    m.adjust.exposure = f64::from(id) / 10.0;
    m
}

fn set_masks(s: &mut Session, photo: u64, masks: Vec<Mask>) {
    let id = lightcraft_catalog::PhotoId(photo);
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.masks = masks;
    s.set_develop(id, d, "Setup").unwrap();
}

fn masks_of(s: &Session, photo: u64) -> Vec<Mask> {
    s.develop_of(lightcraft_catalog::PhotoId(photo)).unwrap().masks.clone()
}

#[test]
fn paste_masks_replaces_a_photos_masks_with_the_copied_ones() {
    let mut s = Session::with_demo();
    s.adaptive.set_finder(Some(Arc::new(|id, _| Ok(Some(subject_of(id.0))))));
    let ids: Vec<u64> = s.catalog.photos().take(2).map(|p| p.id.0).collect();
    let (a, b) = (ids[0], ids[1]);
    s.execute("library.select", &json!({"ids": [a], "active": a})).unwrap();
    let brush = mask(1, "Painted", MaskShape::Brush { strokes: vec![] });
    let subject = mask(2, "Person", MaskShape::Subject { seg: Some(subject_of(a)), detail: vec![], edge: 0.0 });
    set_masks(&mut s, a, vec![brush.clone(), subject]);
    set_masks(&mut s, b, vec![mask(3, "Own", MaskShape::Brush { strokes: vec![] })]);
    assert!(s.execute("mask.pasteAll", &json!({})).is_err(), "nothing copied yet");

    s.execute("develop.copy", &json!({})).unwrap();
    let settings = s.clipboard.clone();
    s.execute("mask.copyAll", &json!({})).unwrap();
    assert_eq!(s.clipboard, settings, "Copy Masks leaves what Copy Settings copied");

    s.execute("library.select", &json!({"ids": [b], "active": b})).unwrap();
    s.execute("mask.pasteAll", &json!({})).unwrap();
    let got = masks_of(&s, b);
    assert_eq!(got.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Painted", "Person"]);
    assert_eq!((&got[0].components, got[0].adjust), (&brush.components, brush.adjust));
    assert_eq!(got[1].components[0].shape.seg(), Some(&subject_of(b)), "B's own subject, not A's");
    // one undo step
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(masks_of(&s, b).iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Own"]);
}

/// Issue #399: a photo's rough Subject, or its Background that found nothing, is not a selection of its own to keep:
/// a paste looks for the part on that photo.
#[test]
fn a_paste_finds_the_parts_the_photo_never_found() {
    let mut s = Session::with_demo();
    s.adaptive.set_finder(Some(Arc::new(|id, _| Ok(Some(subject_of(id.0))))));
    let ids: Vec<u64> = s.catalog.photos().take(2).map(|p| p.id.0).collect();
    let (a, b) = (ids[0], ids[1]);
    let subject = |seg| MaskShape::Subject { seg, detail: vec![], edge: 0.0 };
    let background = |seg| MaskShape::Background { seg, detail: vec![], edge: 0.0 };
    set_masks(&mut s, b, vec![mask(1, "Rough", subject(None)), mask(2, "Empty", background(Some(SegMask::nothing())))]);
    set_masks(&mut s, a, vec![mask(1, "Person", subject(Some(subject_of(a)))), mask(2, "Behind", background(subject_of(a).invert()))]);
    s.execute("library.select", &json!({"ids": [a], "active": a})).unwrap();
    s.execute("mask.copyAll", &json!({})).unwrap();
    s.execute("library.select", &json!({"ids": [b], "active": b})).unwrap();
    s.execute("mask.pasteAll", &json!({})).unwrap();
    let got = masks_of(&s, b);
    assert_eq!(got[0].components[0].shape.seg(), Some(&subject_of(b)), "B's subject, not the rough one");
    assert_eq!(got[1].components[0].shape.seg(), subject_of(b).invert().as_ref(), "B's background, not nothing");
}
