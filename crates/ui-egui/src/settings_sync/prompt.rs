//! The load prompt of Import Settings… and Load from Folder: what the file would change, and when
//! and on which Mac it was saved, shown before anything changes.

use serde_json::{Value, json};

use super::Prompt;
use crate::LightcraftApp;
use crate::theme::Tokens;

/// The names of skipped settings in the prompt's calm line.
const NAMED: usize = 5;

/// "3 settings from a newer LightCraft were skipped: a, b, c, d, e, and 5 more"; `None` when
/// nothing was skipped. The full list is [`unknown_list`].
pub fn unknown_line(unknown: &[String], total: usize) -> Option<String> {
    if total == 0 {
        return None;
    }
    let mut names = unknown.iter().take(NAMED).cloned().collect::<Vec<_>>().join(", ");
    if total > NAMED {
        names.push_str(&crate::i18n::tr_format!(", and {n} more", n = total - NAMED));
    }
    Some(if total == 1 {
        crate::i18n::tr_format!("1 setting from a newer LightCraft was skipped: {names}", names = names)
    } else {
        crate::i18n::tr_format!("{n} settings from a newer LightCraft were skipped: {names}", n = total, names = names)
    })
}

/// Every skipped setting's name, for the line's hover.
pub fn unknown_list(unknown: &[String], total: usize) -> String {
    let mut list = unknown.join("\n");
    if total > unknown.len() {
        list.push_str(&crate::i18n::tr_format!("\n…and {n} more", n = total - unknown.len()));
    }
    list
}

/// What OK replaces, in a sentence.
fn replaces(prompt: &Prompt) -> String {
    let presets = prompt.plan.presets.len();
    if presets == 0 {
        crate::i18n::tr("This replaces your appearance, keyboard shortcuts, arrangements and tool defaults.").to_string()
    } else if presets == 1 {
        crate::i18n::tr("This replaces your appearance, keyboard shortcuts, arrangements and tool defaults, and adds 1 preset.").to_string()
    } else {
        crate::i18n::tr_format!(
            "This replaces your appearance, keyboard shortcuts, arrangements and tool defaults, and adds {n} presets.",
            n = presets
        )
    }
}

/// The dialog's body.
pub fn prompt_body(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some(prompt) = app.sync_settings.prompt.as_ref() else {
        ui.label(crate::i18n::tr("There is nothing to load."));
        return;
    };
    ui.set_max_width(460.0);
    ui.add(egui::Label::new(crate::i18n::tr_format!("Load the settings in {name}?", name = prompt.name)).wrap());
    if let Some(line) = &prompt.saved_line {
        ui.add_space(2.0);
        ui.add(egui::Label::new(egui::RichText::new(line).color(t.text_dim)).wrap());
    }
    ui.add_space(4.0);
    ui.add(egui::Label::new(egui::RichText::new(replaces(prompt)).color(t.text_dim)).wrap());
    if let Some(line) = unknown_line(&prompt.plan.unknown, prompt.plan.unknown_total) {
        ui.add_space(4.0);
        let r = ui.add(egui::Label::new(egui::RichText::new(line).color(t.text_dim)).wrap());
        crate::widgets::register(ui, "text:settingsSyncSkipped", r.rect);
        r.on_hover_text(unknown_list(&prompt.plan.unknown, prompt.plan.unknown_total));
    }
    for note in prompt.plan.notes.iter().take(3) {
        ui.add(egui::Label::new(egui::RichText::new(note).size(11.5).color(t.caution)).wrap());
    }
}

/// OK on the prompt: put the file's settings in place, and note that this Mac has a file from the folder.
pub fn confirm(app: &mut LightcraftApp) -> Result<Value, String> {
    let Some(prompt) = app.sync_settings.prompt.take() else { return Err(crate::i18n::tr("There is nothing to load.").to_string()) };
    let report = prompt.plan.report(true);
    let name = prompt.name.clone();
    let at = prompt.saved_at.clone();
    super::apply(app, prompt.plan)?;
    if let Some(at) = at {
        app.ui.sync.last = at;
    }
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    app.toast(&ctx, crate::i18n::tr_format!("Loaded the settings in {name}", name = name));
    Ok(json!({"applied": true, "report": report}))
}
