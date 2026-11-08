use serde_json::json;

use crate::Session;
use lightcraft_catalog::PhotoId;

fn adjust(s: &Session, id: u32) -> lightcraft_develop::LocalAdjustments {
    s.develop_of(s.active().unwrap()).unwrap().masks.iter().find(|m| m.id == id).unwrap().adjust
}

fn new_mask(s: &mut Session) -> u32 {
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.active_mask.unwrap()
}

#[test]
fn recolor_sets_the_colour_and_amount_and_refuses_bad_values() {
    let mut s = Session::with_demo();
    let m = new_mask(&mut s);
    let v = s.execute("mask.recolor", &json!({"color": "#7a3fb0"})).unwrap();
    assert_eq!(v, json!({"color": "#7a3fb0", "amount": 100.0}), "a first colour turns Recolor on");
    let a = adjust(&s, m);
    assert!((a.recolor_r - 0x7a as f64 / 255.0).abs() < 1e-9 && a.recolor == 100.0);
    s.execute("mask.recolor", &json!({"amount": 140})).unwrap();
    assert_eq!(adjust(&s, m).recolor, 100.0, "clamped");
    s.execute("mask.recolor", &json!({"amount": 40, "color": "#fff"})).unwrap();
    assert_eq!((adjust(&s, m).recolor, adjust(&s, m).recolor_b), (40.0, 1.0));
    for bad in [json!({}), json!({"color": "purple"}), json!({"color": 5}), json!({"amount": "x"}), json!({"id": 999, "amount": 1})] {
        assert!(s.execute("mask.recolor", &bad).is_err(), "{bad}");
    }
    // mask.adjust takes the fields too, clamped
    s.execute("mask.adjust", &json!({"values": {"recolor": -5, "recolor_g": 3}})).unwrap();
    assert_eq!((adjust(&s, m).recolor, adjust(&s, m).recolor_g), (0.0, 1.0));
    // one undo step each
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(adjust(&s, m).recolor, 40.0);
}

#[test]
fn a_mask_without_recolor_is_saved_as_before() {
    let mut s = Session::with_demo();
    let m = new_mask(&mut s);
    let saved = serde_json::to_value(adjust(&s, m)).unwrap();
    assert!(saved.get("recolor").is_none() && saved.get("recolor_r").is_none(), "{saved}");
    s.execute("mask.recolor", &json!({"color": "#102030", "amount": 50})).unwrap();
    let saved = serde_json::to_value(adjust(&s, m)).unwrap();
    assert_eq!(saved["recolor"], 50.0);
    let back: lightcraft_develop::LocalAdjustments = serde_json::from_value(saved).unwrap();
    assert_eq!(back, adjust(&s, m));
}

#[test]
fn the_eyedropper_takes_a_colour_from_another_photo() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.catalog.photos().take(2).map(|p| p.id.0).collect();
    s.execute("library.select", &json!({"ids": [ids[0]], "active": ids[0]})).unwrap();
    let m = new_mask(&mut s);
    let other = PhotoId(ids[1]);
    let want = super::color_at(&mut s, other, 0.3, 0.6, "test").unwrap();
    let v = s.execute("mask.recolorPick", &json!({"x": 0.3, "y": 0.6, "photo": ids[1]})).unwrap();
    assert_eq!(v["color"], lightcraft_pipeline::recolor::to_hex(want));
    let a = adjust(&s, m);
    assert_eq!([a.recolor_r, a.recolor_g, a.recolor_b], want);
    assert_eq!(a.recolor, 100.0);
    assert!(s.execute("mask.recolorPick", &json!({"x": 0.3})).is_err());
    assert!(s.execute("mask.recolorPick", &json!({"x": 0.3, "y": 0.2, "photo": "two"})).is_err());
    assert!(s.execute("mask.recolorPick", &json!({"x": 0.3, "y": 0.2, "photo": 99_999})).is_err());
}

#[test]
fn effects_and_paste_carry_the_recolor() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.catalog.photos().take(2).map(|p| p.id.0).collect();
    s.execute("library.select", &json!({"ids": [ids[0]], "active": ids[0]})).unwrap();
    let m = new_mask(&mut s);
    s.execute("mask.recolor", &json!({"color": "#7a3fb0", "amount": 80})).unwrap();
    s.execute("mask.saveEffect", &json!({"name": "Purple"})).unwrap();
    let effect = s.mask_effects.iter().find(|e| e.name == "Purple").unwrap().clone();
    assert!(effect.matches(&adjust(&s, m)));
    let n = new_mask(&mut s);
    s.execute("mask.applyEffect", &json!({"name": "Purple", "id": n, "remember": false})).unwrap();
    let (a, b) = (adjust(&s, m), adjust(&s, n));
    assert_eq!((b.recolor, b.recolor_r, b.recolor_b), (a.recolor, a.recolor_r, a.recolor_b));
    // copy the masks to the other photo
    s.execute("develop.copy", &json!({"groups": ["masks"]})).unwrap();
    s.execute("develop.paste", &json!({"ids": [ids[1]]})).unwrap();
    let pasted = s.develop_of(PhotoId(ids[1])).unwrap();
    assert!(pasted.masks.iter().any(|x| x.adjust.recolor == 80.0 && x.adjust.recolor_r == a.recolor_r));
}
