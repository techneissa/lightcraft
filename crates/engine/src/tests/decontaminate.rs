//! Fork feature (`fork/decontaminate.md`): new Background and Entire Person masks start decontaminated, others not;
//! `mask.decontaminate` sets the amount and refuses what it should.

use serde_json::json;

use crate::Session;

fn added(s: &mut Session, p: serde_json::Value) -> f64 {
    s.execute("mask.add", &p).unwrap();
    let id = s.active().unwrap();
    s.develop_of(id).unwrap().masks.last().unwrap().decontaminate
}

#[test]
fn background_starts_decontaminated() {
    let mut s = Session::with_demo();
    assert_eq!(added(&mut s, json!({"kind": "background"})), 100.0);
    assert_eq!(added(&mut s, json!({"kind": "radial"})), 0.0);
    assert_eq!(added(&mut s, json!({"kind": "subject"})), 0.0);
}

#[test]
fn the_amount_is_set_and_clamped() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    let id = s.active().unwrap();
    let get = |s: &Session| s.develop_of(id).unwrap().masks[0].decontaminate;
    s.execute("mask.decontaminate", &json!({"value": 40})).unwrap();
    assert_eq!(get(&s), 40.0);
    s.execute("mask.decontaminate", &json!({"value": 1e9})).unwrap();
    assert_eq!(get(&s), 100.0);
    s.execute("mask.decontaminate", &json!({"value": -5})).unwrap();
    assert_eq!(get(&s), 0.0);
    assert!(s.execute("mask.decontaminate", &json!({"id": 999, "value": 50})).is_err());
}

/// (People masks are not made by `mask.add` yet: Select People builds on this)
#[test]
fn whole_people_start_decontaminated_and_their_parts_not() {
    use lightcraft_develop::MaskShape;
    let people = |parts: &[&str]| MaskShape::people(0, parts.iter().map(|p| p.to_string()).collect());
    assert_eq!(crate::cmd::decontaminate_default("people", &people(&["Entire Person"])), 100.0);
    assert_eq!(crate::cmd::decontaminate_default("people", &people(&[])), 100.0);
    assert_eq!(crate::cmd::decontaminate_default("people", &people(&["hair"])), 0.0);
}
