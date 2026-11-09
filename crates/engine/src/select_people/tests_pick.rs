//! `selectPeople.pick` (`fork/select-people.md`): one mask per part pick, each with its own edits.

use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
use serde_json::{Value, json};

use super::tests::session;
use crate::Session;

fn masks(s: &Session) -> Vec<Mask> {
    s.develop_of(s.active().unwrap()).map(|d| d.masks.clone()).unwrap_or_default()
}

fn exposure(s: &Session, id: u32) -> f64 {
    masks(s).iter().find(|m| m.id == id).unwrap().adjust.exposure
}

fn steps(s: &Session) -> usize {
    s.catalog.photo(s.active().unwrap()).unwrap().history.len()
}

fn pick(s: &mut Session, parts: &[&str], face: Value, how: &str) -> Value {
    s.execute("selectPeople.pick", &json!({"parts": parts, "face": face, "how": how})).unwrap()
}

fn mask_id(r: &Value) -> u32 {
    r["mask"].as_u64().unwrap() as u32
}

/// Hair of everyone with Exposure 1.5: a mask nobody can re-target.
fn edited_hair(s: &mut Session) -> u32 {
    let hair = mask_id(&pick(s, &["hair"], json!("all"), "part"));
    s.execute("mask.adjust", &json!({"id": hair, "values": {"exposure": 1.5}})).unwrap();
    hair
}

