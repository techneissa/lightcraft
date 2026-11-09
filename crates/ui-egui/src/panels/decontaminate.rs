//! Fork feature (`fork/decontaminate.md`, issue #115): the mask's Decontaminate slider, under Refine Edges.

use egui::Ui;
use lightcraft_develop::{ControlSpec, Mask, Section, Track};
use serde_json::json;

use super::edit::apply_slider_out;
use crate::LightcraftApp;
use crate::widgets::slider;

/// The slider of mask `m`: it resets to the amount the mask started with (100 for Background, 0 for most kinds).
fn spec_for(m: &Mask) -> ControlSpec {
    ControlSpec {
        id: "decontaminate",
        label: "Decontaminate",
        section: Section::Light,
        min: 0.0,
        max: 100.0,
        default: lightcraft_engine::cmd::decontaminate_start(m),
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    }
}

pub fn slider_row(app: &mut LightcraftApp, ui: &mut Ui, m: &Mask) {
    let spec = spec_for(m);
    let out = slider(ui, &spec, m.decontaminate, true, None);
    apply_slider_out(app, &spec, out, |app, v| app.run("mask.decontaminate", json!({"value": v})));
}

#[cfg(test)]
mod tests {
    use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};

    fn mask(shape: MaskShape) -> Mask {
        Mask { components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], ..Default::default() }
    }

    #[test]
    fn the_slider_resets_to_the_masks_start_value() {
        assert_eq!(super::spec_for(&mask(MaskShape::background())).default, 100.0);
        assert_eq!(super::spec_for(&mask(MaskShape::sky())).default, 0.0);
        assert_eq!(super::spec_for(&Mask::default()).default, 0.0);
    }
}
