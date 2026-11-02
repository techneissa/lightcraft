//! The keys that belong to no single command (ratings, labels, grid navigation), listed under the
//! commands.

use egui::RichText;

use crate::shortcuts::{self, Keymap};
use crate::theme::Tokens;

use super::super::{chord, model};

/// Keys that aren't a command's own shortcut (ratings, labels, keys with parameters): listed, not editable.
/// One the user gave to a command shows struck through.
pub(super) fn fixed_keys(ui: &mut egui::Ui, t: &Tokens, mac: bool, needle: &str, keymap: &Keymap) {
    let mut fixed: Vec<(String, &str)> = Vec::new();
    for (sc, id, _) in shortcuts::ALIASES {
        let label = shortcuts::find_bindable(id).map_or(*id, |b| b.label);
        fixed.push((sc.to_string(), label));
    }
    for n in 0..=5 {
        fixed.push((n.to_string(), ["Clear rating", "Rate ★", "Rate ★★", "Rate ★★★", "Rate ★★★★", "Rate ★★★★★"][n]));
        fixed.push((
            format!("Shift+{n}"),
            [
                "Clear rating and advance",
                "Rate ★ and advance",
                "Rate ★★ and advance",
                "Rate ★★★ and advance",
                "Rate ★★★★ and advance",
                "Rate ★★★★★ and advance",
            ][n],
        ));
    }
    for (sc, label) in [("6", "Red label"), ("7", "Yellow label"), ("8", "Green label"), ("9", "Blue label")] {
        fixed.push((sc.to_string(), label));
    }
    // grid navigation (Photo Grid and Square Grid): fixed keys that belong to no single command
    for (sc, label) in [
        ("Home", "Grid: first photo of the view"),
        ("End", "Grid: last photo of the view"),
        ("Shift+Left", "Grid: add the previous photo to the selection"),
        ("Shift+Right", "Grid: add the next photo to the selection"),
    ] {
        fixed.push((sc.to_string(), label));
    }
    let fixed: Vec<_> = fixed
        .into_iter()
        .filter(|(sc, label)| {
            needle.is_empty() || crate::i18n::tr(label).to_lowercase().contains(needle) || chord::text(sc, mac).to_lowercase().contains(needle)
        })
        .collect();
    if fixed.is_empty() {
        return;
    }
    ui.add_space(14.0);
    ui.label(RichText::new(crate::i18n::tr("Fixed Keys")).font(t.semibold(12.5)).color(t.accent));
    ui.add_space(2.0);
    // keys the keymap gives to commands (single keys, second shortcuts, chord leaders) override these
    let taken: Vec<_> = shortcuts::saved_keys(keymap).map(|(_, k)| k).chain(model::extra_taken(keymap)).collect();
    egui::Grid::new("shortcuts-fixed").striped(true).num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        for (sc, label) in fixed {
            ui.label(RichText::new(crate::i18n::tr(label)).color(t.text_label));
            let mut text = RichText::new(chord::text(&sc, mac)).color(t.text_dim);
            if shortcuts::parse(&sc).is_some_and(|k| taken.contains(&k)) {
                text = text.strikethrough();
            }
            ui.label(text);
            ui.end_row();
        }
    });
}
