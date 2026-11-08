//! Fork feature (`fork/learn-my-edits.md`): the Learn My Edits window (Photo ▸ Learn My Edits…).
//!
//! The profiles on the left; the chosen one on the right: its name and kind, what it learns, its examples, how
//! accurate it was on photos it had not seen, and Apply to Selected Photos / Add Selected Photos as Examples /
//! Train Again / Export… / Delete. New Profile… learns from the selected photos or the photos shown. Without the
//! image model the window asks before it downloads it (88 MB).
//!
//! Everything goes through the engine's `learn.*` commands; jobs run in the background with a row in the activity
//! stack, and [`pump`] takes in the ones that end, every frame, window open or not.

use egui::{Color32, RichText};
use lightcraft_learn::Group;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::pick::{PickRequest, Picked};
use crate::theme::Tokens;

/// Kinds of photography offered as one-click names.
const KINDS: [&str; 7] = ["Portraits", "Landscapes", "Animals", "Sky", "Skin Tones", "Street", "Night"];

/// The window's state (not saved).
#[derive(Clone, Debug, Default)]
pub struct Window {
    pub open: bool,
    /// The chosen profile's id.
    pub selected: Option<String>,
    /// The New Profile form, while it is open.
    pub new: Option<NewForm>,
    /// The name and kind fields of the chosen profile, as typed.
    pub name: String,
    pub kind: String,
    /// Which profile the fields were filled from.
    pub fields_of: Option<String>,
    pub ask_download: bool,
    pub ask_delete: bool,
    /// The last answer worth showing in the window (an error of a button).
    pub note: Option<(String, bool)>,
}

#[derive(Clone, Debug)]
pub struct NewForm {
    pub name: String,
    pub kind: String,
    pub groups: Vec<Group>,
    /// Learn from the photos shown, not the selected ones.
    pub from_view: bool,
}

impl Default for NewForm {
    fn default() -> Self {
        NewForm { name: String::new(), kind: String::new(), groups: Group::ALL.to_vec(), from_view: false }
    }
}

/// `dialog.learnEdits`, `learnEdits.export` and `learnEdits.import`.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    match id {
        "dialog.learnEdits" => {
            app.ui.learn_edits.open = true;
            Ok(Value::Null)
        }
        "learnEdits.export" => {
            let profile = p
                .get("profile")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| app.ui.learn_edits.selected.clone())
                .ok_or("choose a profile")?;
            let path = match p.get("path").and_then(Value::as_str) {
                Some(x) => x.to_string(),
                None => {
                    let name = format!("{profile}.{}", lightcraft_learn::profile::EXTENSION);
                    let req = PickRequest::save(tr("Export Profile"), tr("Learn My Edits Profile"), &["lclearn"], name);
                    let mut params = p.clone();
                    params["profile"] = json!(profile);
                    match crate::pick::ask(app, id, &params, "path", req, |_| None) {
                        Picked::Now(v) => v.into_iter().next().ok_or("no file chosen")?,
                        Picked::Later => return Ok(Value::Null),
                        Picked::Unavailable => return Err("no file dialog on this platform".into()),
                    }
                }
            };
            let r = app.session.execute("learn.export", &json!({"profile": profile, "path": path})).map_err(|e| e.to_string());
            note(app, &r, tr("Profile exported"));
            r
        }
        "learnEdits.import" => {
            let path = match p.get("path").and_then(Value::as_str) {
                Some(x) => x.to_string(),
                None => {
                    let req = PickRequest::file(tr("Import Profile"), tr("Learn My Edits Profile"), &["lclearn"]);
                    match crate::pick::ask(app, id, p, "path", req, |_| None) {
                        Picked::Now(v) => v.into_iter().next().ok_or("no file chosen")?,
                        Picked::Later => return Ok(Value::Null),
                        Picked::Unavailable => return Err("no file dialog on this platform".into()),
                    }
                }
            };
            let r = app.session.execute("learn.import", &json!({"path": path})).map_err(|e| e.to_string());
            if let Ok(v) = &r {
                app.ui.learn_edits.selected = v["profile"]["id"].as_str().map(str::to_string);
            }
            note(app, &r, tr("Profile imported"));
            r
        }
        _ => Err(format!("unknown command `{id}`")),
    }
}

