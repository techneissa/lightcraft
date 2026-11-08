//! A slider's columns come from its control, not its value: going from one digit to three, or adding a minus sign,
//! never moves the track or the label.
use super::{slider, take_registry};
use crate::headless::HeadlessView;
use egui::{Rect, vec2};
use lightcraft_develop::controls;

fn rects(id: &str, value: f64) -> (Rect, Rect) {
    let spec = controls::find(id).unwrap();
    let mut view = HeadlessView::new();
    view.run(HeadlessView::raw_input(vec2(300.0, 100.0), 1.0, 0.0, vec![]), |ui| {
        slider(ui, spec, value, true, None);
    });
    let reg = take_registry(&view.ctx);
    let find = |n: String| reg.iter().find(|(i, _)| *i == n).unwrap().1;
    (find(format!("slider:{id}")), find(format!("sliderLabel:{id}")))
}

#[test]
fn the_track_and_label_stay_put_whatever_the_value() {
    for id in ["light.contrast", "light.exposure", "detail.sharpenAmount"] {
        let spec = controls::find(id).unwrap();
        let base = rects(id, spec.default);
        for v in [0.0, 5.0, 50.0, -100.0, 100.0, spec.min, spec.max] {
            assert_eq!(rects(id, v.clamp(spec.min, spec.max)), base, "{id} at {v}");
        }
    }
}

#[test]
fn the_track_runs_close_to_the_value() {
    let spec = controls::find("light.contrast").unwrap();
    let mut view = HeadlessView::new();
    view.run(HeadlessView::raw_input(vec2(300.0, 100.0), 1.0, 0.0, vec![]), |ui| {
        slider(ui, spec, 0.0, true, None);
    });
    let reg = take_registry(&view.ctx);
    let find = |n: &str| reg.iter().find(|(i, _)| i == n).unwrap().1;
    let gap = find("sliderValue:light.contrast").left() - find("slider:light.contrast").right();
    assert!((6.0..=8.0).contains(&gap), "gap {gap}");
}
