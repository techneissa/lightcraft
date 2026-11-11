//! Fork feature (`fork/coreml.md`, issue #105): Settings ▸ Performance ▸ AI Models, the engine each model with a
//! Core ML version runs on (Automatic, Core ML, Candle), and the asked-for download of its Core ML file. Changes go
//! through `models.runtime` and `models.downloadCoreml`.

use lightcraft_engine::lama_fill::Download;
use lightcraft_engine::model_runtime::{MODELS, Runtime};

use super::widgets::{choices, heading, row};
use super::{LABEL_W, LightcraftApp, RichText, Tokens, json, register};

pub(super) fn section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, "AI Models");
    // Core ML exists only on the Mac; elsewhere Automatic is candle.
    let options: Vec<(Runtime, &str)> =
        Runtime::ALL.into_iter().filter(|r| cfg!(target_os = "macos") || *r != Runtime::CoreMl).map(|r| (r, r.label())).collect();
    for (model, feature) in MODELS {
        let mut value = app.session.model_runtimes.get(model);
        row(ui, t, crate::i18n::tr(feature), |ui| {
            if choices(ui, &format!("settingsRuntime-{model}"), &options, &mut value)
                && let Err(e) = app.run("models.runtime", json!({"model": model, "runtime": value.id()}))
            {
                app.toast(ui.ctx(), e);
            }
        });
        let installed = match *model {
            lightcraft_learn::ENCODER_ID => app.session.learn.coreml_installed(),
            lightcraft_engine::model_runtime::BIREFNET => app.session.segmenter.birefnet_coreml_installed(),
            lightcraft_engine::hair_edges::MODEL_DIR => app.session.hair_edges.coreml_installed(),
            lightcraft_engine::model_runtime::SAM3 => app.session.segmenter.sam3_coreml_installed(),
            _ => app.session.parsing.coreml_installed(model),
        };
        if cfg!(target_os = "macos") && !installed {
            download_row(app, ui, t, model);
        }
    }
}

/// "Download Core ML Model (43 MB)", then the question, then its progress or why it failed.
fn download_row(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, model: &str) {
    let Some(mb) = published_mb(model) else { return };
    let asking = egui::Id::unique(("coreml-download-ask", model));
    let state = app.session.model_runtimes.download_state(model);
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W);
        match state {
            Download::Running { bytes, total } => {
                let pct = bytes.saturating_mul(100).checked_div(total).unwrap_or(0);
                ui.label(RichText::new(crate::i18n::tr_format!("Downloading the Core ML model… {}%", pct.min(100))).size(11.0).color(t.text_dim));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            _ if ui.data(|d| d.get_temp::<bool>(asking)).unwrap_or(false) => {
                ui.label(crate::i18n::tr_format!("Download the Core ML model ({} MB) from github.com?", mb));
                let yes = ui.button(crate::i18n::tr("Download"));
                register(ui, format!("button:settingsCoremlDownloadYes-{model}"), yes.rect);
                let no = ui.button(crate::i18n::tr("Cancel"));
                if yes.clicked() {
                    if let Err(e) = app.run("models.downloadCoreml", json!({"model": model, "confirm": true})) {
                        app.toast(ui.ctx(), e);
                    }
                    ui.data_mut(|d| d.remove::<bool>(asking));
                } else if no.clicked() {
                    ui.data_mut(|d| d.remove::<bool>(asking));
                }
            }
            other => {
                let r = ui.button(crate::i18n::tr_format!("Download Core ML Model ({} MB)…", mb));
                register(ui, format!("button:settingsCoremlDownload-{model}"), r.rect);
                if r.clicked() {
                    ui.data_mut(|d| d.insert_temp(asking, true));
                }
                if let Download::Failed(e) = other {
                    ui.add(egui::Label::new(RichText::new(crate::i18n::tr("Didn't download")).size(11.0).color(t.text_dim)).truncate())
                        .on_hover_text(e);
                }
            }
        }
    });
}

fn published_mb(model: &str) -> Option<u64> {
    #[cfg(not(target_arch = "wasm32"))]
    return lightcraft_engine::model_runtime::release(model).map(|r| (r.bytes + 500_000) / 1_000_000);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = model;
        None
    }
}