fn note(app: &mut LightcraftApp, r: &Result<Value, String>, ok: &str) {
    app.ui.learn_edits.note = Some(match r {
        Ok(_) => (ok.to_string(), false),
        Err(e) => (e.clone(), true),
    });
}

/// Take in a job that has ended (cheap: nothing runs while no job does), with a toast saying how it went.
pub fn pump(app: &mut LightcraftApp, ctx: &egui::Context) {
    if app.session.learn.running().is_none() {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    let Ok(r) = app.session.execute("learn.pump", &json!({})) else { return };
    if let Some(e) = r["error"].as_str() {
        app.toast_error(ctx, e.to_string());
    } else if let Some(m) = r["message"].as_str() {
        app.toast(ctx, m.to_string());
    }
}

/// A button registered for the control channel as `id`.
fn button(ui: &mut egui::Ui, id: &str, label: &str, enabled: bool) -> bool {
    let r = ui.add_enabled(enabled, egui::Button::new(label));
    crate::widgets::register_response(id, r.rect, &r);
    r.clicked()
}

fn run(app: &mut LightcraftApp, cmd: &str, p: Value) -> Option<Value> {
    match app.session.execute(cmd, &p) {
        Ok(v) => {
            app.ui.learn_edits.note = None;
            Some(v)
        }
        Err(e) => {
            app.ui.learn_edits.note = Some((e.to_string(), true));
            None
        }
    }
}

/// The window, when open.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.ui.learn_edits.open {
        return;
    }
    let Ok(status) = app.session.execute("learn.status", &json!({})) else { return };
    let mut open = true;
    egui::Window::new(tr("Learn My Edits"))
        .id(egui::Id::unique("lc-learn-edits"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([720.0, 480.0])
        .show(ctx, |ui| body(app, ui, &status));
    if !open {
        app.ui.learn_edits.open = false;
    }
}

fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, status: &Value) {
    let t = Tokens::get(ui.ctx());
    model_row(app, ui, &t, status);
    let profiles: Vec<Value> = status["profiles"].as_array().cloned().unwrap_or_default();
    if app.ui.learn_edits.selected.as_ref().is_none_or(|s| !profiles.iter().any(|p| p["id"].as_str() == Some(s))) {
        app.ui.learn_edits.selected = profiles.first().and_then(|p| p["id"].as_str()).map(str::to_string);
    }
    let busy = !status["job"].is_null();
    ui.columns(2, |cols| {
        let [left, right] = cols else { return };
        list(app, left, &t, &profiles);
        if app.ui.learn_edits.new.is_some() {
            new_form(app, right, &t, busy);
        } else if let Some(p) = profiles.iter().find(|p| p["id"].as_str() == app.ui.learn_edits.selected.as_deref()) {
            details(app, right, &t, p, busy);
        } else {
            right
                .label(RichText::new(tr("Make a profile from photos you have edited: it learns how you edit that kind of photo.")).color(t.text_dim));
        }
    });
    ui.separator();
    if let Some(job) = status["job"].as_object() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(job.get("label").and_then(Value::as_str).unwrap_or_default());
            if button(ui, "button:learnStop", tr("Stop"), true) {
                run(app, "learn.cancel", json!({}));
            }
        });
    }
    if let Some((text, error)) = app.ui.learn_edits.note.clone() {
        ui.label(RichText::new(text).color(if error { t.reject } else { t.text_dim }));
    } else if let Some(m) = status["message"].as_str() {
        ui.label(RichText::new(m).color(t.text_dim));
    }
}

fn model_row(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, status: &Value) {
    let model = &status["model"];
    if model["installed"].as_bool() == Some(true) {
        return;
    }
    let mb = (model["bytes"].as_u64().unwrap_or(0) + 500_000) / 1_000_000;
    let d = &model["download"];
    ui.horizontal(|ui| match d["state"].as_str() {
        Some("running") => {
            let pct = d["bytes"].as_u64().unwrap_or(0) * 100 / d["total"].as_u64().unwrap_or(1).max(1);
            ui.spinner();
            ui.label(tr_format!("Downloading the image model… {}%", pct.min(100)));
        }
        _ if app.ui.learn_edits.ask_download => {
            ui.label(tr_format!(
                "Download the image model ({} MB) from huggingface.co? It goes in the models folder LightCraft shares with PhotoCraft.",
                mb
            ));
            if button(ui, "button:learnDownloadCancel", tr("Cancel"), true) {
                app.ui.learn_edits.ask_download = false;
            }
            if button(ui, "button:learnDownloadConfirm", tr("Download"), true) {
                app.ui.learn_edits.ask_download = false;
                run(app, "learn.downloadModel", json!({"confirm": true}));
            }
        }
        state => {
            let text = match (state, d["error"].as_str()) {
                (Some("failed"), Some(e)) => tr_format!("The image model didn't download: {}", e),
                _ => tr_format!("Learn My Edits needs an image model ({} MB).", mb),
            };
            ui.label(RichText::new(text).color(t.caution));
            if button(ui, "button:learnDownload", tr("Download…"), true) {
                app.ui.learn_edits.ask_download = true;
            }
        }
    });
    ui.separator();
}

