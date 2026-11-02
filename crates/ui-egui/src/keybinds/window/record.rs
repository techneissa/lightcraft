//! The status line under the list: what recording a shortcut would take away from other commands
//! (shown while the keys are pressed, before Enter), why a key was refused, or the clash among
//! saved shortcuts.

use egui::{RichText, Sense, UiBuilder, vec2};

use crate::LightcraftApp;
use crate::shortcuts::{self, Keymap};
use crate::theme::Tokens;

use super::super::{chord, model};
use super::clashes::Clashes;

/// Where the cell being recorded into was drawn last frame: (command, slot, rect).
const CELL: &str = "shortcuts-recording-cell";

/// The recording cell says where it is, for [`end_on_outside_press`].
pub(super) fn note_cell(ctx: &egui::Context, id: &'static str, slot: usize, rect: egui::Rect) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(CELL), (id, slot, rect)));
}

/// A press outside the cell being recorded into (another row, the list's background, Add Shortcut,
/// the search) ends the recording as Esc does, and so does the search taking the keyboard: nothing
/// is saved and the press goes on to what it hit. Run at the start of the window's frame.
pub(super) fn end_on_outside_press(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(cap) = app.keybinds.capture.as_ref().filter(|c| c.inline) else { return };
    let cell = ctx.data(|d| d.get_temp::<(&'static str, usize, egui::Rect)>(egui::Id::unique(CELL)));
    let outside = match cell {
        // the cell is drawn from the frame after the click that opened it on
        Some((id, slot, rect)) if id == cap.id && slot == cap.slot => {
            ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_none_or(|p| !rect.contains(p)))
        }
        _ => false,
    };
    if outside || ctx.text_edit_focused() {
        super::super::capture::close(app);
    }
}

/// Height of the status line: two lines of small text, kept so the list doesn't jump when it fills.
const LINE_H: f32 = 32.0;

/// The bindings the keys recorded so far in the window would take away from other commands.
pub(super) fn live_conflicts(app: &LightcraftApp, keymap: &Keymap) -> Vec<model::Holder> {
    let Some(cap) = app.keybinds.capture.as_ref().filter(|c| c.inline) else { return Vec::new() };
    cap.text().and_then(|s| chord::parse_chord(&s)).map(|steps| model::conflicts(keymap, &cap.id, &steps)).unwrap_or_default()
}

fn name(id: &str) -> String {
    shortcuts::find_bindable(id).map_or(id.to_string(), |b| crate::i18n::tr(b.label).to_string())
}

/// The sentence for the clash a recording has now, if any.
pub(super) fn live_warning(conflicts: &[model::Holder]) -> Option<String> {
    (!conflicts.is_empty()).then(|| {
        conflicts
            .iter()
            .map(|(other, slot)| crate::i18n::tr_format!("Also used by {} (Shortcut {}); saving takes it away from there.", name(other), slot + 1))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Draw the line: the recording's warning or hint while recording, else the first saved clash.
pub(super) fn status(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens, keymap: &Keymap, clashes: &Clashes, conflicts: &[model::Holder], mac: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), LINE_H), Sense::hover());
    // (a child that does not take room from the list's layout: the line is always LINE_H tall)
    let mut line = ui.new_child(UiBuilder::new().max_rect(rect));
    let ui = &mut line;
    if let Some(cap) = app.keybinds.capture.as_ref().filter(|c| c.inline) {
        if let Some(e) = &cap.error {
            ui.label(RichText::new(e).size(11.5).color(t.caution));
        } else if let Some(w) = live_warning(conflicts) {
            ui.label(RichText::new(w).size(11.5).color(t.caution));
        } else {
            ui.label(RichText::new(crate::i18n::tr("Press the keys, then Enter to save.")).size(11.5).color(t.text_dim))
                .on_hover_text(crate::i18n::tr("Press two keys one after the other for a chord. Esc cancels."));
        }
    } else {
        clash_note(ui, t, clashes, keymap, mac);
    }
}

/// The first clash among the saved shortcuts in words, in the caution colour.
fn clash_note(ui: &mut egui::Ui, t: &Tokens, clashes: &Clashes, keymap: &Keymap, mac: bool) {
    let Some((id, (other, why))) = clashes.reasons.iter().min_by_key(|(id, _)| **id) else { return };
    let key = (0..model::SLOTS)
        .find(|slot| clashes.slots.contains_key(&(*id, *slot)))
        .and_then(|slot| model::effective(keymap, id, slot))
        .map(|sc| chord::text(sc, mac))
        .unwrap_or_default();
    let more = clashes.reasons.len().saturating_sub(2);
    let mut line =
        crate::i18n::tr_format!("{} and {} share {}: {}. Give one of them a condition to tell them apart.", name(id), name(other), key, why);
    if more > 0 {
        line.push_str(&crate::i18n::tr_format!(" {} more conflicts.", more));
    }
    ui.label(RichText::new(line).size(11.5).color(t.caution));
}
