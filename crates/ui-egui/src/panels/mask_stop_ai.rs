//! Fork (issue #407, `fork/mask-models-kept.md`): which AI mask models the open photo's masks use, for the Masking
//! panel's hold (`background::tick`), and each AI mask's Stop AI button in its row of the Masks list.
//!
//! What a mask uses depends on its parts and on the chosen selection model and hair edges, which read a few files, so it
//! is worked out again when the photo or its settings change, and at most once a second otherwise.

use egui::{Rect, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::segment::MaskModels;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// How long the models found for the photo are trusted while nothing changes (s).
const REFRESH: f64 = 1.0;

/// The models each mask of the open photo uses, worked out at `at` for the photo and its settings in `key`.
#[derive(Default)]
pub struct Uses {
    key: Option<(PhotoId, usize)>,
    at: f64,
    per_mask: Vec<(u32, MaskModels)>,
}

fn refresh(app: &mut LightcraftApp, now: f64) {
    let Some(id) = app.session.active() else {
        app.background.ai = Uses::default();
        return;
    };
    let Some(d) = app.session.develop_of(id) else { return };
    let key = Some((id, std::sync::Arc::as_ptr(&d) as usize));
    if app.background.ai.key == key && now - app.background.ai.at < REFRESH {
        return;
    }
    let per_mask = d.masks.iter().map(|m| (m.id, app.session.mask_models_of_mask(m))).collect();
    app.background.ai = Uses { key, at: now, per_mask };
}

/// The models the open photo's masks use (what the Masking panel holds).
pub fn photo_models(app: &mut LightcraftApp, now: f64) -> MaskModels {
    refresh(app, now);
    app.background.ai.per_mask.iter().fold(MaskModels::NONE, |a, (_, m)| a.either(*m))
}

/// The models mask `mask` of the open photo uses.
fn mask_models(app: &mut LightcraftApp, mask: u32, now: f64) -> MaskModels {
    refresh(app, now);
    app.background.ai.per_mask.iter().find(|(id, _)| *id == mask).map_or(MaskModels::NONE, |(_, m)| *m)
}

/// Whether mask `mask` is made by an AI model (its row then has a Stop AI button).
pub fn is_ai(app: &mut LightcraftApp, ui: &egui::Ui, mask: u32) -> bool {
    mask_models(app, mask, ui.input(|i| i.time)).any()
}

/// Mask `mask`'s Stop AI button, centred at `x` in its row `r`: it can be clicked while a model the mask uses is in
/// memory or working, and runs `mask.stopAi`.
pub fn button(app: &mut LightcraftApp, ui: &mut egui::Ui, r: Rect, x: f32, mask: u32, name: &str) {
    let t = Tokens::get(ui.ctx());
    let uses = mask_models(app, mask, ui.input(|i| i.time));
    let enabled = uses.both(app.session.mask_models_in_use()).any();
    if enabled {
        // (the button turns off by itself when the model goes)
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    let br = Rect::from_center_size(pos2(x, r.center().y), vec2(22.0, 22.0));
    register(ui, format!("button:maskStopAi:{mask}"), br);
    let resp = ui.interact(br, egui::Id::unique(("maskStopAi", mask)), if enabled { Sense::click() } else { Sense::hover() });
    let label = crate::i18n::tr("Stop AI");
    let tip = crate::i18n::tr(if enabled {
        "Stop AI: cancels this mask's AI work and unloads its models. The mask stays as it is."
    } else {
        "Stop AI: no AI model is loaded for this mask."
    });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    ui.ctx().accesskit_node_builder(resp.id, |node| node.set_description(tip.to_string()));
    let color = if !enabled {
        t.text_disabled
    } else if resp.hovered() || resp.has_focus() {
        t.text
    } else {
        t.text_dim
    };
    paint(ui.painter(), br.shrink(3.0), Icon::Stop, color);
    if resp.on_hover_text(tip).clicked() && enabled {
        app.ui.status = match app.run("mask.stopAi", json!({"id": mask})) {
            Ok(_) => crate::i18n::tr("AI stopped for {name}").replace("{name}", name),
            Err(e) => e,
        };
    }
}