fn list(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, profiles: &[Value]) {
    ui.label(RichText::new(tr("Profiles")).font(t.semibold(13.0)));
    egui::ScrollArea::vertical().id_salt("lc-learn-list").max_height(300.0).show(ui, |ui| {
        if profiles.is_empty() {
            ui.label(RichText::new(tr("No profiles yet.")).color(t.text_dim));
        }
        for p in profiles {
            let Some(id) = p["id"].as_str() else { continue };
            let name = p["name"].as_str().unwrap_or(id);
            let sub = match (p["error"].as_str(), p["kind"].as_str().filter(|k| !k.is_empty())) {
                (Some(_), _) => tr("Can't be read").to_string(),
                (None, Some(kind)) => tr_format!("{} · {} examples", kind, p["examples"]),
                (None, None) => tr_format!("{} examples", p["examples"]),
            };
            let chosen = app.ui.learn_edits.selected.as_deref() == Some(id) && app.ui.learn_edits.new.is_none();
            let r = ui.selectable_label(chosen, format!("{name}\n{sub}"));
            crate::widgets::register_response(format!("row:learnProfile.{id}"), r.rect, &r);
            if r.clicked() {
                app.ui.learn_edits.selected = Some(id.to_string());
                app.ui.learn_edits.new = None;
                app.ui.learn_edits.ask_delete = false;
            }
        }
    });
    ui.horizontal(|ui| {
        if button(ui, "button:learnNew", tr("New Profile…"), true) {
            app.ui.learn_edits.new = Some(NewForm::default());
        }
        if button(ui, "button:learnImport", tr("Import…"), true) {
            let r = command(app, "learnEdits.import", &json!({}));
            if let Err(e) = r {
                app.ui.learn_edits.note = Some((e, true));
            }
        }
    });
}

fn groups_picker(ui: &mut egui::Ui, id: &str, groups: &mut Vec<Group>) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for g in Group::ALL {
            let mut on = groups.contains(&g);
            let r = ui.checkbox(&mut on, tr(g.label()));
            crate::widgets::register_response(format!("{id}.{}", g.id()), r.rect, &r);
            if r.changed() {
                changed = true;
                if on {
                    groups.push(g);
                } else {
                    groups.retain(|x| *x != g);
                }
            }
        }
    });
    groups.sort();
    changed
}

fn new_form(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, busy: bool) {
    let Some(mut form) = app.ui.learn_edits.new.clone() else { return };
    ui.label(RichText::new(tr("New Profile")).font(t.semibold(13.0)));
    crate::text_field::TextField::singleline("field:learnName", &mut form.name).hint(tr("Name")).label(tr("Name")).width(260.0).show(ui);
    crate::text_field::TextField::singleline("field:learnKind", &mut form.kind)
        .hint(tr("Kind of photography"))
        .label(tr("Kind"))
        .width(260.0)
        .show(ui);
    ui.horizontal_wrapped(|ui| {
        for k in KINDS {
            if ui.small_button(tr(k)).clicked() {
                form.kind = tr(k).to_string();
            }
        }
    });
    ui.label(RichText::new(tr("Learn")).color(t.text_label));
    groups_picker(ui, "check:learnNewGroup", &mut form.groups);
    let selected = app.session.selected().len();
    let shown = app.session.visible_cloned().len();
    ui.radio_value(&mut form.from_view, false, tr_format!("Selected photos ({})", selected));
    ui.radio_value(&mut form.from_view, true, tr_format!("Photos shown ({})", shown));
    let photos = if form.from_view { shown } else { selected };
    let ready = !form.name.trim().is_empty() && !form.groups.is_empty() && photos > 0 && !busy;
    let mut close = false;
    ui.horizontal(|ui| {
        if button(ui, "button:learnNewCancel", tr("Cancel"), true) {
            close = true;
        }
        if button(ui, "button:learnCreate", tr("Create and Train"), ready) {
            let ids: Vec<u64> = if form.from_view { app.session.visible_cloned() } else { app.session.selected() }.iter().map(|i| i.0).collect();
            let groups: Vec<&str> = form.groups.iter().map(|g| g.id()).collect();
            if let Some(v) = run(app, "learn.create", json!({"name": form.name.trim(), "kind": form.kind.trim(), "groups": groups, "ids": ids})) {
                app.ui.learn_edits.selected = v["id"].as_str().or(v["profile"]["id"].as_str()).map(str::to_string);
                close = true;
            }
        }
    });
    app.ui.learn_edits.new = if close { None } else { Some(form) };
}

