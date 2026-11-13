//! Issue #281: a Describe mask limited to a part of the photo keeps that part through its settings.

use serde_json::json;

use super::*;

fn with_prompt(region: Option<[f64; 4]>) -> DevelopSettings {
    let mut d = DevelopSettings::default();
    d.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Prompt { text: "eye".into(), seg: None, detail: Vec::new(), edge: 0.0, region },
        }],
        ..Default::default()
    });
    d
}

#[test]
fn a_describe_region_round_trips_through_the_settings() {
    let d = with_prompt(Some([0.25, 0.1, 0.5, 0.4]));
    let v = d.to_json();
    assert_eq!(v["masks"][0]["components"][0]["shape"]["region"], json!([0.25, 0.1, 0.5, 0.4]));
    assert_eq!(DevelopSettings::from_json(&v).unwrap(), d);
    assert_eq!(DevelopSettings::from_untrusted_json(&v).unwrap(), d);
    assert_eq!(d.masks[0].components[0].shape.region(), Some([0.25, 0.1, 0.5, 0.4]));
}

#[test]
fn a_describe_without_a_region_loads_and_saves_as_before() {
    let old = json!({"masks": [{"id": 1, "components": [{"shape": {"kind": "prompt", "text": "eye", "edge": 12.0}}]}]});
    let d = DevelopSettings::from_json(&old).unwrap();
    let MaskShape::Prompt { region, edge, .. } = &d.masks[0].components[0].shape else { panic!("a Describe") };
    assert_eq!((*region, *edge), (None, 12.0));
    let shape = &d.to_json()["masks"][0]["components"][0]["shape"];
    assert!(shape.get("region").is_none(), "{shape}");
}

#[test]
fn a_region_from_outside_is_put_inside_the_photo_or_dropped() {
    // corners swapped and past the photo: put in order, inside it
    let v = with_prompt(Some([0.6, 1.4, -0.2, 0.5])).to_json();
    assert_eq!(DevelopSettings::from_untrusted_json(&v).unwrap().masks[0].components[0].shape.region(), Some([0.0, 0.5, 0.6, 1.0]));
    // no area, or the whole photo: no region
    for r in [[0.3, 0.3, 0.3, 0.6], [0.0, 0.0, 1.0, 1.0], [-1.0, -1.0, 2.0, 2.0]] {
        let d = DevelopSettings::from_untrusted_json(&with_prompt(Some(r)).to_json()).unwrap();
        let MaskShape::Prompt { region, .. } = &d.masks[0].components[0].shape else { panic!("a Describe") };
        assert_eq!(*region, None, "{r:?}");
    }
    assert_eq!(prompt_region([f64::NAN, 0.0, 0.5, 0.5]), None);
}