#[test]
fn a_plain_click_after_edits_makes_a_second_mask_and_keeps_the_first_ones_edits() {
    let (mut s, dir) = session("pick-new");
    let first = pick(&mut s, &["hair"], json!("all"), "part");
    assert_eq!(first["created"], true);
    let hair = mask_id(&first);
    s.execute("mask.adjust", &json!({"id": hair, "values": {"exposure": 1.5}})).unwrap();
    let before = steps(&s);
    let lips = mask_id(&pick(&mut s, &["lips"], json!("all"), "part"));
    assert_eq!(steps(&s), before + 1, "making the mask is one undo step");
    assert_ne!(lips, hair);
    assert_eq!(masks(&s).len(), 2);
    assert_eq!(s.active_mask, Some(lips));
    assert_eq!((exposure(&s, hair), exposure(&s, lips)), (1.5, 0.0), "the first mask's edits are untouched");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(masks(&s).len(), 1);
    assert_eq!(exposure(&s, hair), 1.5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_plain_click_on_a_part_that_has_a_mask_selects_it_without_an_edit() {
    let (mut s, dir) = session("pick-existing");
    let hair = edited_hair(&mut s);
    let lips = mask_id(&pick(&mut s, &["lips"], json!("all"), "part"));
    s.execute("mask.adjust", &json!({"id": lips, "values": {"saturation": 20.0}})).unwrap();
    let before = steps(&s);
    let r = pick(&mut s, &["hair"], json!("all"), "part");
    assert_eq!((mask_id(&r), r["selected"].clone()), (hair, json!(true)));
    assert_eq!(s.active_mask, Some(hair));
    assert_eq!(masks(&s).len(), 2, "no third mask");
    assert_eq!(steps(&s), before, "selecting a mask is not an edit");
    // another face of the same part is another mask
    assert_eq!(pick(&mut s, &["hair"], json!(0), "part")["created"], true);
    assert_eq!(masks(&s).len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_command_click_adds_the_part_to_the_selected_mask() {
    let (mut s, dir) = session("pick-add");
    let hair = edited_hair(&mut s);
    let r = pick(&mut s, &["hair", "lips"], Value::Null, "add");
    assert_eq!(r["activeMask"], hair);
    assert_eq!(masks(&s).len(), 1, "the same mask gained a part");
    assert_eq!((exposure(&s, hair), masks(&s)[0].name.as_str()), (1.5, "Hair + Lips Mask 1"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_command_click_keeps_the_masks_face_unless_one_is_given() {
    let (mut s, dir) = session("pick-add-face");
    let ann = mask_id(&pick(&mut s, &["hair"], json!(0), "part"));
    s.execute("mask.adjust", &json!({"id": ann, "values": {"exposure": 1.0}})).unwrap();
    // face null: Ann's, still
    pick(&mut s, &["hair", "lips"], Value::Null, "add");
    let MaskShape::People { face: Some(face), .. } = &masks(&s)[0].components[0].shape else { panic!("Ann's face is kept") };
    assert_eq!(face.person.as_deref(), Some("ann"));
    // "all" is everyone's, said out loud
    pick(&mut s, &["hair", "lips"], json!("all"), "add");
    assert!(matches!(masks(&s)[0].components[0].shape, MaskShape::People { face: None, .. }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_command_click_that_would_copy_another_masks_parts_is_refused() {
    let (mut s, dir) = session("pick-add-dup");
    let hair = edited_hair(&mut s);
    let both = mask_id(&pick(&mut s, &["lips"], json!("all"), "part"));
    s.execute("mask.adjust", &json!({"id": both, "values": {"saturation": 20.0}})).unwrap();
    pick(&mut s, &["hair", "lips"], Value::Null, "add");
    let before = steps(&s);
    // removing Lips would leave exactly Hair of everyone, which has its own mask
    let e = s.execute("selectPeople.pick", &json!({"parts": ["hair"], "how": "add"})).unwrap_err().to_string();
    assert!(e.contains("Hair already has a mask"), "{e}");
    assert_eq!(steps(&s), before, "nothing changed");
    assert_eq!(s.active_mask, Some(both));
    assert_eq!(masks(&s).len(), 2);
    assert!(masks(&s).iter().any(|m| m.id == hair));
    let _ = std::fs::remove_dir_all(dir);
}

/// A mask with one more part (a radial) next to its Select People part.
fn with_radial(s: &mut Session, mid: u32) {
    let id = s.active().unwrap();
    let mut d = (*s.develop_of(id).unwrap()).clone();
    let m = d.masks.iter_mut().find(|m| m.id == mid).unwrap();
    m.components.push(MaskComponent {
        name: None,
        op: MaskOp::Subtract,
        invert: false,
        shape: MaskShape::Radial { center: lightcraft_geom::Point::new(0.5, 0.5), rx: 0.2, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
    });
    s.set_develop(id, d, "Add Part").unwrap();
}

#[test]
fn a_mask_with_other_parts_still_counts_as_its_parts_mask_but_is_never_retargeted() {
    let (mut s, dir) = session("pick-other-parts");
    let hair = mask_id(&pick(&mut s, &["hair"], json!("all"), "part"));
    with_radial(&mut s, hair);
    let lips = mask_id(&pick(&mut s, &["lips"], json!("all"), "part"));
    assert_ne!(lips, hair, "an unedited Hair mask with a second part is not re-targeted");
    assert_eq!(masks(&s).len(), 2);
    s.execute("mask.adjust", &json!({"id": lips, "values": {"saturation": 20.0}})).unwrap();
    // a plain click on Hair selects the mask with the radial
    let r = pick(&mut s, &["hair"], json!("all"), "part");
    assert_eq!((mask_id(&r), r["selected"].clone()), (hair, json!(true)));
    assert_eq!(masks(&s).len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unedited_mask_made_by_the_picker_is_changed_not_left_behind() {
    let (mut s, dir) = session("pick-empty");
    let first = mask_id(&pick(&mut s, &["hair"], json!("all"), "part"));
    let r = pick(&mut s, &["lips"], json!("all"), "part");
    assert_eq!(r["activeMask"], first, "an empty mask is re-targeted");
    assert_eq!((masks(&s).len(), masks(&s)[0].name.as_str()), (1, "Lips Mask 1"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn each_kind_of_edit_stops_the_retarget() {
    type Edit = fn(&mut Session, u32);
    let edits: [(&str, Edit); 7] = [
        ("a slider", |s, m| drop(s.execute("mask.adjust", &json!({"id": m, "values": {"contrast": 10.0}})).unwrap())),
        ("Recolour only", |s, m| drop(s.execute("mask.recolor", &json!({"id": m, "color": "#7a3fb0", "amount": 50})).unwrap())),
        ("Amount only", |s, m| drop(s.execute("mask.adjust", &json!({"id": m, "values": {"amount": 50.0}})).unwrap())),
        ("Invert", |s, m| drop(s.execute("mask.invert", &json!({"id": m})).unwrap())),
        ("Refine Edges", |s, m| drop(s.execute("mask.refine", &json!({"id": m, "value": 40})).unwrap())),
        ("a rename", |s, m| drop(s.execute("mask.rename", &json!({"id": m, "name": "Mine"})).unwrap())),
        ("hiding it", |s, m| drop(s.execute("mask.visible", &json!({"id": m, "visible": false})).unwrap())),
    ];
    for (what, edit) in edits {
        let (mut s, dir) = session("pick-blocked");
        let first = mask_id(&pick(&mut s, &["hair"], json!("all"), "part"));
        edit(&mut s, first);
        let second = mask_id(&pick(&mut s, &["lips"], json!("all"), "part"));
        assert_ne!(second, first, "{what} stops the re-target");
        assert_eq!(masks(&s).len(), 2, "{what}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_renamed_part_or_a_changed_mode_stops_the_retarget() {
    let (mut s, dir) = session("pick-part-edits");
    let first = mask_id(&pick(&mut s, &["hair"], json!("all"), "part"));
    s.execute("mask.component", &json!({"id": first, "component": 0, "action": "rename", "name": "Strands"})).unwrap();
    assert_ne!(mask_id(&pick(&mut s, &["lips"], json!("all"), "part")), first, "a named part");
    let second = mask_id(&pick(&mut s, &["teeth"], json!("all"), "part"));
    let id = s.active().unwrap();
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.masks.iter_mut().find(|m| m.id == second).unwrap().components[0].op = MaskOp::Subtract;
    s.set_develop(id, d, "Mode").unwrap();
    assert_ne!(mask_id(&pick(&mut s, &["iris"], json!("all"), "part")), second, "another mode");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clicking_a_part_that_has_a_mask_deletes_the_empty_one_in_the_same_undo_step() {
    let (mut s, dir) = session("pick-empty-gone");
    let hair = edited_hair(&mut s);
    // an empty Lips mask is selected (made by the picker, not edited)... with Hair's mask existing
    let empty = mask_id(&pick(&mut s, &["lips"], json!("all"), "part"));
    assert_eq!(masks(&s).len(), 2);
    let before = steps(&s);
    let r = pick(&mut s, &["hair"], json!("all"), "part");
    assert_eq!((mask_id(&r), r["selected"].clone()), (hair, json!(true)));
    assert_eq!(s.active_mask, Some(hair));
    let left = masks(&s);
    assert_eq!((left.len(), left[0].id), (1, hair), "the empty mask is gone");
    assert!(left.iter().all(|m| m.id != empty));
    assert_eq!(steps(&s), before + 1, "one undo step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(masks(&s).len(), 2, "undo brings it back");
    assert_eq!(exposure(&s, hair), 1.5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_edited_selected_mask_is_not_deleted_when_another_part_is_picked() {
    let (mut s, dir) = session("pick-edited-stays");
    let hair = edited_hair(&mut s);
    let lips = mask_id(&pick(&mut s, &["lips"], json!("all"), "part"));
    s.execute("mask.adjust", &json!({"id": lips, "values": {"saturation": 20.0}})).unwrap();
    pick(&mut s, &["hair"], json!("all"), "part");
    assert_eq!(masks(&s).len(), 2, "Lips has edits: it stays");
    assert_eq!(s.active_mask, Some(hair));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_face_pick_changes_whose_unless_that_mask_exists() {
    let (mut s, dir) = session("pick-face");
    let hair = edited_hair(&mut s);
    let ann = mask_id(&pick(&mut s, &["hair"], json!(0), "part"));
    s.execute("mask.adjust", &json!({"values": {"exposure": 2.0}})).unwrap();
    assert_eq!(masks(&s).len(), 2);
    // a face chip: All again, the mask of everyone's Hair exists → selected, nothing changed
    let r = pick(&mut s, &["hair"], json!("all"), "face");
    assert_eq!((mask_id(&r), r["selected"].clone()), (hair, json!(true)));
    assert_eq!(exposure(&s, ann), 2.0);
    // Bob: no such mask, so the selected one changes whose it is
    let r = pick(&mut s, &["hair"], json!(1), "face");
    assert_eq!(r["activeMask"], hair);
    assert_eq!(masks(&s).len(), 2, "changed, not added");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_is_refused() {
    let (mut s, dir) = session("pick-refused");
    let bad = |s: &mut Session, p: Value| s.execute("selectPeople.pick", &p).unwrap_err().to_string();
    assert!(bad(&mut s, json!({"parts": ["elbows"]})).contains("unknown part"));
    assert!(bad(&mut s, json!({"parts": ["hair"], "face": 9})).contains("people"), "a face past the people");
    assert!(bad(&mut s, json!({"parts": ["hair"], "face": -1})).contains("face"));
    assert!(bad(&mut s, json!({"parts": []})).contains("at least one"));
    assert!(bad(&mut s, json!({"how": "sideways"})).contains("how"));
    assert!(bad(&mut s, json!({"how": 3, "parts": ["hair"]})).contains("how"), "`how` must be a string");
    assert!(bad(&mut s, json!({"how": "add"})).contains("parts"), "`add` needs `parts`");
    assert!(bad(&mut s, json!({"how": "add", "parts": null})).contains("parts"));
    assert!(masks(&s).is_empty(), "nothing was made");
    let _ = std::fs::remove_dir_all(dir);
}
