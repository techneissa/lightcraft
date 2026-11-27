use lightcraft_develop::DevelopSettings;
use serde_json::json;

use crate::Session;

fn develop(s: &Session) -> std::sync::Arc<DevelopSettings> {
    s.develop_of(s.active().unwrap()).unwrap()
}

/// The entries are saved with the edit, undone one step at a time, reordered, and the sliders are reachable as indexed
/// controls: the same commands an automation client uses.
#[test]
fn entries_are_saved_undoable_and_reachable_by_command() {
    let mut s = Session::with_demo();
    s.execute("colorSwap.add", &json!({"from": "#3050c0", "to": "#c03020", "name": "Sky"})).unwrap();
    s.execute("colorSwap.add", &json!({"from": "#20a040"})).unwrap();
    let d = develop(&s);
    assert_eq!(d.color_swaps.len(), 2);
    assert!(d.color_swaps[0].is_active() && !d.color_swaps[1].is_active(), "an entry with no To colour changes nothing");
    s.execute("colorSwap.set", &json!({"index": 1, "to": "#ffd000", "intensity": 70, "keepLightness": false})).unwrap();
    s.execute("develop.set", &json!({"control": "colorSwap.1.range", "value": 80})).unwrap();
    let e = develop(&s).color_swaps[1].clone();
    assert_eq!((e.intensity, e.range, e.keep_lightness), (70.0, 80.0, false));
    s.execute("colorSwap.move", &json!({"index": 1, "to": 0})).unwrap();
    assert_eq!(develop(&s).color_swaps[0].range, 80.0);
    // saved with the edit: the settings round-trip through their JSON
    let saved = serde_json::to_value(&*develop(&s)).unwrap();
    let back: DevelopSettings = serde_json::from_value(saved).unwrap();
    assert_eq!(back.color_swaps, develop(&s).color_swaps);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(develop(&s).color_swaps[1].range, 80.0, "one undo step per command");
    s.execute("colorSwap.remove", &json!({"index": 0})).unwrap();
    assert_eq!(develop(&s).color_swaps.len(), 1);
    for bad in
        [json!({"index": 5, "on": false}), json!({"index": 0, "from": "red"}), json!({"index": 0, "range": "wide"}), json!({"index": 0, "name": 3})]
    {
        assert!(s.execute("colorSwap.set", &bad).is_err(), "{bad}");
    }
    assert_eq!(develop(&s).color_swaps.len(), 1);
    // a ninth entry is refused
    for _ in 0..7 {
        s.execute("colorSwap.add", &json!({})).unwrap();
    }
    assert!(s.execute("colorSwap.add", &json!({})).is_err());
}

/// Copy, paste and sync carry the entries (their own settings group).
#[test]
fn copy_and_paste_carry_the_entries() {
    let mut s = Session::with_demo();
    let ids: Vec<_> = s.visible_cloned().into_iter().take(2).collect();
    s.selection = crate::Selection { ids: vec![ids[0]], active: Some(ids[0]) };
    s.execute("colorSwap.add", &json!({"from": "#3050c0", "to": "#c03020", "range": 30})).unwrap();
    s.execute("develop.copy", &json!({"groups": lightcraft_develop::SettingsGroup::default_copy()})).unwrap();
    s.selection = crate::Selection { ids: vec![ids[1]], active: Some(ids[1]) };
    s.execute("develop.paste", &json!({})).unwrap();
    let pasted = develop(&s).color_swaps.clone();
    assert_eq!(pasted.len(), 1);
    assert_eq!((pasted[0].from, pasted[0].range), (Some([0x30 as f64 / 255.0, 0x50 as f64 / 255.0, 0xc0 as f64 / 255.0]), 30.0));
}

/// Picking from the photo: with no entry named it adds one from that colour; a point off the photo is an error.
#[test]
fn picking_a_colour_adds_an_entry_and_refuses_points_off_the_photo() {
    let mut s = Session::with_demo();
    let v = s.execute("colorSwap.pick", &json!({"x": 0.4, "y": 0.5})).unwrap();
    let hex = v["color"].as_str().unwrap().to_string();
    let e = develop(&s).color_swaps[0].clone();
    assert_eq!(lightcraft_pipeline::recolor::to_hex(e.from.unwrap()), hex);
    assert!(e.to.is_none());
    s.execute("colorSwap.pick", &json!({"x": 0.6, "y": 0.2, "index": 0, "target": "to"})).unwrap();
    assert!(develop(&s).color_swaps[0].to.is_some());
    assert!(s.execute("colorSwap.pick", &json!({"x": 1.5, "y": 0.5})).is_err());
    assert!(s.execute("colorSwap.pick", &json!({"x": 0.5, "y": 0.5, "target": "to"})).is_err(), "a To colour needs an entry");
    assert_eq!(develop(&s).color_swaps.len(), 1);
}
