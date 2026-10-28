//! The search of a preset picker: a combo box whose list holds a few fixed rows ("None", "LightCraft
//! Default") and then every preset, drawn by `panels::presets::picker_rows`, which builds only the
//! rows in view because a library can hold 10 000 presets. Typing narrows the fixed rows and the
//! presets together, and the matches are a flat list that is just as lazy.

use std::sync::Arc;

use egui::{Ui, vec2};
use lightcraft_engine::presets::PresetList;

use super::{Filter, before_lazy};

/// What a preset picker's list gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The fixed row at this position among those given.
    Fixed(usize),
    /// The preset with this id.
    Preset(String),
}

/// The labels of every preset as displayed, made again only when the presets or the language change.
fn labels(ui: &Ui, presets: &PresetList) -> Arc<Vec<String>> {
    let id = egui::Id::unique("lc-preset-search-labels");
    let key = (presets.generation(), crate::i18n::language());
    if let Some((cached, labels)) = ui.data(|d| d.get_temp::<((u64, crate::i18n::Locale), Arc<Vec<String>>)>(id))
        && cached == key
    {
        return labels;
    }
    let labels: Arc<Vec<String>> = Arc::new(presets.iter().map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string()).collect());
    ui.data_mut(|d| d.insert_temp(id, (key, labels.clone())));
    labels
}

/// [`super::before`] for a preset picker: `fixed` are the labels of its fixed rows, then come the
/// presets. Nothing is made while the list is closed.
pub fn before(ui: &mut Ui, salt: &str, fixed: &[String], presets: &PresetList) -> Filter {
    let presets_labels = labels(ui, presets);
    before_lazy(ui, salt, || fixed.iter().cloned().chain(presets_labels.iter().cloned()).collect(), &[])
}

/// The list of the picker (inside the combo box's list, under its search): with nothing typed the
/// fixed rows (label, whether it is the current choice) and then the usual preset rows; with text
/// typed, the rows that match. `selected` is the current preset's id. Returns the row clicked or
/// picked with ↩.
pub fn list(ui: &mut Ui, find: &Filter, salt: &str, fixed: &[(String, bool)], presets: &PresetList, selected: Option<&str>) -> Option<Choice> {
    if let Some(i) = find.picked(ui) {
        ui.close();
        return match i.checked_sub(fixed.len()) {
            None => Some(Choice::Fixed(i)),
            Some(at) => presets.get(at).map(|p| Choice::Preset(p.id.clone())),
        };
    }
    if !find.filtering() {
        let mut out = None;
        for (i, (label, current)) in fixed.iter().enumerate() {
            if ui.selectable_label(*current, label).clicked() {
                out = Some(Choice::Fixed(i));
            }
        }
        return crate::panels::presets::picker_rows(ui, salt, presets, selected).map(Choice::Preset).or(out);
    }
    let shown = find.matches();
    let names = labels(ui, presets);
    let row_h = ui.spacing().interact_size.y;
    let state = egui::Id::unique("lc-preset-search-scroll").with(salt);
    let mut area = egui::ScrollArea::vertical().id_salt(("preset-search", salt)).max_height(300.0).auto_shrink([false, true]);
    // keep the highlighted row in view: it may not be drawn yet, so the offset is worked out from the last one
    if find.moved()
        && let Some(at) = find.active_pos()
    {
        let (offset, view) = ui.data(|d| d.get_temp::<(f32, f32)>(state)).unwrap_or((0.0, 300.0));
        let (top, bottom) = (at as f32 * row_h, (at as f32 + 1.0) * row_h);
        if top < offset {
            area = area.vertical_scroll_offset(top);
        } else if bottom > offset + view {
            area = area.vertical_scroll_offset(bottom - view);
        }
    }
    let mut out = None;
    let shown_area = area.show_rows(ui, row_h, shown.len(), |ui, range| {
        for at in range {
            let Some(&i) = shown.get(at) else { continue };
            let size = vec2(ui.available_width(), row_h);
            let (label, current, choice) = match i.checked_sub(fixed.len()) {
                None => match fixed.get(i) {
                    Some((label, current)) => (label.as_str(), *current, Choice::Fixed(i)),
                    None => continue,
                },
                Some(p) => match (presets.get(p), names.get(p)) {
                    (Some(preset), Some(label)) => (label.as_str(), selected == Some(preset.id.as_str()), Choice::Preset(preset.id.clone())),
                    _ => continue,
                },
            };
            let row = ui.add_sized(size, egui::Button::selectable(current, label));
            find.highlight(ui, &row, at);
            if row.clicked() {
                out = Some(choice);
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(state, (shown_area.state.clamped_offset().y, shown_area.inner_rect.height())));
    find.no_match(ui);
    out
}
