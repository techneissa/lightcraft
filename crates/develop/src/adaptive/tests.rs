use serde_json::json;

use super::*;
use crate::{MaskComponent, MaskOp, Preset, SettingsGroup};

fn found() -> SegMask {
    SegMask::from_logits(2, &[5.0; 4])
}

fn part(shape: MaskShape) -> MaskComponent {
    MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }
}

/// A Subject mask found on its photo, with a brush part subtracted.
fn with_subject() -> DevelopSettings {
    let subject = MaskShape::Subject { seg: Some(found()), detail: vec![found()], edge: 20.0 };
    let brush = MaskComponent { op: MaskOp::Subtract, ..part(MaskShape::Brush { strokes: vec![] }) };
    let mask = Mask { id: 1, name: "Subject Mask 1".into(), components: vec![part(subject), brush], ..Default::default() };
    DevelopSettings { masks: vec![mask], ..Default::default() }
}

#[test]
fn a_preset_carries_what_an_ai_part_selects_and_not_the_selection() {
    let p = Preset::from_settings("p", "P", "G", &with_subject(), &[SettingsGroup::Masks]);
    let shape: MaskShape = serde_json::from_value(p.settings["masks"][0]["components"][0]["shape"].clone()).unwrap();
    assert_eq!(shape, MaskShape::Subject { seg: None, detail: vec![], edge: 20.0 }, "the recipe and its Edge only");
    // applied, the part waits to be found on the photo, and the brush part is as it was
    let out = p.apply(&DevelopSettings::default(), 1.0);
    let m = &out.masks[0];
    assert!(m.components[0].shape.is_waiting() && !m.renders());
    assert_eq!(m.components[1], with_subject().masks[0].components[1]);
}

/// A preset saved before carries the first photo's selection: applied, it waits to be found again.
#[test]
fn an_old_preset_with_a_selection_waits_to_be_found_again() {
    let old = Preset {
        id: "o".into(),
        name: "O".into(),
        group: "G".into(),
        settings: json!({"masks": [with_subject().masks[0]]}),
        favorite: false,
        builtin: false,
    };
    assert!(old.settings["masks"][0]["components"][0]["shape"]["seg"].is_object(), "it carries a selection");
    let out = old.apply(&DevelopSettings::default(), 1.0);
    let waiting = out.waiting();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].recipe, MaskShape::subject());
    // applied again it isn't added twice, found or not
    let mut found_now = out.clone();
    found_now.masks[0].components[0].shape.set_found(Some(found()), vec![]);
    assert_eq!(old.apply(&found_now, 1.0).masks.len(), 1);
}

#[test]
fn a_photo_keeps_its_own_selection_of_the_same_part() {
    let own = with_subject();
    let mut incoming = with_subject();
    let MaskShape::Subject { seg, .. } = &mut incoming.masks[0].components[0].shape else { panic!() };
    *seg = Some(SegMask::from_logits(2, &[-3.0; 4]));
    assert_eq!(adapt(Some(&own), &mut incoming), 0);
    assert_eq!(incoming.masks[0].components[0].shape, own.masks[0].components[0].shape);
    // another photo, or this one turned, finds its own
    assert_eq!(adapt(None, &mut incoming.clone()), 1);
    let mut turned = with_subject();
    turned.orientation = lightcraft_geom::Orientation::Rotate90;
    assert_eq!(adapt(Some(&own), &mut turned), 1);
    // a different description is a different part
    let mut other = with_subject();
    other.masks[0].components[0].shape = MaskShape::Prompt { text: "dog".into(), seg: Some(found()), detail: vec![], edge: 0.0, region: None };
    assert_eq!(adapt(Some(&own), &mut other), 1);
}

/// A waiting part reads back as one, and a build that doesn't know the mark sees a selection of nothing.
#[test]
fn a_waiting_part_survives_a_save_and_selects_nothing() {
    let mut d = with_subject();
    d.masks[0].components[0].shape.set_waiting();
    let back: DevelopSettings = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
    assert!(back.masks[0].is_waiting());
    let logits = SegMask::waiting().logits().unwrap();
    assert!(logits.iter().all(|l| *l < -15.0));
}
