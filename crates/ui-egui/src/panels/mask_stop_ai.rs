//! Fork (issues #407 and #452, `fork/mask-models-kept.md`): which AI mask models the open photo's masks use, and each AI
//! mask's Stop AI button in its row of the Masks list.
//!
//! What a mask uses is the session's (`Session::mask_models_by_mask`, the same the tests check); it depends on the
//! chosen selection model and hair edges, which read a few files, so it is worked out again when the photo or its
//! settings change, and at most once a second otherwise.

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
    /// The last Stop AI's message, read out by the list's live region ([`announce`]).
    said: String,
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
    let per_mask = app.session.mask_models_by_mask(id);
    let ai = &mut app.background.ai;
    (ai.key, ai.at, ai.per_mask) = (key, now, per_mask);
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

/// Mask `mask`'s Stop AI button, centred at `x` in its row `r` of photo `photo`: it can be clicked while a model the
/// mask uses is in memory or a request for the mask is under way, and runs `mask.stopAi`. Enabled, it is a filled square;
/// disabled, an outline (so the state is not told by colour alone).
pub fn button(app: &mut LightcraftApp, ui: &mut egui::Ui, r: Rect, x: f32, photo: PhotoId, mask: u32, name: &str) {
    let t = Tokens::get(ui.ctx());
    let uses = mask_models(app, mask, ui.input(|i| i.time));
    let enabled = app.session.mask_ai_stoppable(photo, mask, uses);
    if enabled {
        // (it turns off by itself when the model goes or the request ends: looked at again once a second)
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    let br = Rect::from_center_size(pos2(x, r.center().y), vec2(22.0, 22.0));
    register(ui, format!("button:maskStopAi:{mask}"), br);
    let resp = ui.interact(br, egui::Id::unique(("maskStopAi", mask)), if enabled { Sense::click() } else { Sense::hover() });
    let label = crate::i18n::tr("Stop AI");
    let tip = crate::i18n::tr(if enabled {
        "Stop AI: cancels this mask's AI work and unloads its models. The mask stays as it is."
    } else {
        "Stop AI: no AI model is loaded or working for this mask."
    });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let color = if !enabled {
        t.text_disabled
    } else if resp.hovered() || resp.has_focus() {
        t.text
    } else {
        t.text_dim
    };
    if resp.has_focus() {
        ui.painter().rect_stroke(br, 4.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
    paint(ui.painter(), br.shrink(3.0), if enabled { Icon::Stop } else { Icon::StopIdle }, color);
    if resp.on_hover_text(tip).clicked() && enabled {
        let said = match app.run("mask.stopAi", json!({"id": mask})) {
            Ok(_) => crate::i18n::tr_format!("AI stopped for {name}", name = name),
            Err(e) => e,
        };
        app.toast(ui.ctx(), said.clone());
        app.background.ai.said = said;
    }
}

/// The last Stop AI's message in a polite live region, so a screen reader reads it out (the toast is not read).
pub fn announce(app: &LightcraftApp, ui: &mut egui::Ui) {
    let (_, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 0.0), Sense::hover());
    let text = app.background.ai.said.clone();
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_label(text);
        node.set_live(egui::accesskit::Live::Polite);
    });
}
