//! Fork feature (`fork/spot-rows.md`): the commands behind a spot's row in the Remove panel (select, hide or show, the
//! circle setting, delete), each one undo step.

use lightcraft_develop::SpotCircle;
use serde_json::json;

use crate::Session;

fn three_spots() -> Session {
    let mut s = Session::with_demo();
    for (x, y) in [(0.25, 0.3), (0.5, 0.6), (0.75, 0.3)] {
        s.execute("spot.add", &json!({"mode": "heal", "points": [[x, y]], "size": 0.03, "source": [0.1, 0.0]})).unwrap();
    }
    s
}

fn spots(s: &Session) -> Vec<lightcraft_develop::Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

#[test]
fn a_row_selects_its_spot() {
    let mut s = three_spots();
    assert_eq!(s.active_spot, Some(2));
    s.execute("spot.select", &json!({"index": 0})).unwrap();
    assert_eq!(s.active_spot, Some(0));
    assert!(s.execute("spot.select", &json!({"index": 3})).is_err(), "no row 3");
}

#[test]
fn hide_and_show_are_one_undo_step_each_and_leave_the_other_spots_alone() {
    let mut s = three_spots();
    let r = s.execute("spot.visible", &json!({"index": 1})).unwrap();
    assert_eq!(r, json!({"index": 1, "visible": false}), "no `visible`: the other way round");
    assert_eq!(spots(&s).iter().map(|p| p.hidden).collect::<Vec<_>>(), vec![false, true, false]);
    // hiding again says nothing new and adds no undo step
    s.execute("spot.visible", &json!({"index": 1, "visible": false})).unwrap();
    s.execute("spot.visible", &json!({"index": 1, "visible": true})).unwrap();
    assert!(!spots(&s)[1].hidden);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s)[1].hidden, "undo brings the hide back: show was one step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!spots(&s)[1].hidden, "one more undo: hide was one step, the repeated hide was none");
    // the selected spot when `index` is left out
    s.execute("spot.select", &json!({"index": 2})).unwrap();
    s.execute("spot.visible", &json!({})).unwrap();
    assert!(spots(&s)[2].hidden);
    assert_eq!(s.active_spot, Some(2), "hiding a spot does not change the selection");
}

#[test]
fn a_hidden_spot_is_saved_with_the_photo() {
    let mut s = three_spots();
    s.execute("spot.visible", &json!({"index": 0, "visible": false})).unwrap();
    let d = s.develop_of(s.active().unwrap()).unwrap();
    let back = lightcraft_develop::DevelopSettings::from_untrusted_json(&d.to_json()).unwrap();
    assert!(back.spots[0].hidden && !back.spots[1].hidden, "the settings a sidecar or the catalog stores keep it");
}

#[test]
fn the_circle_setting_is_per_spot_and_one_undo_step() {
    let mut s = three_spots();
    let r = s.execute("spot.circle", &json!({"index": 0, "mode": "Never"})).unwrap();
    assert_eq!(r, json!({"index": 0, "mode": "never"}));
    s.execute("spot.circle", &json!({"index": 2, "mode": "always"})).unwrap();
    assert_eq!(spots(&s).iter().map(|p| p.circle).collect::<Vec<_>>(), vec![Some(SpotCircle::Never), None, Some(SpotCircle::Always)]);
    assert!(spots(&s).iter().all(|p| !p.hidden), "the circle setting is not the effect");
    s.execute("spot.circle", &json!({"index": 0, "mode": "global"})).unwrap();
    assert_eq!(spots(&s)[0].circle, None, "global = follow Tool Overlay");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s)[0].circle, Some(SpotCircle::Never), "undo: global was one step");
    assert!(s.execute("spot.circle", &json!({"index": 0, "mode": "sometimes"})).is_err());
    assert!(s.execute("spot.circle", &json!({"index": 0})).is_err());
    assert!(s.execute("spot.circle", &json!({"index": 9, "mode": "auto"})).is_err());
    assert!(s.execute("spot.visible", &json!({"index": 9})).is_err());
    assert!(s.execute("spot.visible", &json!({"index": 0, "visible": "no"})).is_err());
}

#[test]
fn delete_by_row_is_one_undo_step_and_keeps_the_selection_on_its_spot() {
    let mut s = three_spots();
    s.execute("spot.circle", &json!({"index": 2, "mode": "selected"})).unwrap();
    s.execute("spot.delete", &json!({"index": 0})).unwrap();
    assert_eq!(spots(&s).len(), 2);
    assert_eq!(s.active_spot, Some(1), "the selected spot moved up one row");
    assert_eq!(spots(&s)[1].circle, Some(SpotCircle::Selected));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s).len(), 3, "one undo step brings the spot back");
}

#[test]
fn undoing_a_delete_forgets_the_selected_spot_whose_index_moved() {
    let mut s = three_spots();
    assert_eq!(s.active_spot, Some(2));
    s.execute("spot.delete", &json!({"index": 1})).unwrap();
    assert_eq!(s.active_spot, Some(1), "C moved up a row and is still the selected one");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s).len(), 3);
    assert_eq!(s.active_spot, None, "B is back at index 1: nothing points at the wrong spot");
    s.execute("spot.select", &json!({"index": 2})).unwrap();
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(spots(&s).len(), 2);
    assert_eq!(s.active_spot, None, "redo changes the spots again: forgotten again");
    // an undo that leaves the spot count alone (hide, circle) keeps the selection
    s.execute("spot.select", &json!({"index": 1})).unwrap();
    s.execute("spot.visible", &json!({"index": 0, "visible": false})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.active_spot, Some(1));
}
