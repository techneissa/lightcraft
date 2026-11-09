//! Per-face settings kept in one frame (after or before lens corrections and transform, [`KeyFrame`]) match faces
//! of the other once the photo's map takes them into one frame, and never by place without it.

use lightcraft_retouch::overrides::{resolve, resolve_in};
use lightcraft_retouch::{Face, FaceKey, KeyFrame, KeyMap, NormRect, Retouch};

/// A lens that moves everything 0.1 to the right: a point after it is 0.1 to the left before it.
struct Shift;

impl KeyMap for Shift {
    fn edit_to_photo(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        Some((x - 0.1, y))
    }
}

fn on_photo(b: NormRect) -> Face {
    Face { upright: Some(b), key_frame: KeyFrame::Photo, ..Face::boxed(b) }
}

#[test]
fn a_key_after_the_lens_matches_its_face_before_it_not_the_neighbour_at_its_place() {
    let (a, b) = (NormRect::new(0.2, 0.2, 0.28, 0.3), NormRect::new(0.3, 0.2, 0.38, 0.3));
    // made under YuNet: A's box after the lens is where B stands on the photo
    let mut r = Retouch::default();
    let yunet = [Face::boxed(b)];
    r.override_mut(&yunet, 0).unwrap().set("retouch.skinTone.amount", 90.0);
    assert_eq!(r.faces[0].face.frame, KeyFrame::Edit);
    let parsed = [on_photo(a), on_photo(b)];
    assert_eq!(resolve_in(&r, &parsed, Some(&Shift)), [Some(0), None], "A keeps it, B doesn't take it");
    assert_eq!(resolve(&r, &parsed), [None, None], "without the map, never by place");
    // and the other way round: made on the parse, matched to YuNet's face after the lens
    let mut r = Retouch::default();
    r.override_mut(&parsed, 0).unwrap().set("retouch.skinTone.amount", 90.0);
    assert_eq!(r.faces[0].face.frame, KeyFrame::Photo);
    assert_eq!(resolve_in(&r, &[Face::boxed(b), Face::boxed(NormRect::new(0.2, 0.2, 0.28, 0.3))], Some(&Shift)), [Some(0), None]);
    // a named face still finds its person's settings across frames, with or without a map
    let mut r = Retouch::default();
    r.override_mut(&[Face { person: Some("ann".into()), ..Face::boxed(b) }], 0).unwrap();
    assert_eq!(resolve(&r, &[Face { person: Some("ann".into()), ..on_photo(a) }]), [Some(0)]);
}

#[test]
fn a_keys_frame_is_saved_only_when_it_is_before_the_lens() {
    let edit = FaceKey { bounds: NormRect::new(0.1, 0.1, 0.2, 0.2), ..FaceKey::default() };
    assert!(serde_json::to_value(&edit).unwrap().get("frame").is_none(), "older keys read the same");
    let photo = FaceKey { frame: KeyFrame::Photo, ..edit.clone() };
    let v = serde_json::to_value(&photo).unwrap();
    assert_eq!(v["frame"], "photo");
    assert_eq!(serde_json::from_value::<FaceKey>(v).unwrap(), photo);
    let old: FaceKey = serde_json::from_value(serde_json::json!({"bounds": {"x0": 0.1, "y0": 0.1, "x1": 0.2, "y1": 0.2}})).unwrap();
    assert_eq!(old.frame, KeyFrame::Edit);
}

#[test]
fn a_map_that_fails_or_gives_no_box_pairs_nothing_by_place() {
    struct Nowhere;
    impl KeyMap for Nowhere {
        fn edit_to_photo(&self, _: f64, _: f64) -> Option<(f64, f64)> {
            None
        }
    }
    struct Nan;
    impl KeyMap for Nan {
        fn edit_to_photo(&self, _: f64, _: f64) -> Option<(f64, f64)> {
            Some((f64::NAN, 0.5))
        }
    }
    let b = NormRect::new(0.3, 0.2, 0.38, 0.3);
    let mut r = Retouch::default();
    r.override_mut(&[Face::boxed(b)], 0).unwrap();
    assert_eq!(resolve_in(&r, &[on_photo(b)], Some(&Nowhere)), [None]);
    assert_eq!(resolve_in(&r, &[on_photo(b)], Some(&Nan)), [None]);
}
