//! The Masking panel's Recolor rows (issue #83): the new colour (swatch with the colour picker, hex value,
//! eyedropper), the Colour to Replace (automatic, or picked with its own eyedropper), Recolor Amount and Range. The
//! main colour of the mask's area takes the new colour and keeps its shading; other colours inside the mask stay.

use egui::vec2;
use lightcraft_develop::{ControlSpec, Mask, Section, Track};
use lightcraft_pipeline::recolor::{parse_hex, source_rgb, to_hex};
use serde_json::json;

use super::edit::apply_slider_out;
use crate::LightcraftApp;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::{icon_button, register, slider};

/// The on-canvas tool that picks the new colour from the photo (`mask.recolorPick`).
pub const PICK_TOOL: &str = "recolorPick";
/// The on-canvas tool that picks the Colour to Replace (`mask.recolorPick {target: "from"}`).
pub const PICK_FROM_TOOL: &str = "recolorPickFrom";

/// The `mask.recolorPick` target of on-canvas tool `tool` (`None`: not a Recolor eyedropper).
pub fn pick_target(tool: &str) -> Option<&'static str> {
    match tool {
        PICK_TOOL => Some("to"),
        PICK_FROM_TOOL => Some("from"),
        _ => None,
    }
}

/// The Recolor amount slider (`LocalAdjustments::recolor`).
pub const AMOUNT: ControlSpec = ControlSpec {
    id: "recolor",
    label: "Recolor Amount",
    section: Section::Light,
    min: 0.0,
    max: 100.0,
    default: 0.0,
    step: 1.0,
    decimals: 0,
    track: Track::Plain,
};

/// The Range slider (`LocalAdjustments::recolor_range`): which colours count as the area's main colour.
pub const RANGE: ControlSpec = ControlSpec {
    id: "recolorRange",
    label: "Range",
    section: Section::Light,
    min: -100.0,
    max: 100.0,
    default: 0.0,
    step: 1.0,
    decimals: 0,
    track: Track::Centered,
};

/// The rows, for the selected mask `m`.
pub fn row(app: &mut LightcraftApp, ui: &mut egui::Ui, m: &Mask) {
    let t = Tokens::get(ui.ctx());
    let a = m.adjust;
    let rgb = [a.recolor_r, a.recolor_g, a.recolor_b];
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 2 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Recolor")).font(t.font(12.0)).color(t.text_label));
            swatch(app, ui, rgb);
            hex_field(app, ui, m.id, rgb);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                picker(app, ui, "recolorPicker", PICK_TOOL, tr("Pick Recolor Color"));
            });
        });
        let from = source_rgb(&a);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Color to Replace")).font(t.font(12.0)).color(t.text_label));
            if from.is_none() {
                ui.label(egui::RichText::new(tr("Automatic")).font(t.font(12.0)).color(t.text_dim));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                picker(app, ui, "recolorFromPicker", PICK_FROM_TOOL, tr("Pick Color to Replace"));
            });
        });
        if let Some(from) = from {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 14.0), egui::Sense::hover());
                let [r, g, b] = from.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
                ui.label(egui::RichText::new(to_hex(from)).font(t.font(12.0)).color(t.text));
                let auto = ui.small_button(tr("Automatic"));
                register(ui, "button:recolorFromAuto", auto.rect);
                if auto.clicked() {
                    let _ = app.run("mask.recolor", json!({"from": null}));
                }
            });
        }
    });
    let out = slider(ui, &AMOUNT, a.recolor, true, None);
    apply_slider_out(app, &AMOUNT, out, |app, v| app.run("mask.recolor", json!({"amount": v})));
    let out = slider(ui, &RANGE, a.recolor_range, true, None);
    apply_slider_out(app, &RANGE, out, |app, v| app.run("mask.recolor", json!({"range": v})));
}

/// An eyedropper button that arms (or disarms) on-canvas tool `tool`.
fn picker(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, tool: &str, tip: &str) {
    let active = app.ui.tool == tool;
    if icon_button(ui, id, Icon::Picker, vec2(24.0, 24.0), active, true, tip).clicked() {
        if active {
            app.ui.tool.clear();
        } else {
            app.ui.tool = tool.into();
        }
    }
}

/// The colour swatch with egui's colour picker; a drag in the picker is one undo step.
fn swatch(app: &mut LightcraftApp, ui: &mut egui::Ui, rgb: [f64; 3]) {
    let before = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let mut c = before;
    let r = ui.color_edit_button_srgb(&mut c).on_hover_text(tr("Recolor Color"));
    register(ui, "recolorSwatch", r.rect);
    let held = egui::Id::unique("lc-recolor-picking");
    let picking = ui.data(|d| d.get_temp::<bool>(held)).unwrap_or(false);
    if c != before {
        if !picking {
            let _ = app.run("develop.beginInteraction", json!({"label": "Recolor"}));
            ui.data_mut(|d| d.insert_temp(held, true));
        }
        let _ = app.run("mask.recolor", json!({"color": to_hex(c.map(|v| f64::from(v) / 255.0))}));
    } else if picking && !ui.input(|i| i.pointer.any_down()) {
        let _ = app.run("develop.endInteraction", json!({}));
        ui.data_mut(|d| d.remove::<bool>(held));
    }
}

/// The colour as `#rrggbb`, editable; Return or leaving the field sets it, a bad value is refused.
fn hex_field(app: &mut LightcraftApp, ui: &mut egui::Ui, mask: u32, rgb: [f64; 3]) {
    let draft = egui::Id::unique("lc-recolor-hex").with(mask);
    let current = to_hex(rgb);
    let mut text = ui.data(|d| d.get_temp::<String>(draft)).unwrap_or_else(|| current.clone());
    let field = crate::text_field::TextField::singleline("field:recolorHex", &mut text).width(70.0).select_on_focus(true).show(ui);
    if field.ending.is_some() || !field.editing {
        ui.data_mut(|d| d.remove::<String>(draft));
        if field.committed() && text.trim() != current {
            match parse_hex(&text) {
                Some(c) => {
                    let _ = app.run("mask.recolor", json!({"color": to_hex(c)}));
                }
                None => app.toast(ui.ctx(), tr("Enter a color as #rrggbb")),
            }
        }
    } else {
        ui.data_mut(|d| d.insert_temp(draft, text.clone()));
    }
}
