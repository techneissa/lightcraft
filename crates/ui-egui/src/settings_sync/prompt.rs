//! The load prompt of Import Settings… and Load from Folder: what the file would change, and when
//! and on which Mac it was saved, shown before anything changes; and the Replace question of Save to
//! Folder.

use serde_json::{Value, json};

use super::Plan;
use super::plan::AREAS;
use crate::LightcraftApp;
use crate::theme::Tokens;

/// The names of skipped settings in the prompt's calm line.
const NAMED: usize = 5;

/// "3 settings from a newer LightCraft were skipped: a, b, c, d, e, and 5 more"; `None` when
/// nothing was skipped. A file that isn't from a newer LightCraft says "3 settings were skipped".
/// The full list is [`unknown_list`].
pub fn unknown_line(unknown: &[String], total: usize, newer: bool) -> Option<String> {
    if total == 0 {
        return None;
    }
    let mut names = unknown.iter().take(NAMED).cloned().collect::<Vec<_>>().join(", ");
    if total > NAMED {
        names.push_str(&crate::i18n::tr_format!(", and {n} more", n = total - NAMED));
    }
    Some(match (total, newer) {
        (1, true) => crate::i18n::tr_format!("1 setting from a newer LightCraft was skipped: {names}", names = names),
        (1, false) => crate::i18n::tr_format!("1 setting was skipped: {names}", names = names),
        (n, true) => crate::i18n::tr_format!("{n} settings from a newer LightCraft were skipped: {names}", n = n, names = names),
        (n, false) => crate::i18n::tr_format!("{n} settings were skipped: {names}", n = n, names = names),
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

/// "This replaces your appearance, keyboard shortcuts and arrangements." with only the parts the
/// file has; `None` when it has none.
pub fn replaces_line(plan: &Plan) -> Option<String> {
    let words: Vec<String> = AREAS.iter().filter(|(id, _)| plan.areas.contains(id)).map(|(_, words)| crate::i18n::tr(words).to_string()).collect();
    let (last, rest) = words.split_last()?;
    let list = if rest.is_empty() { last.clone() } else { crate::i18n::tr_format!("{rest} and {last}", rest = rest.join(", "), last = last) };
    Some(crate::i18n::tr_format!("This replaces your {list}.", list = list))
}

/// "Presets: 2 added, 1 replaced." counting only those that would change something; `None` when none would.
pub fn presets_line(plan: &Plan) -> Option<String> {
    let (added, replaced) = (plan.presets_added, plan.presets_replaced);
    match (added, replaced) {
        (0, 0) => None,
        (a, 0) => Some(crate::i18n::tr_format!("Presets: {a} added.", a = a)),
        (0, r) => Some(crate::i18n::tr_format!("Presets: {r} replaced.", r = r)),
        (a, r) => Some(crate::i18n::tr_format!("Presets: {a} added, {r} replaced.", a = a, r = r)),
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
    for line in [replaces_line(&prompt.plan), presets_line(&prompt.plan)].into_iter().flatten() {
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(line).color(t.text_dim)).wrap());
    }
    if let Some(line) = unknown_line(&prompt.plan.unknown, prompt.plan.unknown_total, prompt.plan.newer.is_some()) {
        ui.add_space(4.0);
        let r = ui.add(egui::Label::new(egui::RichText::new(line).color(t.text_dim)).wrap());
        crate::widgets::register(ui, "text:settingsSyncSkipped", r.rect);
        r.on_hover_text(unknown_list(&prompt.plan.unknown, prompt.plan.unknown_total));
    }
    for note in prompt.plan.notes.iter().take(3) {
        ui.add(egui::Label::new(egui::RichText::new(note).size(11.5).color(t.caution)).wrap());
    }
}

/// The Replace dialog's body: the question.
pub fn replace_body(app: &LightcraftApp, ui: &mut egui::Ui, _t: &Tokens) {
    ui.set_max_width(460.0);
    let question = app.sync_settings.replace.clone().unwrap_or_default();
    let r = ui.add(egui::Label::new(question).wrap());
    crate::widgets::register(ui, "text:settingsSyncReplace", r.rect);
}

/// OK on the prompt: put the file's settings in place, and note that this Mac has a file from the folder.
/// When that fails the prompt stays, with the error on screen.
pub fn confirm(app: &mut LightcraftApp) -> Result<Value, String> {
    let Some(prompt) = app.sync_settings.prompt.clone() else { return Err(crate::i18n::tr("There is nothing to load.").to_string()) };
    let report = prompt.plan.report(true);
    super::apply(app, prompt.plan)?;
    app.sync_settings.prompt = None;
    if let Some(at) = prompt.saved_at {
        app.ui.sync.last = at;
    }
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    app.toast(&ctx, crate::i18n::tr_format!("Loaded the settings in {name}", name = prompt.name));
    Ok(json!({"applied": true, "report": report}))
}
