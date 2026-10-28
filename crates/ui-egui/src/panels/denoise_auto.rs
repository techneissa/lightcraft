//! Auto AI Denoise in the interface: tells the engine (`denoise.autoOpen`) when a photo becomes the open one in the
//! Detail view, and the Settings ▸ AI Denoise controls (a switch and the ISO it starts at). What it does, and why a photo
//! is handled only once, is the engine's (`crates/engine/src/denoise/auto.rs`).

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::settings::{check, heading, hint};
use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::state::ViewMode;
use crate::text_field::TextField;
use crate::theme::Tokens;

/// How long the note about a missing model stays.
const NOTE_SECONDS: f64 = 9.0;

/// What the interface keeps between frames.
#[derive(Default)]
pub struct State {
    /// The photo last told to the engine as opened; `None` while no photo is open in Detail.
    seen: Option<PhotoId>,
    /// The missing-model note was shown (once a session, not on every photo).
    told_no_model: bool,
}

/// Every frame: when the open photo changed, ask the engine whether it gets AI Denoise.
pub fn pump(app: &mut LightcraftApp, ctx: &egui::Context) {
    let open = if app.ui.view == ViewMode::Detail { app.session.active() } else { None };
    if open == app.caches.denoise.auto.seen {
        return;
    }
    let Some(id) = open else {
        app.caches.denoise.auto.seen = None;
        return;
    };
    // an edit under way (a slider drag on another photo) finishes first
    if app.session.interaction.is_some() {
        return;
    }
    app.caches.denoise.auto.seen = Some(id);
    let Ok(v) = app.session.execute("denoise.autoOpen", &json!({"id": id.0})) else { return };
    match v["reason"].as_str() {
        Some("applied") => {
            let name = app.session.catalog.photo(id).map(|p| p.file_name.clone()).unwrap_or_default();
            app.ui.status = tr_format!("Auto AI Denoise switched on for {name}", name = name);
            ctx.request_repaint();
        }
        Some("noModel") if !app.caches.denoise.auto.told_no_model => {
            app.caches.denoise.auto.told_no_model = true;
            let text = tr("Auto AI Denoise needs a denoise model: install one in Settings, AI Denoise");
            app.ui.status = text.to_string();
            app.toast_for(ctx, text, NOTE_SECONDS);
        }
        Some("recordUnreadable") => app.ui.status = tr("Auto AI Denoise is off: the library's auto-denoise.json is damaged or too large").to_string(),
        _ => {}
    }
}

/// The controls in Settings ▸ AI Denoise. `list` is `denoise.models.list`.
pub fn section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, list: &Value) {
    heading(ui, t, "Auto AI Denoise");
    let mut on = list["autoOpen"].as_bool().unwrap_or(false);
    if check(ui, "settings.denoiseAutoOpen", &mut on, "Denoise High-ISO Photos When Opened") {
        let _ = app.run("denoise.settings", json!({"autoOpen": on}));
        app.caches.denoise.epoch += 1;
    }
    let current = list["autoIso"].as_u64().unwrap_or(6400);
    let draft_id = ui.scope_id().with("denoise-auto-iso-draft");
    let mut text = ui.data(|d| d.get_temp::<String>(draft_id)).unwrap_or_else(|| current.to_string());
    let mut refused = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tr("ISO Threshold")).color(t.text_label));
        let field =
            TextField::singleline("field:denoiseAutoIso", &mut text).width(70.0).horizontal_align(egui::Align::RIGHT).select_on_focus(true).show(ui);
        ui.label(egui::RichText::new(tr("and above")).color(t.text_dim));
        if field.ending.is_some() || !field.editing {
            ui.data_mut(|d| d.remove::<String>(draft_id));
            if field.committed() {
                match text.trim().parse::<u32>() {
                    Ok(iso) if lightcraft_engine::denoise::AUTO_ISO_RANGE.contains(&iso) => {
                        if u64::from(iso) != current {
                            let _ = app.run("denoise.settings", json!({"autoIso": iso}));
                            app.caches.denoise.epoch += 1;
                        }
                    }
                    _ => refused = true,
                }
            }
        } else {
            ui.data_mut(|d| d.insert_temp(draft_id, text.clone()));
        }
    });
    if refused {
        let range = lightcraft_engine::denoise::AUTO_ISO_RANGE;
        hint(ui, t, &tr_format!("Enter an ISO from {low} to {high}", low = range.start(), high = range.end()));
    }
    hint(
        ui,
        t,
        "When a raw photo at this ISO or above is opened in Detail, AI Denoise is switched on for it, once, as an edit you can undo. A photo you undo or switch off is left alone. New photos can get it at import in Settings, Import. Needs a denoise model.",
    );
}
