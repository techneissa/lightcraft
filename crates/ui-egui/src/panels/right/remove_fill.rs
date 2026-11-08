//! Fork feature (`fork/lama-fill.md`): the Remove panel's Fill picker, at the right of its heading. It sets the fill for the
//! spots made from now on (each spot keeps its own); Find People / Find Wires → Remove and Remove All use it too.
//!
//! LaMa is offered when its model is installed; without it the entry reads "LaMa (Download…)" and asks before it
//! downloads anything (the size and where it goes), and while it downloads the entry shows how far it is.

use egui::{Align, Align2, Layout, Rect, Sense, UiBuilder, pos2, vec2};
use lightcraft_develop::SpotFill;
use lightcraft_engine::lama_fill::{DOWNLOAD_BYTES, Download};
use serde_json::Value;

use super::{LightcraftApp, Tokens, json, register, text_button};

/// The memory flag of the "Download LaMa?" question.
fn ask_id() -> egui::Id {
    egui::Id::unique("lc-lama-ask")
}

/// The fill new spots get, and the picker shows: LaMa when it is chosen and its model is installed, else Classic (a
/// LaMa that is still downloading is chosen, but new spots keep the Classic fill until it arrives).
fn effective(app: &LightcraftApp) -> SpotFill {
    // (Object mode keeps its own choice, LaMa unless Classic is picked: `fork/object-remove.md`)
    if super::remove_object::chosen_fill(app) == SpotFill::Lama && app.session.lama.installed() { SpotFill::Lama } else { SpotFill::Classic }
}

/// `params` for a command that makes spots of `mode` (`remove`, `fill`, `heal`, `clone`), with the chosen fill when
/// the mode takes one. Heal and Clone copy a source, so they never get one.
pub(crate) fn with_fill(app: &LightcraftApp, mode: &str, mut params: Value) -> Value {
    if effective(app) == SpotFill::Lama && matches!(mode, "remove" | "fill") {
        params["fill"] = json!("lama");
    }
    params
}

fn lama_label(app: &LightcraftApp) -> String {
    match app.session.lama.download_state() {
        Download::Running { bytes, total } if !app.session.lama.installed() => {
            crate::i18n::tr_format!("LaMa (Downloading {}%)", (bytes * 100 / total.max(1)).min(100))
        }
        _ if app.session.lama.installed() => crate::i18n::tr("LaMa").to_string(),
        _ => crate::i18n::tr("LaMa (Download…)").to_string(),
    }
}

/// The Remove panel's heading with the Fill picker at its right.
pub(super) fn heading(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 24.0, r.center().y + 2.0), Align2::LEFT_CENTER, crate::i18n::tr("Remove"), t.semibold(15.0), t.text);
    if !super::remove_compact::takes_fill(app) {
        ask(app, ui.ctx());
        return;
    }
    let area = Rect::from_min_max(pos2(r.left() + 120.0, r.top()), pos2(r.right() - 16.0, r.bottom()));
    ui.scope_builder(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)), |ui| {
        let current = effective(app);
        let installed = app.session.lama.installed();
        let shown = if current == SpotFill::Lama { "LaMa".to_string() } else { "Classic".to_string() };
        let r = crate::widgets::dropdown(ui, "removeFill", crate::i18n::tr(&shown), t.font(12.5), t.text_label)
            .on_hover_text(crate::i18n::tr("Fill for new spots: Classic or LaMa"));
        ui.label(egui::RichText::new(crate::i18n::tr("Fill")).color(t.text_dim));
        let items = [SpotFill::Classic, SpotFill::Lama];
        let downloading = matches!(app.session.lama.download_state(), Download::Running { .. }) && !installed;
        let label = |f: &SpotFill| match f {
            SpotFill::Classic => crate::i18n::tr("Classic").to_string(),
            SpotFill::Lama => lama_label(app),
        };
        let picked = crate::widgets::dropdown_search::menu_pick_with(
            &mut *ui,
            &r,
            &items,
            |f| *f == current,
            label,
            |ui, f, item| register(ui, format!("removeFill:{}", f.name()), item.rect),
        )
        .and_then(|i| items.get(i).copied());
        match picked {
            Some(SpotFill::Classic) => super::remove_object::set_fill(app, SpotFill::Classic),
            Some(SpotFill::Lama) if installed => super::remove_object::set_fill(app, SpotFill::Lama),
            Some(SpotFill::Lama) if !downloading => {
                ui.ctx().data_mut(|d| d.insert_temp(ask_id(), true));
            }
            _ => {}
        }
        if downloading {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    });
    ask(app, ui.ctx());
}

/// "Download LaMa?": says what and how big, and downloads only on Download.
fn ask(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !ctx.data(|d| d.get_temp::<bool>(ask_id())).unwrap_or(false) {
        return;
    }
    let t = Tokens::get(ctx);
    let folder = app.session.lama.dir().map(|d| d.display().to_string()).unwrap_or_default();
    let modal = egui::Modal::new(egui::Id::unique("lc-lama-ask-modal")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Download LaMa?")).font(t.semibold(15.0)).color(t.text));
        ui.add_space(8.0);
        ui.label(crate::i18n::tr("LaMa rebuilds large removed areas, and busy backgrounds, better than the Classic fill."));
        ui.add_space(4.0);
        ui.label(crate::i18n::tr_format!(
            "It is a {} MB download from huggingface.co, kept in the folder LightCraft shares with PhotoCraft:",
            (DOWNLOAD_BYTES + 500_000) / 1_000_000
        ));
        ui.label(egui::RichText::new(&folder).font(t.font(11.5)).color(t.text_dim));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Or copy big-lama.pt into that folder yourself.")).font(t.font(11.5)).color(t.text_dim));
        ui.add_space(10.0);
        let mut choice = None;
        ui.horizontal(|ui| {
            if text_button(ui, "lamaDownload", crate::i18n::tr("Download"), true).clicked() {
                choice = Some(true);
            }
            if text_button(ui, "lamaCancel", crate::i18n::tr("Cancel"), false).clicked() {
                choice = Some(false);
            }
        });
        choice
    });
    let choice = modal.inner.or(modal.should_close().then_some(false));
    if let Some(download) = choice {
        ctx.data_mut(|d| d.insert_temp(ask_id(), false));
        if download {
            match app.run("lama.download", json!({"confirm": true})) {
                Ok(_) => super::remove_object::set_fill(app, SpotFill::Lama),
                Err(e) => app.toast(ctx, e),
            }
        }
    }
}

/// Under the Remove panel's buttons: why LaMa is not drawing, while it is the fill in use (no model, a failed download or run).
pub(super) fn notice(app: &LightcraftApp, ui: &mut egui::Ui) {
    let lama_in_use = app.ui.remove_fill == SpotFill::Lama
        || app.session.active().and_then(|id| app.session.develop_of(id)).is_some_and(|d| d.spots.iter().any(|s| s.fill == SpotFill::Lama));
    if !lama_in_use {
        return;
    }
    let failed = match app.session.lama.download_state() {
        Download::Failed(e) => Some(e),
        _ => None,
    };
    let Some(text) = failed.or_else(|| app.session.lama.message()) else { return };
    super::padded(ui, |ui| {
        ui.label(egui::RichText::new(crate::i18n::tr(&text)).size(11.0).color(Tokens::get(ui.ctx()).text_dim));
    });
}
