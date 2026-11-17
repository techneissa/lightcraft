//! Fork (issue #301, `fork/relight.md`): the Light Presets flyout of the Relight section. One row for each preset (the
//! built-in ones, then your own) with a small picture of the current photo lit by it; resting on a row previews it on
//! the photo, a click applies it (`relight.applyPreset`, one undo step). Your own presets rename and delete from the row's
//! menu; Save Lights as Preset… names the photo's lights (`relight.savePreset`).

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight_presets::LightPreset;
use serde_json::json;

use super::super::{Icon, LightcraftApp, Tokens, register};
use crate::HoverPreview;

/// The long edge of a row's picture (pixels).
const THUMB_EDGE: usize = 128;
/// A row's least height (points), where its text starts from the row's left edge, and the height of its "n lights" line.
const ROW_H: f32 = 52.0;
const TEXT_X: f32 = 4.0 + 66.0 + 10.0;
const SUB_H: f32 = 14.0;

/// What the flyout remembers between frames: the name being typed for a new preset, and a rename under way.
#[derive(Clone, Default)]
struct Edit {
    naming: Option<String>,
    renaming: Option<(String, String)>,
    /// A preset clicked while the photo's depth was still being found: applied once it is.
    pending: Option<(PhotoId, String)>,
}

fn state_id() -> egui::Id {
    egui::Id::unique("relightPresetsEdit")
}

/// The flyout's header row and, open, its rows and the Save button.
pub(super) fn presets(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, id: PhotoId) {
    let open = app.ui.flyout_open("relightPresets");
    if crate::widgets::flyout_row(ui, "relightPresets", "Light Presets", Icon::Presets, open).clicked() {
        app.ui.toggle_flyout("relightPresets");
    }
    let ctx = ui.ctx().clone();
    let mut edit: Edit = ctx.data(|x| x.get_temp(state_id())).unwrap_or_default();
    apply_pending(app, &ctx, &mut edit, id);
    if !open {
        ctx.data_mut(|x| x.insert_temp(state_id(), edit));
        return;
    }
    let subject = app.session.relight_subject(id).0;
    let list = app.session.relight_preset_list();
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 4 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let mut user_label = false;
        for p in &list {
            if !p.builtin && !user_label {
                user_label = true;
                let t = Tokens::get(ui.ctx());
                ui.add_space(4.0);
                ui.label(egui::RichText::new(crate::i18n::tr("My Presets")).size(11.0).color(t.text_dim));
            }
            row(app, ui, d, id, p, &p.onto(d, &subject), &mut edit);
        }
        ui.add_space(4.0);
        save_box(app, ui, d, &mut edit);
    });
    ctx.data_mut(|x| x.insert_temp(state_id(), edit));
}

/// Apply preset `preset` (`relight.applyPreset`); when the photo's depth is still being found nothing is applied and it
/// is applied once the depth is there (the top-bar ring shows the finding).
fn apply(app: &mut LightcraftApp, ctx: &egui::Context, edit: &mut Edit, photo: PhotoId, preset: &str) {
    match app.run("relight.applyPreset", json!({"preset": preset})) {
        Ok(r) if r["finding"].as_bool() == Some(true) => {
            edit.pending = Some((photo, preset.to_string()));
            app.toast(ctx, crate::i18n::tr("Finding the photo's depth… the preset applies when it is found"));
        }
        Ok(_) => {}
        Err(e) => app.toast(ctx, e),
    }
}

