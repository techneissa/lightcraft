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
    /// Saved from All Faces: it goes to every face, and follows nobody.
    any_face: bool,
}

/// What a portrait preset's row says after its name: the person it follows, "All Faces", or "By Face" (a face
/// known only to recognition). `None` for a preset that is not a portrait one.
pub(crate) fn tag(p: &lightcraft_develop::Preset) -> Option<String> {
    let part = pp::split(p).1?;
    Some(match part.who.person {
        Some(n) => super::overlay::title_case(&n),
        None if part.is_any_face() => tr("All Faces").to_string(),
        None => tr("By Face").to_string(),
    })
}

fn rows(app: &crate::LightcraftApp) -> Vec<Row> {
    app.session
        .presets
        .iter()
        .filter(|p| pp::is_portrait(p))
        .map(|p| {
            let part = pp::split(p).1;
            Row {
                id: p.id.clone(),
                name: p.name.clone(),
                person: part.as_ref().and_then(|x| x.who.person.as_deref()).map(super::overlay::title_case),
                any_face: part.as_ref().is_none_or(pp::Portrait::is_any_face),
            }
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
        let r = ui.checkbox(&mut draft.others, tr("Include the Photo's Other Settings"));
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
                let tag = match &row.person {
                    Some(p) => p.clone(),
                    None if row.any_face => tr("All Faces").to_string(),
                    None => tr("By Face").to_string(),
                };
                ui.label(RichText::new(tag).size(11.5).color(t.text_dim));
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
            let b = ui.button(tr_format!("Apply to Every Photo of {who}", who = name));
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

/// What Apply to Every Photo did: "Applied to 12 photos", the photos that could not take it, or the error. `None`
/// while it runs in the background (its row is in the activity stack; [`pump`] says how it ended).
fn done(r: Result<Value, String>) -> Option<String> {
    let v = match r {
        Ok(v) => v,
        Err(e) => return Some(e),
    };
    match v["state"].as_str() {
        Some("running") => None,
        Some("cancelled") => Some(tr("Stopped: no photo was changed").to_string()),
        _ => {
            let n = v["count"].as_u64().unwrap_or(0);
            let applied = if n == 1 { tr("Applied to 1 photo").to_string() } else { tr_format!("Applied to {n} photos", n = n) };
            let failed = v["failed"].as_array().map_or(0, Vec::len);
            Some(match failed {
                0 => applied,
                1 => tr_format!("{applied}; 1 photo could not take it (too many faces with their own settings)", applied = applied),
                f => tr_format!("{applied}; {f} photos could not take it (too many faces with their own settings)", applied = applied, f = f),
            })
        }
    }
}

/// Every frame: finish Apply to Every Photo running in the background, and say how it went.
pub(crate) fn pump(app: &mut crate::LightcraftApp, ctx: &egui::Context) {
    if !app.session.portrait_job_running() {
        return;
    }
    match app.session.portrait_pump() {
        Some(r) => {
            if let Some(m) = done(r.map_err(|e| e.to_string())) {
                app.toast(ctx, m);
            }
        }
        None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
    }
}

/// After `preset.apply`: how many photos did not have the person, when some did not.
fn skipped(v: &Value) -> Option<String> {
    let n = v["portrait"]["skipped"].as_array().map_or(0, Vec::len);
    (n > 0)
        .then(|| if n == 1 { tr("1 photo does not show this person").to_string() } else { tr_format!("{n} photos do not show this person", n = n) })
}

/// The People view's menu on a person's page: each portrait preset, to every photo of `person`.
pub(super) fn person_menu(app: &mut crate::LightcraftApp, ui: &mut egui::Ui, person: &str) {
    // a preset from All Faces follows nobody: not offered here
    let list: Vec<Row> = rows(app).into_iter().filter(|r| !r.any_face).collect();
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// The toast says how many photos changed and how many could not take it; nothing while it runs on.
    #[test]
    fn the_message_says_what_changed_and_what_could_not() {
        assert_eq!(super::done(Ok(json!({"state": "done", "count": 3, "failed": []}))).as_deref(), Some("Applied to 3 photos"));
        let m = super::done(Ok(json!({"state": "done", "count": 2, "failed": [{"id": 4, "why": "full"}]}))).unwrap();
        assert!(m.starts_with("Applied to 2 photos; 1 photo could not take it"), "{m}");
        assert_eq!(super::done(Ok(json!({"state": "running"}))), None);
        assert_eq!(super::done(Err("nothing was changed: x".into())).as_deref(), Some("nothing was changed: x"));
    }
}
