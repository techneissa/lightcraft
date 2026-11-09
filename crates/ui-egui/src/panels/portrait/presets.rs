//! The Portrait tab's Portrait Presets section (fork, `fork/portrait-presets.md`, issue #106): save the picked face's
//! settings (or those for every face) as a preset that follows the person, apply one to the selected photos, or give
//! the picked face's settings or a preset to every photo of the person.
//!
//! Everything runs through commands: `retouch.savePreset`, `preset.apply` and `retouch.applyToPerson`.

use egui::RichText;
use lightcraft_engine::portrait_presets as pp;
use serde_json::{Value, json};

use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::text_field::TextField;
use crate::widgets::{register, register_response};

pub(super) const KEY: &str = "presets";
pub(super) const TITLE: &str = "Portrait Presets";

/// What the save row holds between frames: the name typed and the Include Other Settings tick.
#[derive(Clone, Default)]
struct Draft {
    name: String,
    others: bool,
}

fn draft_id() -> egui::Id {
    egui::Id::unique("portraitPresetDraft")
}

/// A portrait preset as a row shows it: id, name, and the person it follows (title case).
struct Row {
    id: String,
    name: String,
    person: Option<String>,
}

fn rows(app: &crate::LightcraftApp) -> Vec<Row> {
    app.session
        .presets
        .iter()
        .filter(|p| pp::is_portrait(p))
        .map(|p| Row {
            id: p.id.clone(),
            name: p.name.clone(),
            person: pp::split(p).1.and_then(|x| x.who.person).map(|n| super::overlay::title_case(&n)),
        })
        .collect()
}

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    let t = Tokens::get(cx.ui.ctx());
    let face = cx.face.and_then(|i| cx.input.faces.get(i).map(|f| (i, f)));
    let who = face.map(|(_, f)| cx.app.session.portrait_who(cx.photo, f));
    let mut draft: Draft = cx.ui.ctx().data(|d| d.get_temp(draft_id())).unwrap_or_default();
    let mut message: Option<String> = None;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 6 }).show(cx.ui, |ui| {
        // save
        let from = match face {
            Some((i, f)) => super::overlay::face_name(f, i),
            None => tr("All Faces").to_string(),
        };
        ui.label(RichText::new(tr_format!("Save {who}'s Settings", who = from)).size(12.0).color(t.text_label));
        let field = TextField::singleline("portrait:presetName", &mut draft.name).hint(tr("Preset Name")).width(ui.available_width() - 4.0).show(ui);
        let r = ui.checkbox(&mut draft.others, tr("Include Other Settings"));
        register_response("portrait:presetOthers", r.rect, &r);
        let named = !draft.name.trim().is_empty();
        let save = ui.add_enabled(named, egui::Button::new(tr("Save Portrait Preset")));
        register_response("portrait:savePreset", save.rect, &save);
        if named && (save.clicked() || field.ending == Some(crate::widgets::text_field::Ending::Return)) {
            let p = json!({"name": draft.name.trim(), "face": cx.face, "id": cx.photo.0, "others": draft.others});
            match cx.app.run("retouch.savePreset", p) {
                Ok(_) => draft.name.clear(),
                Err(e) => message = Some(e),
            }
        }
        ui.add_space(8.0);
        // the saved presets
        let list = rows(cx.app);
        if list.is_empty() {
            ui.label(RichText::new(tr("No portrait presets yet")).size(11.5).color(t.text_dim));
        }
        for (n, row) in list.iter().enumerate() {
            ui.horizontal(|ui| {
                let label = ui.label(RichText::new(&row.name).size(12.5).color(t.text));
                register(ui, format!("portrait:preset:{n}"), label.rect);
                if let Some(p) = &row.person {
                    ui.label(RichText::new(p).size(11.5).color(t.text_dim));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let more = ui.menu_button("…", |ui| {
                        if let Some(p) = &row.person
                            && ui.button(tr_format!("Apply to Every Photo of {who}", who = p)).clicked()
                        {
                            message = done(cx.app.run("retouch.applyToPerson", json!({"name": p, "preset": row.id})));
                            ui.close();
                        }
                        if ui.button(tr("Delete Preset")).clicked() {
                            if let Err(e) = cx.app.run("preset.delete", json!({"id": row.id})) {
                                message = Some(e);
                            }
                            ui.close();
                        }
                    });
                    register(ui, format!("portrait:presetMenu:{n}"), more.response.rect);
                    let apply = ui.button(tr("Apply"));
                    register_response(format!("portrait:applyPreset:{n}"), apply.rect, &apply);
                    if apply.clicked() {
                        match cx.app.run("preset.apply", json!({"id": row.id})) {
                            Ok(v) => message = skipped(&v),
                            Err(e) => message = Some(e),
                        }
                    }
                });
            });
        }
        // the picked face's settings to every photo of the person
        if let (Some((i, f)), Some(who)) = (face, &who)
            && who.is_known()
        {
            ui.add_space(8.0);
            let name = who.person.as_deref().map(super::overlay::title_case).unwrap_or_else(|| super::overlay::face_name(f, i));
            let b = ui.button(tr_format!("Apply to Every Photo of {who}", who = name)).on_hover_text(tr("This face's settings"));
            register_response("portrait:applyToPerson", b.rect, &b);
            if b.clicked() {
                message = done(cx.app.run("retouch.applyToPerson", json!({"face": i, "id": cx.photo.0})));
            }
        }
    });
    cx.ui.ctx().data_mut(|d| d.insert_temp(draft_id(), draft));
    if let Some(m) = message {
        cx.app.toast(cx.ui.ctx(), m);
    }
}

/// "Applied to 12 photos", or the error.
fn done(r: Result<Value, String>) -> Option<String> {
    Some(match r {
        Ok(v) => {
            let n = v["count"].as_u64().unwrap_or(0);
            if n == 1 { tr("Applied to 1 photo").to_string() } else { tr_format!("Applied to {n} photos", n = n) }
        }
        Err(e) => e,
    })
}

/// After `preset.apply`: how many photos did not have the person, when some did not.
fn skipped(v: &Value) -> Option<String> {
    let n = v["portrait"]["skipped"].as_array().map_or(0, Vec::len);
    (n > 0)
        .then(|| if n == 1 { tr("1 photo does not show this person").to_string() } else { tr_format!("{n} photos do not show this person", n = n) })
}

/// The People view's menu on a person's page: each portrait preset, to every photo of `person`.
pub(super) fn person_menu(app: &mut crate::LightcraftApp, ui: &mut egui::Ui, person: &str) {
    let list = rows(app);
    if list.is_empty() {
        return;
    }
    let mut message = None;
    let menu = ui.menu_button(tr("Apply Portrait Preset"), |ui| {
        for row in &list {
            if ui.button(&row.name).clicked() {
                message = done(app.run("retouch.applyToPerson", json!({"name": person, "preset": row.id})));
                ui.close();
            }
        }
    });
    register(ui, "person:portraitPreset", menu.response.rect);
    if let Some(m) = message {
        app.toast(ui.ctx(), m);
    }
}
