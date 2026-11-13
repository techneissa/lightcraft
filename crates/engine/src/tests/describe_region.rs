//! Issue #281: a Describe mask limited to the zoomed-in area. The `mask.*` commands take the area (`region`), and the
//! mask keeps it in the catalog and in the sidecar. No test needs the SAM 3 model: the selection is passed in (`seg`).

use lightcraft_develop::{MaskShape, SegMask};
use serde_json::{Value, json};

use crate::Session;

fn region_of(s: &Session, mask: usize, comp: usize) -> Option<[f64; 4]> {
    let d = s.develop_of(s.active().unwrap()).unwrap();
    let MaskShape::Prompt { region, .. } = &d.masks[mask].components[comp].shape else { panic!("a Describe: {:?}", d.masks[mask]) };
    *region
}

fn found() -> Value {
    serde_json::to_value(SegMask::from_logits(4, &[5.0; 16])).unwrap()
}

#[test]
fn describe_commands_take_a_region() {
    let mut s = Session::with_demo();
    // corners in any order, kept in order
    s.execute("mask.add", &json!({"kind": "prompt", "text": "eye", "seg": found(), "region": [0.3, 0.4, 0.1, 0.2]})).unwrap();
    assert_eq!(region_of(&s, 0, 0), Some([0.1, 0.2, 0.3, 0.4]));
    // the whole photo, or none given: no region
    s.execute("mask.addComponent", &json!({"kind": "prompt", "text": "eye", "seg": found(), "region": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    s.execute("mask.addComponent", &json!({"kind": "prompt", "text": "eye", "seg": found()})).unwrap();
    assert_eq!((region_of(&s, 0, 1), region_of(&s, 0, 2)), (None, None));
    // not four numbers: an error, nothing added
    for bad in [json!([0.1, 0.2]), json!("left"), json!([0.1, 0.2, "x", 0.4])] {
        let e = s.execute("mask.add", &json!({"kind": "prompt", "text": "eye", "seg": found(), "region": bad})).unwrap_err().to_string();
        assert!(e.contains("region"), "{e}");
    }
    assert_eq!(s.develop_of(s.active().unwrap()).unwrap().masks.len(), 1);
}

#[test]
fn a_describe_region_round_trips_through_the_sidecar() {
    use crate::sidecar::{DevelopPatch, parse_sidecar, sidecar_packet};
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "prompt", "text": "eye", "seg": found(), "region": [0.1, 0.2, 0.3, 0.4]})).unwrap();
    let p = s.catalog.photo(s.active().unwrap()).unwrap().clone();
    let back = parse_sidecar(&sidecar_packet(&p, &s.catalog), crate::crs::Target::Rendered).unwrap();
    let Some(DevelopPatch::Full(d)) = &back.develop else { panic!("no lc:settings") };
    assert_eq!(d.masks[0].components[0].shape.region(), Some([0.1, 0.2, 0.3, 0.4]));
    assert_eq!(d.masks, p.develop.masks);
}