/// The preset clicked while the depth was being found, once it is (or can't be): applied then; dropped on another photo.
fn apply_pending(app: &mut LightcraftApp, ctx: &egui::Context, edit: &mut Edit, photo: PhotoId) {
    let Some((on, preset)) = edit.pending.clone() else { return };
    edit.pending = None;
    if on != photo {
        return;
    }
    if app.session.relight_status(photo)["finding"].as_bool() == Some(true) {
        edit.pending = Some((on, preset));
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    apply(app, ctx, edit, photo, &preset);
}

/// One preset's row: its picture, its name and how many lights it sets.
fn row(app: &mut LightcraftApp, ui: &mut egui::Ui, _d: &DevelopSettings, photo: PhotoId, p: &LightPreset, look: &DevelopSettings, edit: &mut Edit) {
    let t = Tokens::get(ui.ctx());
    // (the name wraps, whatever its length: a long one makes the row taller)
    let name = if p.builtin { crate::i18n::builtin_label(&p.name, true).to_string() } else { p.name.clone() };
    let wrap_w = (ui.available_width() - TEXT_X - 6.0).max(40.0);
    let galley = ui.painter().layout(name, t.font(13.0), t.text_label, wrap_w);
    let row_h = ROW_H.max(galley.size().y + SUB_H + 18.0);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
    crate::widgets::announce(&resp, p.name.clone());
    register(ui, format!("relight:preset:{}", p.id), r);
    let renaming = edit.renaming.as_ref().is_some_and(|(id, _)| *id == p.id);
    if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, t.hover.gamma_multiply(0.7));
    }
    let tr = Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 4.0), vec2(66.0, ROW_H - 8.0));
    ui.painter().rect_filled(tr, 2.0, t.canvas);
    if let Some(job) = app.session.variant_job(photo, look, THUMB_EDGE)
        && let Some(tex) = app.renderer.variant(job)
    {
        ui.painter().image(tex.id(), tr, crate::panels::presets::cover_uv(tr, tex.size), Color32::WHITE);
    }
    let x = r.left() + TEXT_X;
    if renaming {
        let field = Rect::from_min_max(pos2(x, r.top() + 6.0), pos2(r.right() - 6.0, r.top() + 30.0));
        rename_field(app, ui, p, field, edit);
    } else {
        let below = r.top() + 8.0 + galley.size().y + 4.0;
        ui.painter().galley(pos2(x, r.top() + 8.0), galley, t.text_label);
        let n = p.lights.lights.len();
        let sub = crate::i18n::tr_format!("{n} light{s}", n = n, s = crate::i18n::plural_s(n));
        ui.painter().text(pos2(x, below), Align2::LEFT_TOP, sub, t.font(11.0), t.text_dim);
    }
    // resting on a preset shows it on the photo (no undo step)
    if resp.hovered() && !renaming {
        let label = crate::i18n::tr_format!("Light Preset: {name}", name = p.name);
        app.hover_preview = Some(HoverPreview { label, settings: look.clone() });
    }
    if resp.clicked() && !renaming {
        let ctx = ui.ctx().clone();
        apply(app, &ctx, edit, photo, &p.id);
    }
    if !p.builtin {
        crate::widgets::context_menu(&resp, |ui| {
            if ui.button(crate::i18n::tr("Rename…")).clicked() {
                edit.renaming = Some((p.id.clone(), p.name.clone()));
                ui.close();
            }
            if ui.button(crate::i18n::tr("Delete Preset")).clicked() {
                if let Err(e) = app.run("relight.deletePreset", json!({"preset": p.id})) {
                    app.toast(ui.ctx(), e);
                }
                ui.close();
            }
        });
    }
}

/// The name of a preset of your own being typed in its row.
fn rename_field(app: &mut LightcraftApp, ui: &mut egui::Ui, p: &LightPreset, rect: Rect, edit: &mut Edit) {
    let Some((id, mut text)) = edit.renaming.take() else { return };
    let list = app.session.relight_preset_list();
    let verdict = crate::inline_rename::verdict(&text, list.iter().filter(|q| q.id != id).map(|q| q.name.as_str()));
    let unchanged = text.trim() == p.name;
    let done = crate::inline_rename::edit(ui, &format!("field:relightPresetRename:{id}"), &mut text, rect, &verdict, unchanged);
    let ctx = ui.ctx().clone();
    let mut keep = None;
    crate::inline_rename::finish_with(app, &ctx, done, "relight.renamePreset", json!({"preset": id}), "name", text, |_, still| keep = still);
    edit.renaming = keep.map(|t| (id, t));
}

/// Save Lights as Preset…: a button (on while some light is on), then a name field with Save.
fn save_box(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, edit: &mut Edit) {
    let t = Tokens::get(ui.ctx());
    let Some(mut text) = edit.naming.take() else {
        let on = d.relight.is_on();
        ui.add_enabled_ui(on, |ui| {
            if crate::widgets::text_button(ui, "relightSavePreset", crate::i18n::tr("Save Lights as Preset…"), false).clicked() {
                edit.naming = Some(String::new());
            }
        });
        return;
    };
    let first = !ui.ctx().data(|x| x.get_temp::<bool>(state_id().with("named"))).unwrap_or(false);
    ui.ctx().data_mut(|x| x.insert_temp(state_id().with("named"), true));
    let list = app.session.relight_preset_list();
    let verdict = crate::inline_rename::verdict(&text, list.iter().map(|q| q.name.as_str()));
    let w = ui.available_width();
    let field = crate::text_field::TextField::singleline("field:relightPresetName", &mut text)
        .hint(crate::i18n::tr("Preset Name"))
        .label(crate::i18n::tr("Preset Name"))
        .width(w)
        .begin_edit(first)
        .show(ui);
    let mut save = field.ending == Some(crate::text_field::Ending::Return);
    let mut cancel = field.cancelled();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        save |= crate::widgets::text_button(ui, "relightSavePresetOk", crate::i18n::tr("Save"), false).clicked();
        cancel |= crate::widgets::text_button(ui, "relightSavePresetCancel", crate::i18n::tr("Cancel"), false).clicked();
    });
    if let Err(why) = &verdict
        && !text.trim().is_empty()
    {
        ui.label(egui::RichText::new(why.as_str()).size(11.0).color(t.text_dim));
    }
    if save {
        match app.run("relight.savePreset", json!({"name": text})) {
            Ok(_) => cancel = true,
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    if cancel {
        ui.ctx().data_mut(|x| x.remove::<bool>(state_id().with("named")));
    } else {
        edit.naming = Some(text);
    }
}
