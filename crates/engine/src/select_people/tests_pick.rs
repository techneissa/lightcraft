//! `selectPeople.pick` (`fork/select-people.md`): one mask per part pick, each with its own edits.

use lightcraft_develop::Mask;
use serde_json::json;

use super::tests::session;
use crate::Session;

fn masks(s: &Session) -> Vec<Mask> {
    s.develop_of(s.active().unwrap()).map(|d| d.masks.clone()).unwrap_or_default()
}

fn exposure(s: &Session, id: u32) -> f64 {
    masks(s).iter().find(|m| m.id == id).unwrap().adjust.exposure
}

fn pick(s: &mut Session, parts: &[&str], face: serde_json::Value, how: &str) -> serde_json::Value {
    s.execute("selectPeople.pick", &json!({"parts": parts, "face": face, "how": how})).unwrap()
}

#[test]
fn a_plain_click_after_edits_makes_a_second_mask_and_keeps_the_first_ones_edits() {
    let (mut s, dir) = session("pick-new");
    // the first pick makes Hair; the user edits it
    let first = pick(&mut s, &["hair"], json!("all"), "part");
    let hair = first["mask"].as_u64().unwrap() as u32;
    assert_eq!(first["created"], true);
    s.execute("mask.adjust", &json!({"id": hair, "values": {"exposure": 1.5}})).unwrap();
    // Lips: a new mask, selected; Hair keeps its exposure
    let id = s.active().unwrap();
    let steps = s.catalog.photo(id).unwrap().history.len();
    let second = pick(&mut s, &["lips"], json!("all"), "part");
    assert_eq!(s.catalog.photo(id).unwrap().history.len(), steps + 1, "one step");
    let lips = second["mask"].as_u64().unwrap() as u32;
    assert_ne!(lips, hair);
    assert_eq!(masks(&s).len(), 2);
    assert_eq!(s.active_mask, Some(lips));
    assert_eq!(exposure(&s, hair), 1.5, "the first mask's edits are untouched");
    assert_eq!(exposure(&s, lips), 0.0);
    // creating it is one undo step
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(masks(&s).len(), 1);
    assert_eq!(exposure(&s, hair), 1.5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_plain_click_on_a_part_that_has_a_mask_selects_it_without_an_edit() {
    let (mut s, dir) = session("pick-existing");
    let hair = pick(&mut s, &["hair"], json!("all"), "part")["mask"].as_u64().unwrap() as u32;
    s.execute("mask.adjust", &json!({"id": hair, "values": {"exposure": 1.0}})).unwrap();
    let lips = pick(&mut s, &["lips"], json!("all"), "part")["mask"].as_u64().unwrap() as u32;
    s.execute("mask.adjust", &json!({"id": lips, "values": {"saturation": 20.0}})).unwrap();
    let id = s.active().unwrap();
    let before = s.catalog.photo(id).unwrap().history.len();
    let r = pick(&mut s, &["hair"], json!("all"), "part");
    assert_eq!((r["mask"].as_u64().unwrap() as u32, r["selected"].clone()), (hair, json!(true)));
    assert_eq!(s.active_mask, Some(hair));
    assert_eq!(masks(&s).len(), 2, "no third mask");
    assert_eq!(s.catalog.photo(id).unwrap().history.len(), before, "selecting a mask is not an edit");
    // another face of the same part is another mask
    let ann = pick(&mut s, &["hair"], json!(0), "part");
    assert_eq!(ann["created"], true);
    assert_eq!(masks(&s).len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_command_click_adds_the_part_to_the_selected_mask() {
    let (mut s, dir) = session("pick-add");
    let hair = pick(&mut s, &["hair"], json!("all"), "part")["mask"].as_u64().unwrap() as u32;
    s.execute("mask.adjust", &json!({"id": hair, "values": {"exposure": 1.0}})).unwrap();
    // (the picker sends the whole new list)
    let r = pick(&mut s, &["hair", "lips"], serde_json::Value::Null, "add");
    assert_eq!(r["activeMask"], hair);
    assert_eq!(masks(&s).len(), 1, "the same mask gained a part");
    assert_eq!(exposure(&s, hair), 1.0);
    let m = &masks(&s)[0];
    assert_eq!(m.name, "Hair + Lips Mask 1");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unedited_mask_made_by_the_picker_is_changed_not_left_behind() {
    let (mut s, dir) = session("pick-empty");
    let first = pick(&mut s, &["hair"], json!("all"), "part")["mask"].as_u64().unwrap() as u32;
    let r = pick(&mut s, &["lips"], json!("all"), "part");
    assert_eq!(r["activeMask"], first, "an empty mask is re-targeted");
    assert_eq!(masks(&s).len(), 1);
    assert_eq!(masks(&s)[0].name, "Lips Mask 1");
    // one edit and the next pick makes a new mask
    s.execute("mask.adjust", &json!({"values": {"contrast": 10.0}})).unwrap();
    pick(&mut s, &["teeth"], json!("all"), "part");
    assert_eq!(masks(&s).len(), 2);
    // a renamed mask is not the picker's to change
    s.execute("mask.rename", &json!({"id": masks(&s)[1].id, "name": "Mine"})).unwrap();
    pick(&mut s, &["iris"], json!("all"), "part");
    assert_eq!(masks(&s).len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_face_pick_changes_whose_unless_that_mask_exists() {
    let (mut s, dir) = session("pick-face");
    let hair = pick(&mut s, &["hair"], json!("all"), "part")["mask"].as_u64().unwrap() as u32;
    s.execute("mask.adjust", &json!({"values": {"exposure": 1.0}})).unwrap();
    // Ann's Hair exists already
    let ann = pick(&mut s, &["hair"], json!(0), "part")["mask"].as_u64().unwrap() as u32;
    s.execute("mask.adjust", &json!({"values": {"exposure": 2.0}})).unwrap();
    assert_eq!(masks(&s).len(), 2);
    // a face chip: All again, the mask of everyone's Hair exists → selected, nothing changed
    let r = pick(&mut s, &["hair"], json!("all"), "face");
    assert_eq!((r["mask"].as_u64().unwrap() as u32, r["selected"].clone()), (hair, json!(true)));
    assert_eq!(exposure(&s, ann), 2.0);
    // Bob: no such mask, so the selected one changes whose it is
    let r = pick(&mut s, &["hair"], json!(1), "face");
    assert_eq!(r["activeMask"], hair);
    assert_eq!(masks(&s).len(), 2, "changed, not added");
    assert!(s.execute("selectPeople.pick", &json!({"how": "sideways"})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