/// A group's report as two lines: its learned sliders with their held-out error ("Exposure ±0.18 EV, Shadows ±0"),
/// and the sliders it skipped ("Contrast"). A colour wheel's two axes are one name.
pub(crate) fn group_lines(group: &Value) -> (Vec<String>, Vec<String>) {
    let group_kept = group["kept"].as_bool() == Some(true);
    let (mut learned, mut skipped) = (Vec::new(), Vec::new());
    let mut seen = Vec::new();
    for s in group["sliders"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let id = s["id"].as_str().unwrap_or_default();
        let Some(sl) = lightcraft_learn::sliders::find(id) else { continue };
        if seen.contains(&sl.label) {
            continue;
        }
        seen.push(sl.label);
        // reports saved before sliders were decided one by one carry no `learned`: the group decided
        if !s["learned"].as_bool().unwrap_or(group_kept) {
            skipped.push(tr(sl.label).to_string());
            continue;
        }
        let e = s["error"].as_f64().unwrap_or(0.0);
        learned.push(match id {
            "wb.temp" => format!("{} ±{:.0} mired", tr(sl.label), e),
            "light.exposure" => format!("{} ±{:.2} EV", tr(sl.label), e),
            _ => format!("{} ±{:.0}", tr(sl.label), e),
        });
    }
    (learned, skipped)
}

/// At most four names, then "and N more".
fn names(list: &[String]) -> String {
    if list.len() <= 4 {
        return list.join(", ");
    }
    let shown = list.get(..4).map(|l| l.join(", ")).unwrap_or_default();
    tr_format!("{} and {} more", shown, list.len() - 4)
}

fn accuracy(ui: &mut egui::Ui, t: &Tokens, p: &Value) {
    if let Some(problem) = p["problem"].as_str() {
        ui.label(RichText::new(problem).color(t.reject));
        return;
    }
    let r = &p["report"];
    if r.is_null() {
        ui.label(RichText::new(tr("Not trained yet.")).color(t.text_dim));
        return;
    }
    let n = r["examples"].as_u64().unwrap_or(0);
    ui.label(tr_format!("On {} photos it had not seen, it came closer to your edits than the defaults on {}.", n, r["beatsDefaults"]));
    ui.label(
        RichText::new(tr_format!(
            "Its error is {}% of leaving photos at their defaults; one fixed preset of your average edit beats it on {} of {}.",
            (r["errorVsDefaults"].as_f64().unwrap_or(1.0) * 100.0).round(),
            n.saturating_sub(r["beatsPreset"].as_u64().unwrap_or(0)),
            n
        ))
        .color(t.text_dim),
    );
    for g in r["groups"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let label = Group::from_id(g["group"].as_str().unwrap_or_default()).map(|g| tr(g.label()).to_string()).unwrap_or_default();
        let (learned, skipped) = group_lines(g);
        if learned.is_empty() {
            ui.label(RichText::new(tr_format!("{}: not learned, your edits there follow no pattern it can see", label)).color(t.text_dim));
            continue;
        }
        ui.label(format!("{label}: {}", names(&learned)));
        if !skipped.is_empty() {
            ui.label(RichText::new(tr_format!("{} skipped, no pattern: {}", label, names(&skipped))).color(t.text_dim));
        }
    }
}

