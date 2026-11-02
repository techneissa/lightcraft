//! Fork feature (fork/birefnet-masks.md, issue #48): the model dropdown at the right of the Masking heading. It sets
//! the model for new Subject and Background parts only; each part keeps the model that made it. With one model
//! installed there is nothing to choose, and a quiet "Get BiRefNet" asks (with the size) before it downloads.

use egui::{Align, Layout, Rect, UiBuilder, pos2};
use lightcraft_engine::segment::Segmenter;
use lightcraft_engine::segment::birefnet::{SelectionModel, label_of};
use serde_json::json;

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// The heading row's height (`right::header`).
const HEADER_H: f32 = 46.0;
/// Room for the control, from the panel's right margin.
const SLOT_W: f32 = 176.0;

/// Show the control on the heading row just drawn by `header`.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !Segmenter::AVAILABLE {
        return;
    }
    let seg = &app.session.segmenter;
    let models = seg.available_models();
    let can_offer = models.len() == 1 && seg.has_model() && !seg.birefnet_installed() && seg.birefnet_dir().is_some();
    if models.len() < 2 && !can_offer {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let top = ui.cursor().top() - HEADER_H;
    let right = ui.max_rect().right() - 22.0;
    let slot = Rect::from_min_max(pos2(right - SLOT_W, top), pos2(right, top + HEADER_H));
    let mut child = ui.new_child(UiBuilder::new().max_rect(slot).layout(Layout::right_to_left(Align::Center)));
    let ui = &mut child;
    if models.len() >= 2 {
        let current = app.session.segmenter.selection();
        let r = crate::widgets::dropdown_within(ui, "selectionModel", crate::i18n::tr(current.label()), t.font(12.0), t.text_label, SLOT_W);
        let r = r.on_hover_text(crate::i18n::tr("Model for new Subject and Background masks"));
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(190.0);
            for m in &models {
                let resp = ui.selectable_label(*m == current, crate::i18n::tr(m.label()));
                register(ui, format!("selectionModel:{}", m.id()), resp.rect);
                if resp.clicked() {
                    let _ = app.run("segment.selectionModel", json!({"model": m.id()}));
                    ui.close();
                }
            }
        });
    } else {
        let r = crate::widgets::dropdown_within(ui, "getBiRefNet", crate::i18n::tr("Get BiRefNet"), t.font(12.0), t.text_dim, SLOT_W);
        egui::Popup::menu(&r).show(|ui| {
            ui.set_max_width(240.0);
            let mb = Segmenter::birefnet_bytes() as f64 / 1e6;
            ui.label(egui::RichText::new(format!("{mb:.0} MB, MIT licence.")).color(t.text_dim));
            let running = app.session.segmenter.birefnet_download_status().running;
            let go = ui.add_enabled(!running, egui::Button::new(crate::i18n::tr("Download BiRefNet")));
            register(ui, "button:getBiRefNetDownload", go.rect);
            if go.clicked() {
                let _ = app.run("birefnet.model.download", json!({"confirm": true}));
                ui.close();
            }
        });
    }
}

/// What a part's tooltip says about the model that made its stored selection: `None` for a part with no selection
/// (the classical shapes, the brush…).
pub fn part_tip(shape: &lightcraft_develop::MaskShape) -> Option<String> {
    use lightcraft_develop::MaskShape as S;
    let seg = match shape {
        S::Object { seg, .. } | S::Prompt { seg, .. } | S::Subject { seg, .. } | S::Sky { seg, .. } | S::Background { seg, .. } => seg.as_ref()?,
        _ => return None,
    };
    let label = label_of(seg.model.as_deref());
    // (a model this build knows is translated; an id it does not know is shown as it is)
    let label = SelectionModel::parse(seg.model.as_deref().unwrap_or("sam3")).map_or(label, |m| crate::i18n::tr(m.label()).to_string());
    Some(format!("{} {label}", crate::i18n::tr("Made with")))
}
