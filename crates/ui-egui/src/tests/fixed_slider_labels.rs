//! Issue #21: the slider label column of the Edit and Masking panels is one fixed width, whatever sections are open,
//! and the labels are left-aligned in it, so opening a section with long labels moves nothing.

use egui::{Rect, vec2};
use serde_json::json;

use crate::{LightcraftApp, Services, headless::HeadlessView};

fn text_shapes(shape: &egui::Shape, out: &mut Vec<egui::epaint::TextShape>) {
    match shape {
        egui::Shape::Text(text) => out.push(text.clone()),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| text_shapes(s, out)),
        _ => {}
    }
}

/// What one pass of a panel drew.
struct Drawn {
    texts: Vec<egui::epaint::TextShape>,
    widgets: Vec<(String, Rect)>,
}

impl Drawn {
    fn rect(&self, id: &str) -> Rect {
        self.widgets.iter().find(|(w, _)| w == id).unwrap_or_else(|| panic!("{id} not drawn")).1
    }

    /// Where the text `label` starts (left edge of its glyphs' origin).
    fn text_left(&self, label: &str) -> f32 {
        self.texts.iter().find(|t| t.galley.job.text == label).unwrap_or_else(|| panic!("{label} not drawn")).pos.x
    }
}

/// A panel drawn in one context, once per entry of `states` (each run for a few frames, so what a pass leaves for
/// the next has settled), with `set` putting the app into that state first.
fn draw<S: Copy>(
    states: &[S],
    width: f32,
    prepare: impl Fn(&mut LightcraftApp),
    set: impl Fn(&mut LightcraftApp, S),
    show: impl Fn(&mut LightcraftApp, &mut egui::Ui, lightcraft_catalog::PhotoId),
) -> Vec<Drawn> {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    prepare(&mut app);
    let photo = app.session.active().unwrap();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::theme::apply(&ctx);
    let mut frame = 0;
    let mut all = Vec::new();
    for state in states {
        set(&mut app, *state);
        let mut drawn = Drawn { texts: Vec::new(), widgets: Vec::new() };
        for _ in 0..4 {
            let raw = HeadlessView::raw_input(vec2(width, 4000.0), 1.0, frame as f64 / 60.0, vec![]);
            frame += 1;
            let mut out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| show(&mut app, ui, photo));
            });
            out.textures_delta.clear();
            drawn.texts.clear();
            for shape in &out.shapes {
                text_shapes(&shape.shape, &mut drawn.texts);
            }
            drawn.widgets = crate::widgets::take_registry(&ctx);
        }
        all.push(drawn);
    }
    all
}

const LIGHT: &[&str] = &["light"];
const LIGHT_AND_DETAIL: &[&str] = &["light", "detail"];

fn edit_panel(width: f32) -> Vec<Drawn> {
    draw(
        &[LIGHT, LIGHT_AND_DETAIL, LIGHT],
        width,
        |_| {},
        |app, open| app.ui.open_sections = open.iter().map(|s| s.to_string()).collect(),
        crate::panels::edit::show,
    )
}

#[test]
fn opening_noise_reduction_moves_neither_the_label_nor_the_track() {
    let passes = edit_panel(crate::state::RIGHT_WIDTH.default);
    let rects = |d: &Drawn| (d.rect("sliderLabel:light.exposure"), d.rect("slider:light.exposure"), d.rect("sliderValue:light.exposure"));
    let (light, both, light_again) = (rects(&passes[0]), rects(&passes[1]), rects(&passes[2]));
    // the Detail section is really there, with its long labels
    passes[1].rect("sliderLabel:detail.nrColor");
    assert_eq!(light, both, "Detail opened: the Exposure row moved");
    assert_eq!(light, light_again, "Detail closed again: the Exposure row moved");
}

#[test]
fn edit_labels_start_at_the_left_of_their_column() {
    let d = &edit_panel(crate::state::RIGHT_WIDTH.default)[1];
    let column = d.rect("sliderLabel:light.exposure").left();
    for label in ["Exposure", "Contrast", "Highlights", "Shadows"] {
        let left = d.text_left(label);
        assert!((left - column).abs() < 0.5, "{label} starts at {left}, its column at {column}");
    }
}

#[test]
fn a_long_label_in_a_narrow_panel_still_fits_its_column() {
    let d = &edit_panel(crate::state::RIGHT_WIDTH.min)[1];
    let label = d.rect("sliderLabel:detail.nrColor");
    let track = d.rect("slider:detail.nrColor");
    assert!(label.right() < track.left(), "{label:?} runs into the track {track:?}");
    assert!(label.width() <= (crate::state::RIGHT_WIDTH.min * 0.41).max(52.0) + 0.5, "{label:?} is wider than its cap");
}

fn mask_panel(width: f32, kinds: &[&str]) -> Vec<Drawn> {
    draw(
        kinds,
        width,
        |app| {
            app.run("mask.add", json!({"kind": "linear", "start": [0.5, 0.2], "end": [0.5, 0.7]})).unwrap();
        },
        |app, kind| {
            app.run("mask.add", json!({"kind": kind, "start": [0.5, 0.2], "end": [0.5, 0.7], "center": [0.5, 0.5], "radius": [0.2, 0.2]})).unwrap();
        },
        crate::panels::masking::show,
    )
}

#[test]
fn the_mask_labels_start_at_the_left_of_one_fixed_column() {
    let passes = mask_panel(crate::state::RIGHT_WIDTH.default, &["linear", "radial"]);
    let first = passes[0].rect("sliderLabel:exposure");
    // (the panel grows with the second mask, so only the columns are compared)
    let columns = |d: &Drawn| ["sliderLabel:exposure", "slider:exposure", "sliderValue:exposure"].map(|id| (d.rect(id).left(), d.rect(id).right()));
    assert_eq!(columns(&passes[0]), columns(&passes[1]), "another mask moved the Exposure columns");
    for label in ["Exposure", "Contrast", "Highlights"] {
        let left = passes[0].text_left(label);
        assert!((left - first.left()).abs() < 0.5, "{label} starts at {left}, its column at {}", first.left());
    }
}
