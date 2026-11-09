//! The picker makes one mask per part pick, each with its own edits (`fork/select-people.md`): a plain click makes
//! (or selects) the mask of that part and leaves the others alone, ⌘-click changes the selected one, an unedited
//! mask is re-targeted, a dot marks the parts that have a mask, and the dropdown sits under the face chips.

use lightcraft_develop::Mask;
use serde_json::json;

use super::tests::{T, app, click, masks, part, person, rect_of, until};
use crate::headless::Headless;

fn edit(h: &mut Headless, values: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": "mask.adjust", "params": {"values": values}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
}

fn mask_with(h: &Headless, part_id: &str) -> Mask {
    masks(h)
        .into_iter()
        .find(|m| matches!(&m.components[0].shape, lightcraft_develop::MaskShape::People { parts, .. } if parts == &[part_id.to_string()]))
        .unwrap_or_else(|| panic!("no mask of {part_id}"))
}

/// The app with an Entire Person mask made by the Person button (nobody has edited it).
fn with_person_mask(tag_people: usize) -> (Headless, std::path::PathBuf) {
    let people = if tag_people == 1 { vec![person(0.1, Some("ann"))] } else { vec![person(0.1, Some("ann")), person(0.6, None)] };
    let (mut h, dir) = app(people);
    click(&mut h, "button:maskMode-person", false);
    until(&mut h, "a Person mask", |h| !masks(h).is_empty());
    (h, dir)
}

#[test]
fn a_plain_click_after_edits_makes_a_second_mask_and_keeps_the_first_ones_edits() {
    let (mut h, dir) = with_person_mask(2);
    click(&mut h, "selectPeople:part:hair", false);
    assert_eq!(masks(&h).len(), 1, "the unedited Entire Person mask became Hair");
    edit(&mut h, json!({"exposure": 1.5}));
    click(&mut h, "selectPeople:part:lips", false);
    assert_eq!(masks(&h).len(), 2, "Lips is a mask of its own");
    assert_eq!(part(&h).0, ["lips"], "and is the selected one");
    edit(&mut h, json!({"saturation": 30.0}));
    let (hair, lips) = (mask_with(&h, "hair"), mask_with(&h, "lips"));
    assert_eq!((hair.adjust.exposure, hair.adjust.saturation), (1.5, 0.0), "Hair keeps its own edit");
    assert_eq!((lips.adjust.exposure, lips.adjust.saturation), (0.0, 30.0));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_plain_click_on_a_part_that_has_a_mask_selects_it() {
    let (mut h, dir) = with_person_mask(2);
    click(&mut h, "selectPeople:part:hair", false);
    edit(&mut h, json!({"exposure": 1.5}));
    click(&mut h, "selectPeople:part:lips", false);
    edit(&mut h, json!({"saturation": 30.0}));
    assert_eq!(masks(&h).len(), 2);
    click(&mut h, "selectPeople:part:hair", false);
    assert_eq!(masks(&h).len(), 2, "no third mask");
    assert_eq!(h.app.session.active_mask, Some(mask_with(&h, "hair").id), "Hair's mask is selected again");
    assert_eq!(mask_with(&h, "hair").adjust.exposure, 1.5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_command_click_adds_a_part_to_the_selected_mask() {
    let (mut h, dir) = with_person_mask(2);
    click(&mut h, "selectPeople:part:hair", false);
    edit(&mut h, json!({"exposure": 1.5}));
    click(&mut h, "selectPeople:part:lips", true);
    assert_eq!(masks(&h).len(), 1, "the same mask gained Lips");
    assert_eq!(part(&h).0, ["hair", "lips"]);
    assert_eq!(masks(&h)[0].adjust.exposure, 1.5, "and kept its edit");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unedited_picker_mask_is_changed_instead_of_left_empty() {
    let (mut h, dir) = with_person_mask(2);
    click(&mut h, "selectPeople:part:hair", false);
    click(&mut h, "selectPeople:part:lips", false);
    click(&mut h, "selectPeople:part:teeth", false);
    assert_eq!(masks(&h).len(), 1, "no empty mask is left behind");
    assert_eq!(part(&h).0, ["teeth"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_face_chip_selects_the_mask_that_exists_and_otherwise_changes_whose() {
    let (mut h, dir) = with_person_mask(2);
    click(&mut h, "selectPeople:part:hair", false);
    edit(&mut h, json!({"exposure": 1.0}));
    click(&mut h, "selectPeople:face:1", false);
    assert_eq!(masks(&h).len(), 1, "no Hair mask of the second person: the mask changed whose it is");
    assert_eq!(part(&h), (vec!["hair".to_string()], true));
    // a plain click on Lips (that person) makes its own mask; All then changes whose Lips it is
    click(&mut h, "selectPeople:part:lips", false);
    assert_eq!(masks(&h).len(), 2);
    click(&mut h, "selectPeople:face:all", false);
    assert_eq!(masks(&h).len(), 2, "Lips of everyone does not exist: Lips changed whose");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dot_marks_the_parts_that_have_a_mask() {
    let (mut h, dir) = with_person_mask(2);
    click(&mut h, "selectPeople:part:hair", false);
    edit(&mut h, json!({"exposure": 1.0}));
    click(&mut h, "selectPeople:part:lips", false);
    // open the menu: dots on Hair and Lips only
    click(&mut h, "selectPeople:parts", false);
    h.step();
    assert!(rect_of(&mut h, "selectPeople:partsMenu").is_some(), "the menu is open");
    assert!(rect_of(&mut h, "selectPeople:partMask:hair").is_some(), "Hair has a mask");
    assert!(rect_of(&mut h, "selectPeople:partMask:lips").is_some(), "Lips has a mask");
    assert!(rect_of(&mut h, "selectPeople:partMask:teeth").is_none(), "Teeth has none");
    let name = rect_of(&mut h, "selectPeople:part:hair").unwrap();
    let dot = rect_of(&mut h, "selectPeople:partMask:hair").unwrap();
    assert!(dot[0] > name[0] + 26.0, "the dot is after the name: {dot:?} in {name:?}");
    // another face: its own parts have no mask
    click(&mut h, "selectPeople:face:1", false);
    click(&mut h, "selectPeople:parts", false);
    h.step();
    assert!(rect_of(&mut h, "selectPeople:partMask:hair").is_none(), "no mask of the second person's Hair");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_parts_dropdown_is_on_its_own_line_under_the_chips() {
    let (mut h, dir) = with_person_mask(2);
    let all = rect_of(&mut h, "selectPeople:face:all").unwrap();
    let last = rect_of(&mut h, "selectPeople:face:1").unwrap();
    let drop = rect_of(&mut h, "selectPeople:parts").unwrap();
    let card = rect_of(&mut h, "selectPeople:card").unwrap();
    // [x, y, w, h]
    assert!(drop[1] >= last[1] + last[3], "below the chips: {drop:?} under {last:?}");
    assert!(drop[1] >= all[1] + all[3]);
    assert!((drop[0] - all[0]).abs() < 0.5, "left-aligned with the chips: {drop:?} {all:?}");
    assert!(drop[1] + drop[3] <= card[1] + card[3], "inside the card");
    let _ = std::fs::remove_dir_all(dir);
}
