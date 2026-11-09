//! The shared base: settings, controls, override matching, masks and the step.

mod common;

use lightcraft_raster::Plane;
use lightcraft_retouch::{FEATURES, Face, FaceKey, FaceOverride, Features, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};
use serde_json::json;

fn named(f: Face, name: &str) -> Face {
    Face { person: Some(name.into()), ..f }
}

#[test]
fn every_feature_has_a_section_and_unique_reachable_controls() {
    assert_eq!(FEATURES.len(), 10);
    let mut ids = std::collections::BTreeSet::new();
    let mut f = Features::default();
    for spec in lightcraft_retouch::controls() {
        assert!(ids.insert(spec.id), "duplicate {}", spec.id);
        assert!(spec.id.starts_with("retouch."), "{}", spec.id);
        assert!(spec.min <= spec.default && spec.default <= spec.max, "{}", spec.id);
        assert_eq!(f.get(spec.id), Some(spec.default), "{}", spec.id);
        assert!(f.set(spec.id, spec.max + 1000.0));
        assert_eq!(f.get(spec.id), Some(spec.max), "clamped: {}", spec.id);
        assert!(f.set(spec.id, f64::NAN));
        assert_eq!(f.get(spec.id), Some(spec.default), "not a number reads as the default: {}", spec.id);
        assert!(lightcraft_retouch::feature_of(spec.id).is_some());
    }
    assert!(!f.set("retouch.nope.amount", 1.0) && !f.set("retouch.skin", 1.0) && f.get("light.exposure").is_none());
    // a feature whose key starts like another's is not confused with it
    assert!(f.set("retouch.skinTone.faceLight", 30.0));
    assert_eq!((f.get("retouch.skin.smoothness"), f.get("retouch.skinTone.faceLight")), (Some(0.0), Some(30.0)));
    let mut keys: Vec<&str> = FEATURES.iter().map(|f| f.key).collect();
    keys.dedup();
    assert_eq!(keys.len(), FEATURES.len());
}

#[test]
fn the_block_is_off_by_default_and_round_trips_through_json() {
    let r = Retouch::default();
    assert!(r.is_default() && r.is_off());
    let mut r = Retouch::default();
    r.global.set("retouch.skin.smoothness", 40.0);
    r.faces.push(FaceOverride {
        face: FaceKey { person: Some("ann".into()), bounds: NormRect::new(0.1, 0.1, 0.3, 0.4), ..FaceKey::default() },
        settings: Features::default(),
        ..FaceOverride::default()
    });
    assert!(!r.is_off());
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["global"]["skin"]["smoothness"], json!(40.0));
    let back: Retouch = serde_json::from_value(v).unwrap();
    assert_eq!(back, r);
    // an override that is off for its face is still a setting, but renders nothing on its own
    let only_face = Retouch { faces: r.faces.clone(), ..Retouch::default() };
    assert!(only_face.is_off() && !only_face.is_default());
}

#[test]
fn a_malformed_block_reads_as_off_and_keeps_what_it_can() {
    let v = json!({
        "global": {"skin": {"smoothness": "lots"}, "eyes": {"whites": 30.0}, "future": {"x": 1}, "teeth_lips": 5},
        "faces": [{"face": {"bounds": {"x0": 0.0, "y0": 0.0, "x1": 0.0, "y1": 0.0}}, "settings": {}}, {"face": 3}, "x",
                  {"face": {"bounds": {"x0": 0.1, "y0": 0.1, "x1": 0.2, "y1": 0.2}}, "settings": {"eyes": {"whites": 1e9}}}]
    });
    let r: Retouch = serde_json::from_value(v).unwrap();
    assert_eq!(r.global.get("retouch.skin.smoothness"), Some(0.0));
    assert_eq!(r.global.get("retouch.eyes.whites"), Some(30.0));
    assert_eq!(r.global.get("retouch.teethLips.teethWhitening"), Some(0.0));
    assert_eq!(r.faces.len(), 1, "overrides without a usable box are dropped");
    assert_eq!(r.faces[0].settings.get("retouch.eyes.whites"), Some(100.0), "clamped on read");
    for bad in [json!(null), json!(7), json!("x"), json!([]), json!({"global": []})] {
        assert!(serde_json::from_value::<Retouch>(bad).unwrap().is_default());
    }
}

