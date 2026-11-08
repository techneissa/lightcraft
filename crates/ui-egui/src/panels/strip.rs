//! The vertical tool strip at the far right (Presets, Edit, Crop, Remove, Masking, Red Eye …).

use egui::vec2;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::panels::tool_strip_order::{Placed, Tool};
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::{icon_button, icon_button_sense};

/// Height of one tool button.
const BUTTON_H: f32 = 40.0;
/// Space between tool buttons.
const GAP: f32 = 6.0;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::right("tool_strip")
        .exact_size(t.strip_w)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(t.chrome)
                .inner_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 8 })
                .stroke(egui::Stroke::new(1.0, t.divider)),
        )
        .show(ui, |ui| {
            crate::widgets::announce_region(ui, egui::accesskit::Role::Toolbar, crate::i18n::tr("Tool strip"));
            let has_photo = app.session.active().is_some();
            let order = app.ui.tool_strip.tools().to_vec();
            let (top, bottom) = order.split_at(order.len().saturating_sub(BOTTOM_PLACES));
            let mut placed: Placed = Vec::with_capacity(order.len());
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = GAP;
                let sz = vec2(t.strip_w, BUTTON_H);
                for (i, tool) in top.iter().enumerate() {
                    // the dividers stay in their places while the icons move
                    if DIVIDERS_BEFORE.contains(&i) {
                        separator(ui, &t);
                    }
                    placed.push((*tool, tool_button(app, ui, *tool, sz, has_photo)));
                }
                separator(ui, &t);
                if icon_button(ui, "more", Icon::More, sz, false, true, "About LightCraft").clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::About);
                }
            });
            // bottom icons: two buttons and one gap tall, so the last ends at the bottom margin as
            // the first starts at the top one
            let bottom_edge = ui.max_rect().bottom();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
                egui::pos2(ui.max_rect().left(), bottom_edge - (2.0 * BUTTON_H + GAP)),
                ui.max_rect().right_bottom(),
            )));
            child.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = GAP;
                let sz = vec2(t.strip_w, BUTTON_H);
                for tool in bottom {
                    placed.push((*tool, tool_button(app, ui, *tool, sz, has_photo)));
                }
            });
            crate::panels::tool_strip_order::drag(app, ui, &placed);
        });
}

/// How many icons are pinned to the bottom of the strip.
const BOTTOM_PLACES: usize = 2;
/// Before which place (counted from the top) a divider is drawn.
const DIVIDERS_BEFORE: [usize; 2] = [2, 7];

/// One icon: a click opens its panel, a drag moves it (see `tool_strip_order`), its right-click
/// menu moves it too. Returns where it is.
fn tool_button(app: &mut LightcraftApp, ui: &mut egui::Ui, tool: Tool, sz: egui::Vec2, has_photo: bool) -> egui::Rect {
    let (icon, tip, panel) = match tool {
        Tool::Presets => (Icon::Presets, "Presets (Shift+P)", None),
        Tool::Edit => (Icon::Sliders, "Edit (E)", Some(RightPanel::Edit)),
        Tool::Crop => (Icon::Crop, "Crop & Rotate (C)", Some(RightPanel::Crop)),
        Tool::Remove => (Icon::Eraser, "Remove (H)", Some(RightPanel::Remove)),
        Tool::Masking => (Icon::Mask, "Masking (M)", Some(RightPanel::Masking)),
        Tool::RedEye => (Icon::Eye, "Red Eye", Some(RightPanel::RedEye)),
        Tool::Portrait => (Icon::Subject, "Portrait (T)", Some(RightPanel::Portrait)),
        Tool::Versions => (Icon::Versions, "Versions (Shift+V)", Some(RightPanel::Versions)),
        Tool::Activity => (Icon::Activity, "History & Activity (Y)", Some(RightPanel::Activity)),
        Tool::Keywords => (Icon::Tag, "Keywords (K)", Some(RightPanel::Keywords)),
        Tool::Info => (Icon::Info, "Info (I)", Some(RightPanel::Info)),
    };
    let on = match panel {
        None => app.ui.presets,
        Some(p) => app.ui.right == p || (p == RightPanel::Edit && app.ui.right == RightPanel::Profiles),
    };
    let enabled = has_photo || tool == Tool::Activity;
    let id = tool.id();
    let resp = icon_button_sense(ui, id, icon, sz, on, enabled, tip, true);
    if resp.clicked() {
        let _ = app.run(&format!("panel.{id}"), json!({}));
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        app.ui.dragging_tool = Some(tool);
    }
    crate::widgets::context_menu(&resp, |ui| crate::panels::tool_strip_order::menu(app, ui, tool));
    resp.rect
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(t.strip_w - 16.0, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.button_border.gamma_multiply(0.7));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use crate::headless::HeadlessView;

    #[test]
    fn info_ends_as_far_above_the_bottom_as_presets_starts_below_the_top() {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        let mut view = HeadlessView::new();
        let mut widgets = vec![];
        for frame in 0..3 {
            let raw = HeadlessView::raw_input(vec2(300.0, 700.0), 1.0, frame as f64 / 60.0, vec![]);
            view.run(raw, |ui| show(&mut app, ui));
            widgets = crate::widgets::take_registry(&view.ctx);
        }
        let rect = |id: &str| widgets.iter().find(|(w, _)| w == id).unwrap_or_else(|| panic!("{id} not registered")).1;
        let (presets, keywords, info) = (rect("icon:presets"), rect("icon:keywords"), rect("icon:info"));
        let (top, bottom) = (presets.top(), 700.0 - info.bottom());
        assert!((top - bottom).abs() < 0.5, "Presets starts {top} below the top, Info ends {bottom} above the bottom");
        assert_eq!(info.top() - keywords.bottom(), GAP, "the bottom buttons are spaced like the top ones");
    }
}
