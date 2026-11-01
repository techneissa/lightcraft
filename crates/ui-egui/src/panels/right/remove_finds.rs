//! The Remove panel's Find People / Find Wires: the two search buttons, and, once something is found, how many and
//! Remove / Remove All / Clear. The outlines on the photo are drawn by `panels/detail/find_overlay.rs`.

use super::{LightcraftApp, Tokens, json, padded, text_button};
use lightcraft_pipeline::find::Kind;

/// Shown in the toast once per finished search.
fn toast_new_outcome(app: &mut LightcraftApp, ui: &egui::Ui) {
    let Some(last) = app.session.finds.view(app.session.active()).last else { return };
    let key = egui::Id::new("find-removals-seen");
    let seen = ui.ctx().data_mut(|d| d.get_temp::<u64>(key));
    if seen != Some(last.seq) {
        ui.ctx().data_mut(|d| d.insert_temp(key, last.seq));
        // (the first look at a photo's old result is not news)
        if seen.is_some() {
            app.toast(ui.ctx(), last.text);
        }
    }
}

pub(super) fn section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let view = app.session.finds.view(app.session.active());
    toast_new_outcome(app, ui);
    if !view.running.is_empty() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    padded(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (id, label, command, kind) in
                [("findPeople", "Find People", "spot.findPeople", Kind::Person), ("findWires", "Find Wires", "spot.findWires", Kind::Wire)]
            {
                let busy = view.running.contains(&kind);
                ui.add_enabled_ui(!busy, |ui| {
                    let r = text_button(ui, id, label, false);
                    let r = if kind == Kind::Person {
                        r.on_hover_text(crate::i18n::tr("Outline the people in the background; the main subject is left out"))
                    } else {
                        r
                    };
                    if r.clicked()
                        && let Err(e) = app.run(command, json!({}))
                    {
                        app.toast(ui.ctx(), e);
                    }
                });
            }
        });
        if view.entries.is_empty() {
            return;
        }
        ui.add_space(4.0);
        let n = view.entries.len();
        let selected = view.entries.iter().filter(|e| e.selected).count();
        ui.label(egui::RichText::new(crate::i18n::tr_format!("{} found", n)).color(Tokens::get(ui.ctx()).text_dim));
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(selected > 0, |ui| {
                if text_button(ui, "findRemove", "Remove", false).clicked()
                    && let Err(e) = app.run("spot.removeCandidates", json!({}))
                {
                    app.toast(ui.ctx(), e);
                }
            });
            if text_button(ui, "findRemoveAll", "Remove All", false).clicked()
                && let Err(e) = app.run("spot.removeCandidates", json!({"all": true}))
            {
                app.toast(ui.ctx(), e);
            }
            if text_button(ui, "findClear", "Clear", false).clicked() {
                let _ = app.run("spot.clearCandidates", json!({}));
            }
        });
    });
}
