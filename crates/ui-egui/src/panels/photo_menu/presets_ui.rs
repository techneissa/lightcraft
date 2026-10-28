//! The presets control of Settings ▸ Photo Menu: one small dropdown showing the active preset (a dot
//! after its name when the menu has changes not saved to it), then the Export and Import icon buttons
//! for the backup file. The dropdown's menu switches presets, saves the menu to one, clears one and
//! undoes the last switch.
//!
//! Every entry runs the command of the same name (`photoMenu.switchPreset`, `savePreset`,
//! `clearPreset`, `undoPreset`, `export`, `import`), so the app and an agent do the same.

use serde_json::json;

use super::presets::{self, COUNT};
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

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

/// The dropdown's label: the active preset, with a dot when its menu was changed since.
fn label(app: &LightcraftApp) -> (String, bool) {
    let states = presets::states(app);
    match states.iter().position(|s| s.active) {
        Some(i) => (format!("{} {}{}", crate::i18n::tr("Preset"), i + 1, if states[i].edited { " •" } else { "" }), states[i].edited),
        None => (crate::i18n::tr("Presets").to_string(), false),
    }
}

/// A menu entry `button:{id}` that runs when clicked; true when it was.
fn entry(ui: &mut egui::Ui, id: &str, text: &str, checked: bool) -> bool {
    let r = ui.add(egui::Button::new(text).right_text("    "));
    register(ui, format!("button:{id}"), r.rect);
    if checked {
        super::check_mark(ui, r.rect);
    }
    r.clicked()
}

/// The Save to Preset and Clear Preset submenus.
fn manage(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let name = |n: usize| format!("{} {n}", crate::i18n::tr("Preset"));
    let save = crate::widgets::menu_button(ui, crate::i18n::tr("Save to Preset"), |ui| {
        for n in 1..=COUNT {
            if entry(ui, &format!("photoMenuSavePreset-{n}"), &n.to_string(), false) {
                let done = format!("{} {n}", crate::i18n::tr("Saved the menu to Preset"));
                run(app, ui, "photoMenu.savePreset", json!({"slot": n}), Some(done));
            }
        }
    });
    register(ui, "button:photoMenuSaveMenu", save.response.rect);
    let states = presets::states(app);
    if states.iter().any(|s| s.saved) {
        let clear = crate::widgets::menu_button(ui, crate::i18n::tr("Clear Preset"), |ui| {
            for n in (1..=COUNT).filter(|n| states[n - 1].saved) {
                if entry(ui, &format!("photoMenuClearPreset-{n}"), &name(n), false) {
                    let done = format!("{} {n}", crate::i18n::tr("Cleared Preset"));
                    run(app, ui, "photoMenu.clearPreset", json!({"slot": n}), Some(done));
                }
            }
        });
        register(ui, "button:photoMenuClearMenu", clear.response.rect);
    }
}

/// The presets control, laid out right to left on the heading's line.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    if tray_button(ui, t, "photoMenuImport", Icon::TrayIn, "Import Right-Click Menu…") {
        run(app, ui, "photoMenu.import", json!({}), None);
    }
    if tray_button(ui, t, "photoMenuExport", Icon::TrayOut, "Export Right-Click Menu…") {
        run(app, ui, "photoMenu.export", json!({}), None);
    }
    let (text, edited) = label(app);
    let r = crate::widgets::dropdown(ui, "photoMenuPreset", &text, t.font(12.0), t.text);
    let r = if edited { r.on_hover_text(crate::i18n::tr("Changed since saved")) } else { r };
    egui::Popup::menu(&r).show(|ui| {
        let states = presets::states(app);
        for n in 1..=COUNT {
            let s = states[n - 1];
            let name = format!("{} {n}", crate::i18n::tr("Preset"));
            let text = if s.saved { name } else { format!("{name} ({})", crate::i18n::tr("Empty")) };
            if entry(ui, &format!("photoMenuPreset-{n}"), &text, s.active) {
                let done = format!("{} {n}", crate::i18n::tr("Switched to Preset"));
                run(app, ui, "photoMenu.switchPreset", json!({"slot": n}), Some(done));
            }
        }
        ui.separator();
        manage(app, ui);
        if presets::can_undo(app) {
            ui.separator();
            let r = ui.button(crate::i18n::tr("Undo"));
            register(ui, "button:photoMenuUndoPreset", r.rect);
            if r.on_hover_text(crate::i18n::tr("Bring back the menu you had before switching")).clicked() {
                run(app, ui, "photoMenu.undoPreset", json!({}), None);
            }
        }
    });
}
