//! The Remove panel's Find row (Dust, People, Wires), and, once something is found, how many and
//! Remove / Remove All / Clear. The outlines on the photo are drawn by `panels/detail/find_overlay.rs`.

use super::{LightcraftApp, Tokens, json, padded, text_button};
use lightcraft_pipeline::find::Kind;

/// Shown in the toast once per finished search.
fn toast_new_outcome(app: &mut LightcraftApp, ui: &egui::Ui) {
    let Some(last) = app.session.finds.view(app.session.active()).last else { return };
    let key = egui::Id::unique("find-removals-seen");
    let seen = ui.ctx().data_mut(|d| d.get_temp::<u64>(key));
    if seen != Some(last.seq) {
        ui.ctx().data_mut(|d| d.insert_temp(key, last.seq));
        // (the first look at a photo's old result is not news)
        if seen.is_some() {
            app.toast(ui.ctx(), last.text);
        }
    }
}

pub(super) fn section(app: &mut LightcraftApp, ui: &mut egui::Ui, spots: usize) {
    let view = app.session.finds.view(app.session.active());
    toast_new_outcome(app, ui);
    if !view.running.is_empty() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    egui::Frame::NONE.inner_margin(super::remove_compact::margin()).show(ui, |ui| {
        let busy = |kind| view.running.contains(&kind);
        match super::remove_compact::find(ui, [false, busy(Kind::Person), busy(Kind::Wire)], spots) {
            Some(0) => find_dust(app, ui),
            Some(i) => {
                let command = if i == 1 { "spot.findPeople" } else { "spot.findWires" };
                if let Err(e) = app.run(command, json!({})) {
                    app.toast(ui.ctx(), e);
                }
            }
            None => {}
        }
    });
    padded(ui, |ui| {
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
                    && let Err(e) = app.run("spot.removeCandidates", super::with_fill(app, "fill", json!({})))
                {
                    app.toast(ui.ctx(), e);
                }
            });
            if text_button(ui, "findRemoveAll", "Remove All", false).clicked()
                && let Err(e) = app.run("spot.removeCandidates", super::with_fill(app, "fill", json!({"all": true})))
            {
                app.toast(ui.ctx(), e);
            }
            if text_button(ui, "findClear", "Clear", false).clicked() {
                let _ = app.run("spot.clearCandidates", json!({}));
            }
        });
    });
    super::remove_fill::notice(app, ui);
}

/// Find Dust: heals the small dark spots sensor dust leaves, and says how many in a toast.
fn find_dust(app: &mut LightcraftApp, ui: &egui::Ui) {
    match app.run("spot.findDust", json!({})) {
        Ok(r) => {
            let n = r["added"].as_u64().unwrap_or(0);
            app.toast(
                ui.ctx(),
                if n == 0 {
                    crate::i18n::tr("No dust spots found").to_string()
                } else {
                    crate::i18n::tr_format!("Healed {n} dust spot{}", if n == 1 { "" } else { "s" }, n = n)
                },
            );
        }
        Err(e) => app.toast(ui.ctx(), e),
    }
}
