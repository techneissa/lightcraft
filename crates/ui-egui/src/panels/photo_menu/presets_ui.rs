//! The Presets rows of Settings ▸ Photo Menu: three slots to switch between, three buttons to save
//! the menu into one, and a line with Undo Switch, two icon buttons for the backup file (Export and Import) and the status.
//!
//! Every button runs the command of the same name (`photoMenu.switchPreset`, `photoMenu.savePreset`,
//! `photoMenu.undoPreset`, `photoMenu.export`, `photoMenu.import`), so the app and an agent do the same.

use serde_json::json;

use super::presets::{self, COUNT};
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{register, segmented};

/// The row's status line.
fn status(app: &LightcraftApp) -> String {
    let states = presets::states(app);
    match states.iter().position(|s| s.active) {
        Some(i) if states[i].edited => {
            format!("{} {} {}", crate::i18n::tr("Preset"), i + 1, crate::i18n::tr("is active, with changes not saved to it."))
        }
        Some(i) => format!("{} {} {}", crate::i18n::tr("Preset"), i + 1, crate::i18n::tr("is active.")),
        None => crate::i18n::tr("No preset is active. Save the menu to a slot to keep it.").to_string(),
    }
}

/// Run `command` and say how it went.
fn run(app: &mut LightcraftApp, ui: &egui::Ui, command: &str, params: serde_json::Value, done: Option<String>) {
    match app.run(command, params) {
        Ok(_) => {
            if let Some(done) = done {
                app.toast(ui.ctx(), done);
            }
        }
        Err(e) => app.toast_error(ui.ctx(), e),
    }
}

/// A small icon button (`button:{id}`) named and explained by `tip`; true when clicked.
fn tray_button(ui: &mut egui::Ui, t: &Tokens, id: &str, icon: Icon, tip: &str) -> bool {
    let tip = crate::i18n::tr(tip);
    let (rect, r) = ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::click());
    register(ui, format!("button:{id}"), rect);
    crate::widgets::announce(&r, tip.to_string());
    if r.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }
    paint(ui.painter(), egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0)), icon, if r.hovered() { t.text } else { t.icon });
    r.on_hover_text(tip).clicked()
}

/// The Presets row.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let states = presets::states(app);
    let switch: Vec<String> = (1..=COUNT)
        .map(|n| {
            let name = format!("{} {n}", crate::i18n::tr("Preset"));
            if states[n - 1].saved { name } else { format!("{name} ({})", crate::i18n::tr("Empty")) }
        })
        .collect();
    let items: Vec<(&str, &str)> = switch.iter().map(String::as_str).zip(["1", "2", "3"]).collect();
    if let Some(i) = segmented(ui, "photoMenuPreset", &items, states.iter().position(|s| s.active), COUNT) {
        let done = format!("{} {} {}", crate::i18n::tr("Switched to Preset"), i + 1, crate::i18n::tr("(Undo Switch brings the old menu back)"));
        run(app, ui, "photoMenu.switchPreset", json!({"slot": i + 1}), Some(done));
    }
    ui.add_space(4.0);
    let save: Vec<String> = (1..=COUNT).map(|n| format!("{} {} {n}", crate::i18n::tr("Save to"), crate::i18n::tr("Preset"))).collect();
    let items: Vec<(&str, &str)> = save.iter().map(String::as_str).zip(["1", "2", "3"]).collect();
    if let Some(i) = segmented(ui, "photoMenuSavePreset", &items, None, COUNT) {
        let done = format!("{} {}", crate::i18n::tr("Saved the menu to Preset"), i + 1);
        run(app, ui, "photoMenu.savePreset", json!({"slot": i + 1}), Some(done));
    }
    ui.add_space(4.0);
    let text = status(app);
    ui.horizontal(|ui| {
        let r = ui.add_enabled(presets::can_undo(app), egui::Button::new(crate::i18n::tr("Undo Switch")));
        register(ui, "button:photoMenuUndoPreset", r.rect);
        if r.clicked() {
            run(app, ui, "photoMenu.undoPreset", json!({}), None);
        }
        if tray_button(ui, t, "photoMenuExport", Icon::TrayOut, "Export Right-Click Menu…") {
            run(app, ui, "photoMenu.export", json!({}), None);
        }
        if tray_button(ui, t, "photoMenuImport", Icon::TrayIn, "Import Right-Click Menu…") {
            run(app, ui, "photoMenu.import", json!({}), None);
        }
        ui.label(egui::RichText::new(text).size(11.5).color(t.text_dim));
    });
    ui.add_space(6.0);
}
