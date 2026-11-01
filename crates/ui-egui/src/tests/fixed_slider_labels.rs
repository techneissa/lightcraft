//! Issues #21 and #29: the slider label column of the Edit and Masking panels is one fixed width, whatever sections
//! are open, sized to the common labels (not the longest one), and the labels are left-aligned in it, so opening a
//! section with long labels moves nothing. A label too long for it shrinks, then wraps onto two lines.

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

    /// Where the text `label` ends.
    fn text_right(&self, label: &str) -> f32 {
        let t = self.texts.iter().find(|t| t.galley.job.text == label).unwrap_or_else(|| panic!("{label} not drawn"));
        t.pos.x + t.galley.size().x
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

/// The width of `label` at the normal slider label font.
fn normal_width(label: &str) -> f32 {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::theme::apply(&ctx);
    let font = crate::theme::Tokens::get(&ctx).font(crate::widgets::LABEL_FONT);
    let mut width = 0.0;
    for frame in 0..2 {
        let mut out = ctx.run_ui(HeadlessView::raw_input(vec2(300.0, 300.0), 1.0, frame as f64 / 60.0, vec![]), |ui| {
            width = ui.painter().layout_no_wrap(label.to_string(), font.clone(), egui::Color32::WHITE).size().x;
        });
        out.textures_delta.clear();
    }
    width
}

fn widest(labels: &[&str]) -> f32 {
    labels.iter().map(|l| normal_width(l)).fold(0.0, f32::max)
}

const NORMAL_FONT: f32 = crate::widgets::LABEL_FONT;

#[test]
fn the_edit_column_fits_the_common_labels_not_the_longest() {
    let passes = edit_panel(crate::state::RIGHT_WIDTH.default);
    let column = passes[0].rect("sliderLabel:light.exposure").width();
    assert_eq!(column, passes[1].rect("sliderLabel:light.exposure").width(), "Detail opened: the column changed width");
    let common =
        widest(&["Exposure", "Contrast", "Highlights", "Shadows", "Whites", "Blacks", "Vibrance", "Saturation", "Texture", "Clarity", "Dehaze"]);
    assert!(column >= common, "column {column} is narrower than the common labels ({common})");
    assert!(column <= common + 4.0, "column {column} is wider than the common labels ({common})");
    assert!(column < normal_width("Color Noise Reduction") - 20.0, "column {column} still fits Color Noise Reduction");
}

#[test]
fn the_tracks_start_right_after_the_short_labels() {
    let d = &edit_panel(crate::state::RIGHT_WIDTH.default)[1];
    let gap = d.rect("slider:light.exposure").left() - d.text_right("Exposure");
    assert!(gap < 40.0, "{gap} points of empty space between Exposure and its track");
}

/// The text drawn for `label` and the label rect of `id`, if the panel drew them.
fn label_in<'a>(d: &'a Drawn, id: &str, label: &str) -> Option<(&'a egui::epaint::TextShape, Rect)> {
    let rect = d.widgets.iter().find(|(w, _)| w == id)?.1;
    // (the text inside the label's row: a section header can have the same name)
    Some((
        d.texts.iter().find(|t| t.galley.job.text == label && t.pos.y >= rect.top() && t.pos.y <= rect.bottom() && t.pos.x >= rect.left() - 0.5)?,
        rect,
    ))
}

fn font_size(t: &egui::epaint::TextShape) -> f32 {
    t.galley.job.sections.first().map_or(0.0, |s| s.format.font_id.size)
}

#[test]
fn a_long_label_wraps_onto_two_small_lines_inside_its_column() {
    let d = &edit_panel(crate::state::RIGHT_WIDTH.default)[1];
    let (text, label) = label_in(d, "sliderLabel:detail.nrColor", "Color Noise Reduction").expect("Color Noise Reduction not drawn");
    let track = d.rect("slider:detail.nrColor");
    assert_eq!(text.galley.rows.len(), 2, "Color Noise Reduction should wrap onto two lines");
    assert!(font_size(text) < NORMAL_FONT, "the wrapped label should use a smaller font");
    assert!(
        text.pos.x >= label.left() - 0.5 && text.pos.x + text.galley.size().x <= label.right() + 0.5,
        "{label:?} does not hold the text at {:?}",
        text.pos
    );
    assert!(text.pos.y >= label.top() && text.pos.y + text.galley.size().y <= label.bottom(), "the two lines leave their row {label:?}");
    assert!(label.right() < track.left(), "{label:?} runs into the track {track:?}");
    // the row grew for the two lines; the track stays centred in it as in a normal row
    let normal = d.rect("sliderLabel:light.exposure");
    assert!(label.height() > normal.height(), "the row did not grow: {} vs {}", label.height(), normal.height());
    let offset = |label: Rect, track: Rect| track.center().y - label.center().y;
    assert!((offset(label, track) - offset(normal, d.rect("slider:light.exposure"))).abs() < 0.5, "the track is off centre in the tall row");
}

#[test]
fn every_detail_label_stays_inside_its_column() {
    for width in [crate::state::RIGHT_WIDTH.default, crate::state::RIGHT_WIDTH.min] {
        let d = &edit_panel(width)[1];
        let mut shrunk = false;
        for spec in lightcraft_develop::controls::in_section(lightcraft_develop::controls::Section::Detail) {
            let label = crate::i18n::tr(spec.label).to_string();
            let Some((text, rect)) = label_in(d, &format!("sliderLabel:{}", spec.id), &label) else { continue };
            let track = d.rect(&format!("slider:{}", spec.id));
            assert!(text.galley.rows.len() <= 2, "{label} takes {} lines", text.galley.rows.len());
            assert!(text.pos.x + text.galley.size().x <= rect.right() + 0.5, "{label} overflows its column {rect:?} at {width}");
            assert!(rect.right() < track.left(), "{label} runs into its track at {width}");
            shrunk |= font_size(text) < NORMAL_FONT;
        }
        assert!(shrunk, "no Detail label needed a smaller font at {width}");
    }
}

#[test]
fn the_mask_column_is_narrower_than_the_longest_mask_label() {
    let passes = mask_panel(crate::state::RIGHT_WIDTH.default, &["linear"]);
    let column = passes[0].rect("sliderLabel:exposure").width();
    let common = widest(&["Exposure", "Contrast", "Highlights", "Saturation", "Sharpness", "Defringe"]);
    assert!(column >= common && column <= common + 4.0, "mask column {column}, common labels {common}");
    for spec in crate::panels::masking::LOCAL {
        let label = crate::i18n::tr(spec.label).to_string();
        let Some((text, rect)) = label_in(&passes[0], &format!("sliderLabel:{}", spec.id), &label) else { continue };
        assert!(text.pos.x + text.galley.size().x <= rect.right() + 0.5, "{label} overflows its column {rect:?}");
    }
}
