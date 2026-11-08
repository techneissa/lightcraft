//! Fork feature (`fork/people-parsing.md`): Settings ▸ Faces ▸ People Parts, the model set that finds people's
//! skin, hair, eyes, lips and clothes. A set that is not downloaded reads "(Download…)": choosing it asks first (its
//! size, where it goes) and downloads only on Download, with a row in the activity stack.

use egui::RichText;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::panels::settings::{heading, hint, row};
use crate::theme::Tokens;
use crate::widgets::text_button;

fn ask_id() -> egui::Id {
    egui::Id::unique("lc-parsing-ask")
}

/// The section, at the end of the Faces tab.
pub fn section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Ok(status) = app.session.execute("parsing.status", &json!({})) else { return };
    if status["modelsFolder"].is_null() {
        return;
    }
    ui.add_space(8.0);
    heading(ui, t, "People Parts");
    hint(ui, t, "Finds each person's skin, hair, eyes, lips and clothes for portrait masks.");
    let sets: Vec<Value> = status["sets"].as_array().cloned().unwrap_or_default();
    let chosen = status["modelSet"].as_str().unwrap_or("").to_string();
    let mut busy = false;
    row(ui, t, "Model Set", |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for set in &sets {
            let id = set["id"].as_str().unwrap_or("");
            let installed = set["installed"].as_bool() == Some(true);
            let download = &set["download"];
            let label = match (installed, download["state"].as_str()) {
                (true, _) => set["label"].as_str().unwrap_or(id).to_string(),
                (false, Some("running")) => {
                    busy = true;
                    let pct = download["bytes"].as_u64().unwrap_or(0) * 100 / download["total"].as_u64().unwrap_or(1).max(1);
                    format!("{} ({pct} %)", set["label"].as_str().unwrap_or(id))
                }
                (false, _) => format!("{} (Download…)", set["label"].as_str().unwrap_or(id)),
            };
            if text_button(ui, &format!("parsing.set-{id}"), &label, installed && id == chosen).clicked() {
                if installed {
                    if let Err(e) = app.run("parsing.modelSet", json!({"set": id})) {
                        app.toast(ui.ctx(), e);
                    }
                } else if download["state"].as_str() != Some("running") {
                    ui.ctx().data_mut(|d| d.insert_temp(ask_id(), id.to_string()));
                }
            }
        }
    });
    for set in &sets {
        let label = set["label"].as_str().unwrap_or("");
        let why = match set["download"]["state"].as_str() {
            Some("failed") => set["download"]["error"].as_str().map(String::from),
            Some("cancelled") => Some(crate::i18n::tr_format!("The {} download stopped. Choose it again to resume.", label)),
            _ => None,
        };
        if let Some(why) = why {
            ui.add(egui::Label::new(RichText::new(crate::i18n::tr(&why)).size(11.0).color(t.caution)).wrap());
        }
        if set["id"] == chosen.as_str() && set["installed"] != true && set["download"]["state"] != "running" {
            hint(ui, t, &crate::i18n::tr_format!("{} is not downloaded, so people's parts are not found yet.", label));
        }
    }
    if let Some(m) = status["message"].as_str() {
        hint(ui, t, m);
    }
    if busy {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    ask(app, ui.ctx(), &sets);
}

/// "Download the … models?": what, how big and where; downloads only on Download, then uses the set.
fn ask(app: &mut LightcraftApp, ctx: &egui::Context, sets: &[Value]) {
    let Some(id) = ctx.data(|d| d.get_temp::<String>(ask_id())) else { return };
    let Some(set) = sets.iter().find(|s| s["id"] == id.as_str()) else {
        ctx.data_mut(|d| d.remove::<String>(ask_id()));
        return;
    };
    let t = Tokens::get(ctx);
    let label = set["label"].as_str().unwrap_or("").to_string();
    let mb = set["missingBytes"].as_u64().unwrap_or(0).div_ceil(1_000_000);
    let folder = app.session.parsing.models_dir().map(|d| d.display().to_string()).unwrap_or_default();
    let modal = egui::Modal::new(egui::Id::unique("lc-parsing-ask-modal")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(RichText::new(crate::i18n::tr_format!("Download the {} Models?", label)).font(t.semibold(15.0)).color(t.text));
        ui.add_space(8.0);
        ui.label(crate::i18n::tr_format!("They are a {} MB download from huggingface.co, kept in the folder LightCraft shares with PhotoCraft:", mb));
        ui.label(RichText::new(&folder).font(t.font(11.5)).color(t.text_dim));
        ui.add_space(10.0);
        let mut choice = None;
        ui.horizontal(|ui| {
            if text_button(ui, "parsingDownload", "Download", true).clicked() {
                choice = Some(true);
            }
            if text_button(ui, "parsingCancel", "Cancel", false).clicked() {
                choice = Some(false);
            }
        });
        choice
    });
    let choice = modal.inner.or(modal.should_close().then_some(false));
    if let Some(download) = choice {
        ctx.data_mut(|d| d.remove::<String>(ask_id()));
        if download {
            // the set is used once its download has finished; a download that stops or fails changes nothing
            if let Err(e) = app.run("parsing.download", json!({"set": id, "confirm": true, "choose": true})) {
                app.toast(ctx, e);
            }
        }
    }
}
