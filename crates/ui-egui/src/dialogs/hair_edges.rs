//! Fork feature (`fork/hair-edges.md`, issue #103): Settings ▸ Faces ▸ Hair Edges, the edge of Entire Person /
//! Background and Subject masks: Current (the default), BiRefNet Matting, or Apple Vision on a Mac that runs it. Without the model, BiRefNet Matting reads
//! "(Download…)": choosing it asks first (its size, where it goes), downloads only on Download, with a row in the
//! activity stack, and is chosen once the download has finished.

use egui::RichText;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::panels::settings::{heading, hint, row};
use crate::theme::Tokens;
use crate::widgets::text_button;

/// The kind whose BiRefNet Matting the user asked to download (set while the question or the download is open).
fn ask_id() -> egui::Id {
    egui::Id::unique("lc-hair-edges-ask")
}

/// The section, at the end of the Faces tab.
pub fn section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Ok(status) = app.session.execute("maskEdges.status", &json!({})) else { return };
    if status["available"] != true || status["dir"].is_null() {
        return;
    }
    let installed = status["installed"] == true;
    let download = &status["download"];
    let running = download["running"] == true;
    let vision = status["visionAvailable"] == true;
    ui.add_space(8.0);
    heading(ui, t, "Hair Edges");
    hint(ui, t, "BiRefNet Matting keeps flyaway hair in people and subject masks.");
    for kind in status["kinds"].as_array().cloned().unwrap_or_default() {
        let id = kind["kind"].as_str().unwrap_or("").to_string();
        let chosen = kind["chosen"].as_str().unwrap_or("current").to_string();
        row(ui, t, kind["label"].as_str().unwrap_or(""), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if text_button(ui, &format!("maskEdges.{id}-current"), "Current", chosen == "current").clicked()
                && let Err(e) = app.run("maskEdges.edge", json!({"kind": id, "edge": "current"}))
            {
                app.toast(ui.ctx(), e);
            }
            let label = match (installed, running) {
                (true, _) => "BiRefNet Matting".to_string(),
                (false, true) => {
                    let pct = download["done"].as_u64().unwrap_or(0) * 100 / download["total"].as_u64().unwrap_or(1).max(1);
                    format!("BiRefNet Matting ({pct} %)")
                }
                (false, false) => "BiRefNet Matting (Download…)".to_string(),
            };
            if text_button(ui, &format!("maskEdges.{id}-matting"), &label, installed && chosen == "birefnet-matting").clicked() {
                if installed {
                    if let Err(e) = app.run("maskEdges.edge", json!({"kind": id, "edge": "birefnet-matting"})) {
                        app.toast(ui.ctx(), e);
                    }
                } else if !running {
                    ui.ctx().data_mut(|d| d.insert_temp(ask_id(), id.clone()));
                }
            }
            // (Apple Vision's own masks, no download: `fork/apple-vision-masks.md`)
            if vision
                && text_button(ui, &format!("maskEdges.{id}-vision"), "Apple Vision", chosen == "apple-vision").clicked()
                && let Err(e) = app.run("maskEdges.edge", json!({"kind": id, "edge": "apple-vision"}))
            {
                app.toast(ui.ctx(), e);
            }
        });
    }
    if let Some(e) = download["error"].as_str().filter(|_| !running) {
        let why = if download["cancelled"] == true {
            "The BiRefNet Matting download stopped. Choose it again to resume.".to_string()
        } else {
            e.to_string()
        };
        ui.add(egui::Label::new(RichText::new(crate::i18n::tr(&why)).size(11.0).color(t.caution)).wrap());
    }
    if running {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    ask(app, ui.ctx(), &status);
}

/// "Download BiRefNet Matting?": how big and where; downloads only on Download.
fn ask(app: &mut LightcraftApp, ctx: &egui::Context, status: &Value) {
    let Some(kind) = ctx.data(|d| d.get_temp::<String>(ask_id())) else { return };
    let t = Tokens::get(ctx);
    let mb = status["sizeBytes"].as_u64().unwrap_or(0).div_ceil(1_000_000);
    let folder = status["dir"].as_str().unwrap_or("").to_string();
    let modal = egui::Modal::new(egui::Id::unique("lc-hair-edges-ask-modal")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(RichText::new(crate::i18n::tr("Download BiRefNet Matting?")).font(t.semibold(15.0)).color(t.text));
        ui.add_space(8.0);
        ui.label(crate::i18n::tr_format!("It is a {} MB download from huggingface.co, kept in the folder LightCraft shares with PhotoCraft:", mb));
        ui.label(RichText::new(&folder).font(t.font(11.5)).color(t.text_dim));
        ui.add_space(10.0);
        let mut choice = None;
        ui.horizontal(|ui| {
            if text_button(ui, "maskEdgesDownload", "Download", true).clicked() {
                choice = Some(true);
            }
            if text_button(ui, "maskEdgesCancel", "Cancel", false).clicked() {
                choice = Some(false);
            }
        });
        choice
    });
    let choice = modal.inner.or(modal.should_close().then_some(false));
    if let Some(download) = choice {
        ctx.data_mut(|d| d.remove::<String>(ask_id()));
        if download {
            // (the engine chooses it for `kind` once the download has finished, whether or not Settings is open)
            if let Err(e) = app.run("maskEdges.download", json!({"confirm": true, "kind": kind})) {
                app.toast(ctx, e);
            }
        }
    }
}
