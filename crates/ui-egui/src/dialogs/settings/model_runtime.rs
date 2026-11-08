//! Fork feature (`fork/coreml.md`, issue #105): Settings ▸ Performance ▸ AI Models, the engine each model with a
//! Core ML version runs on (Automatic, Core ML, Candle). Changes go through `models.runtime`.

use lightcraft_engine::model_runtime::{MODELS, Runtime};

use super::widgets::{choices, heading, row};
use super::{LightcraftApp, Tokens, json};

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
    }
}
