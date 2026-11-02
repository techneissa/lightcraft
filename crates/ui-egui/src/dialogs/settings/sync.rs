//! Settings ▸ Sync: all settings in one file (Export Settings…, Import Settings…) or in a folder
//! (Choose…, Save to Folder, Load from Folder, Keep in Sync). The work is `settings_sync`'s; each
//! button runs its command (`settings.export`, `settings.import`, `settings.syncChoose`,
//! `settings.syncSave`, `settings.syncLoad`).

use super::{LightcraftApp, Tokens};
#[cfg(not(target_arch = "wasm32"))]
use super::{
    RichText, check, heading, json, register,
    widgets::{button_width, path_label, row},
};

/// A button of the page: it runs `command`, and a refusal is a toast.
#[cfg(not(target_arch = "wasm32"))]
fn button(app: &mut LightcraftApp, ui: &mut egui::Ui, enabled: bool, id: &str, label: &str, command: &str, params: serde_json::Value) {
    let r = ui.add_enabled(enabled, egui::Button::new(crate::i18n::tr(label)));
    register(ui, format!("button:{id}"), r.rect);
    if r.clicked()
        && let Err(e) = app.run(command, params)
    {
        app.toast_error(ui.ctx(), e);
    }
}

/// The folder the other ArtCraft apps use, for a Mac with none chosen yet; the file is read at most every two seconds.
#[cfg(not(target_arch = "wasm32"))]
fn offered(app: &LightcraftApp, ctx: &egui::Context) -> Option<String> {
    let id = egui::Id::unique("settings-sync-offer");
    let now = ctx.input(|i| i.time);
    if let Some((at, found)) = ctx.data(|d| d.get_temp::<(f64, Option<String>)>(id))
        && now - at < 2.0
    {
        return found;
    }
    let found = crate::settings_sync::run::offered_folder(app);
    ctx.data_mut(|d| d.insert_temp(id, (now, found.clone())));
    found
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn sync_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use crate::settings_sync::run::tilde;
    let chosen = !app.ui.sync.folder.is_empty();
    let offer = if chosen { None } else { offered(app, ui.ctx()) };
    heading(ui, t, "Sync Folder");
    row(ui, t, crate::i18n::tr("Folder"), |ui| {
        let (choose, use_it) = (crate::i18n::tr("Choose…"), crate::i18n::tr("Use This Folder"));
        let after = button_width(ui, choose) + offer.as_ref().map_or(0.0, |_| button_width(ui, use_it));
        let font = egui::TextStyle::Body.resolve(ui.style());
        match (&app.ui.sync.folder, &offer) {
            (folder, _) if chosen => path_label(ui, "settingsSyncFolder", &tilde(folder), font, t.text, after),
            (_, Some(o)) => path_label(ui, "settingsSyncFolder", &tilde(o), font, t.text_dim, after),
            _ => {
                ui.label(RichText::new(crate::i18n::tr("None chosen")).color(t.text_dim));
            }
        }
        if let Some(o) = &offer {
            button(app, ui, true, "settingsSyncUse", "Use This Folder", "settings.syncChoose", json!({"path": o}));
        }
        button(app, ui, true, "settingsSyncChoose", "Choose…", "settings.syncChoose", json!({}));
    });
    row(ui, t, "", |ui| {
        button(app, ui, chosen, "settingsSyncSave", "Save to Folder", "settings.syncSave", json!({}));
        button(app, ui, chosen, "settingsSyncLoad", "Load from Folder", "settings.syncLoad", json!({}));
    });
    row(ui, t, "", |ui| {
        let mut keep = app.ui.sync.keep;
        ui.add_enabled_ui(chosen, |ui| {
            if check(ui, "settingsSyncKeep", &mut keep, "Keep in Sync") {
                app.ui.sync.keep = keep;
            }
        });
    });
    heading(ui, t, "Settings File");
    row(ui, t, "", |ui| {
        button(app, ui, true, "settingsSyncExport", "Export Settings…", "settings.export", json!({}));
        button(app, ui, true, "settingsSyncImport", "Import Settings…", "settings.import", json!({}));
    });
    heading(ui, t, "Included");
    row(ui, t, "", |ui| {
        let mut presets = app.ui.sync.presets;
        if check(ui, "settingsSyncPresets", &mut presets, "User Presets") {
            app.ui.sync.presets = presets;
        }
    });
}

#[cfg(target_arch = "wasm32")]
pub(super) fn sync_tab(_app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    super::widgets::hint(ui, t, "Settings sync is for the desktop app.");
}