#[test]
fn an_override_stays_with_its_person_whatever_order_faces_come_in() {
    let ann = named(Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.4)), "ann");
    let bob = named(Face::boxed(NormRect::new(0.6, 0.1, 0.8, 0.4)), "bob");
    let anon = Face::boxed(NormRect::new(0.4, 0.5, 0.5, 0.7));
    let faces = vec![ann.clone(), bob.clone(), anon.clone()];
    let mut r = Retouch::default();
    r.override_mut(&faces, 0).unwrap().set("retouch.skin.smoothness", 10.0);
    r.override_mut(&faces, 2).unwrap().set("retouch.skin.smoothness", 30.0);
    assert_eq!(r.faces.len(), 2);
    let amount = |r: &Retouch, faces: &[Face], i: usize| r.settings_for(faces, i).get("retouch.skin.smoothness");
    for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0], [2, 0, 1]] {
        let shuffled: Vec<Face> = order.iter().map(|&i| faces[i].clone()).collect();
        for (pos, &i) in order.iter().enumerate() {
            let want = [Some(10.0), Some(0.0), Some(30.0)][i];
            assert_eq!(amount(&r, &shuffled, pos), want, "order {order:?}, face {i}");
        }
    }
    // Ann moved across the photo (another crop of the same scene): her settings follow her name
    let moved = vec![bob.clone(), named(Face::boxed(NormRect::new(0.55, 0.5, 0.75, 0.8)), "ann")];
    assert_eq!(amount(&r, &moved, 1), Some(10.0));
    assert_eq!(amount(&r, &moved, 0), Some(0.0));
    // someone else now stands where Ann was: her settings never go to them
    let swapped = vec![named(Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.4)), "carl")];
    assert_eq!(amount(&r, &swapped, 0), Some(0.0));
    // Ann not recognised this time, at her place: the same face, it keeps her settings
    assert_eq!(amount(&r, &[Face::boxed(NormRect::new(0.11, 0.1, 0.31, 0.4))], 0), Some(10.0));
    // the unnamed face's settings stay at its place, and go to a face named later at that place
    assert_eq!(amount(&r, &[named(anon.clone(), "dora")], 0), Some(30.0));
    // one override goes to one face only
    let twins = vec![Face::boxed(NormRect::new(0.4, 0.5, 0.5, 0.7)), Face::boxed(NormRect::new(0.41, 0.5, 0.51, 0.7))];
    let got: Vec<Option<f64>> = (0..2).map(|i| amount(&r, &twins, i)).collect();
    assert_eq!(got.iter().filter(|v| **v == Some(30.0)).count(), 1, "{got:?}");
    assert!(r.remove_override(&faces, 0));
    assert_eq!(amount(&r, &faces, 0), Some(0.0));
}

#[test]
fn masks_sample_inside_their_area_and_never_panic() {
    let m = PartMask { plane: Plane::from_fn(4, 2, |x, _| x as f32 / 3.0), area: NormRect::new(0.5, 0.5, 1.0, 1.0) };
    assert_eq!(m.sample(0.25, 0.75), 0.0, "outside its area");
    assert!(m.sample(0.99, 0.75) > 0.9 && m.sample(0.51, 0.75) < 0.1);
    for (x, y) in [(f64::NAN, 0.5), (f64::INFINITY, 0.7), (0.75, -0.0)] {
        assert!(m.sample(x, y).is_finite());
    }
    let broken = PartMask { plane: Plane { width: 5, height: 5, data: vec![0.5; 3] }, area: NormRect::new(0.0, 0.0, 1.0, 1.0) };
    assert_eq!(broken.sample(0.5, 0.5), 0.0);
    let empty = PartMask { plane: Plane::new(0, 0), area: NormRect::new(0.0, 0.0, 1.0, 1.0) };
    assert_eq!(empty.sample(0.5, 0.5), 0.0);
    let f = Face { masks: vec![(Part::Hair, m)], ..Face::default() };
    assert!(f.mask(Part::Hair).is_some() && f.mask(Part::Teeth).is_none());
}

#[test]
fn the_fingerprint_follows_every_part_of_the_input() {
    let a = Input::new(vec![common::face(0.2, 0.2, 0.5, 0.6)]);
    let mut b_face = common::face(0.2, 0.2, 0.5, 0.6);
    assert_eq!(a, Input::new(vec![b_face.clone()]));
    b_face.person = Some("ann".into());
    assert_ne!(a.fingerprint(), Input::new(vec![b_face.clone()]).fingerprint());
    let mut c_face = common::face(0.2, 0.2, 0.5, 0.6);
    if let Some((_, m)) = c_face.masks.first_mut() {
        m.plane.data[3] = 0.5;
    }
    assert_ne!(a.fingerprint(), Input::new(vec![c_face]).fingerprint());
    assert_ne!(Input::default().fingerprint(), a.fingerprint());
}

#[test]
fn the_scene_measures_faces_in_the_images_pixels() {
    let input = Input::new(vec![common::face(0.25, 0.25, 0.75, 0.5), Face::boxed(NormRect::new(0.5, 0.5, 0.4, 0.9))]);
    let view = WholePhoto { width: 200, height: 100 };
    let scene = lightcraft_retouch::Scene::new(&input, &view, 200, 100);
    assert_eq!(scene.faces.len(), 1, "a box that is not one is left out");
    let f = &scene.faces[0];
    assert_eq!(f.bounds, [50.0, 25.0, 150.0, 50.0]);
    assert_eq!(f.size, 100.0);
    assert_eq!(f.px(0.1), 10.0);
    assert_eq!(f.px(f64::NAN), 0.0);
    // the same face in a picture twice as large is twice as large: sizes follow the picture
    let big = WholePhoto { width: 400, height: 200 };
    assert_eq!(lightcraft_retouch::Scene::new(&input, &big, 400, 200).faces[0].px(0.1), 20.0);
    assert_eq!(scene.mask_at(0, Part::Hair, 100, 30), 1.0);
    assert_eq!(scene.mask_at(0, Part::Hair, 10, 10), 0.0);
    assert_eq!(scene.mask_at(7, Part::Hair, 10, 10), 0.0);
    assert!(scene.mask_plane(0, Part::Teeth).is_some_and(|p| (p.width, p.height) == (200, 100)));
}

#[test]
fn the_step_runs_nothing_when_off_even_with_overrides() {
    let before = common::picture(32, 24);
    let mut img = before.clone();
    let faces = vec![common::face(0.2, 0.2, 0.6, 0.7)];
    let mut r = Retouch::default();
    r.override_mut(&faces, 0);
    assert!(r.is_off());
    lightcraft_retouch::run::apply(&mut img, &r, &Input::new(faces), &WholePhoto { width: 32, height: 24 }, Stage::Retouch);
    assert_eq!(img, before);
}
