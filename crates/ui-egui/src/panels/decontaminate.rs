//! Fork feature (`fork/decontaminate.md`, issue #115): the mask's Decontaminate slider, under Refine Edges.

use egui::Ui;
use lightcraft_develop::{ControlSpec, Mask, Section, Track};
use serde_json::json;

use super::edit::apply_slider_out;
use crate::LightcraftApp;
use crate::widgets::slider;

pub fn slider_row(app: &mut LightcraftApp, ui: &mut Ui, m: &Mask) {
    let spec = ControlSpec {
        id: "decontaminate",
        label: "Decontaminate",
        section: Section::Light,
        min: 0.0,
        max: 100.0,
        default: 0.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &spec, m.decontaminate, true, None);
    apply_slider_out(app, &spec, out, |app, v| app.run("mask.decontaminate", json!({"value": v})));
}