fn details(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, p: &Value, busy: bool) {
    let Some(id) = p["id"].as_str().map(str::to_string) else { return };
    if let Some(e) = p["error"].as_str() {
        ui.label(RichText::new(tr_format!("This profile file can't be read: {}", e)).color(t.reject));
        delete_row(app, ui, &id, busy);
        return;
    }
    let w = &mut app.ui.learn_edits;
    if w.fields_of.as_deref() != Some(&id) {
        w.name = p["name"].as_str().unwrap_or_default().to_string();
        w.kind = p["kind"].as_str().unwrap_or_default().to_string();
        w.fields_of = Some(id.clone());
    }
    let (mut name, mut kind) = (w.name.clone(), w.kind.clone());
    let a = crate::text_field::TextField::singleline("field:learnProfileName", &mut name).label(tr("Name")).width(260.0).show(ui);
    let b = crate::text_field::TextField::singleline("field:learnProfileKind", &mut kind)
        .hint(tr("Kind of photography"))
        .label(tr("Kind"))
        .width(260.0)
        .show(ui);
    (app.ui.learn_edits.name, app.ui.learn_edits.kind) = (name.clone(), kind.clone());
    if (a.committed() || b.committed())
        && !name.trim().is_empty()
        && (Some(name.as_str()) != p["name"].as_str() || Some(kind.as_str()) != p["kind"].as_str())
    {
        run(app, "learn.update", json!({"profile": id, "name": name.trim(), "kind": kind.trim()}));
    }
    ui.label(RichText::new(tr("Learns")).color(t.text_label));
    let mut groups: Vec<Group> =
        p["groups"].as_array().map(Vec::as_slice).unwrap_or_default().iter().filter_map(|g| g.as_str().and_then(Group::from_id)).collect();
    if groups_picker(ui, "check:learnGroup", &mut groups) && !groups.is_empty() && !busy {
        let ids: Vec<&str> = groups.iter().map(|g| g.id()).collect();
        run(app, "learn.update", json!({"profile": id, "groups": ids}));
    }
    ui.add_space(6.0);
    accuracy(ui, t, p);
    ui.add_space(6.0);
    let selected = app.session.selected().len();
    let trained = p["trained"].as_bool() == Some(true);
    ui.horizontal_wrapped(|ui| {
        if button(ui, "button:learnApply", &tr_format!("Apply to Selected Photos ({})", selected), trained && selected > 0 && !busy) {
            run(app, "learn.apply", json!({"profile": id}));
        }
        if button(ui, "button:learnAddExamples", &tr_format!("Add Selected Photos as Examples ({})", selected), selected > 0 && !busy) {
            run(app, "learn.addExamples", json!({"profile": id}));
        }
        if button(ui, "button:learnTrain", tr("Train Again"), p["examples"].as_u64().unwrap_or(0) > 0 && !busy) {
            run(app, "learn.train", json!({"profile": id}));
        }
    });
    ui.label(RichText::new(tr_format!("{} examples", p["examples"])).color(t.text_dim));
    ui.horizontal(|ui| {
        if button(ui, "button:learnExport", tr("Export…"), true)
            && let Err(e) = command(app, "learnEdits.export", &json!({"profile": id}))
        {
            app.ui.learn_edits.note = Some((e, true));
        }
    });
    delete_row(app, ui, &id, busy);
}

fn delete_row(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, busy: bool) {
    ui.horizontal(|ui| {
        if app.ui.learn_edits.ask_delete {
            ui.label(tr("Delete this profile?"));
            if button(ui, "button:learnDeleteCancel", tr("Cancel"), true) {
                app.ui.learn_edits.ask_delete = false;
            }
            let red = egui::Button::new(RichText::new(tr("Delete")).color(Color32::WHITE)).fill(Tokens::get(ui.ctx()).reject);
            let r = ui.add_enabled(!busy, red);
            crate::widgets::register_response("button:learnDeleteConfirm", r.rect, &r);
            if r.clicked() {
                app.ui.learn_edits.ask_delete = false;
                if run(app, "learn.delete", json!({"profile": id})).is_some() {
                    app.ui.learn_edits.selected = None;
                }
            }
        } else if button(ui, "button:learnDelete", tr("Delete…"), !busy) {
            app.ui.learn_edits.ask_delete = true;
        }
    });
}
