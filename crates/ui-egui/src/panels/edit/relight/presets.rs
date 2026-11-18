//! Fork (issue #301, `fork/relight.md`): the Light Presets flyout of the Relight section. One card for each preset (the
//! built-in ones, then your own; a grid that follows the panel's width, `widgets/preset_grid.rs`, issue #344) with a
//! picture of the current photo lit by it; resting on a card previews it on the photo, a click applies it (`relight.applyPreset`, one undo step). Your own presets rename and delete from the row's
//! menu; Save Lights as Preset… names the photo's lights (`relight.savePreset`). Every card's menu and a small Add button
//! on the card under the pointer add the preset's lights to the photo's instead (`relight.applyPreset` with `add`), and a
//! light's own menu names one light as a preset (`relight.saveLightPreset`, [`begin_save_light`]).

use egui::{Align2, Rect, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight_presets::{LightPreset, Subject};
use serde_json::json;

use super::super::{Icon, LightcraftApp, Tokens};
use crate::HoverPreview;
use crate::widgets::preset_grid::{Card, Grid, Item};

/// The long edge of a row's picture (pixels).
const THUMB_EDGE: usize = 128;
/// The size of a card's Add button.
const ADD_SIZE: egui::Vec2 = vec2(20.0, 18.0);

/// What the flyout remembers between frames: the name being typed for a new preset, and a rename under way.
#[derive(Clone, Default)]
struct Edit {
    naming: Option<String>,
    /// The name being typed is for this one light (Save This Light as Preset…), not for all the photo's lights: the photo
    /// and library it was started on, and the light's number. Dropped when the photo or the library changes.
    light: Option<(PhotoId, u64, usize)>,
    renaming: Option<(String, String)>,
    /// A preset clicked while the photo's depth was still being found, and whether it is added: applied once it is.
    pending: Option<(PhotoId, String, u64, bool)>,
}

fn state_id() -> egui::Id {
    egui::Id::unique("relightPresetsEdit")
}

/// Save This Light as Preset… on light `index`'s menu: opens the flyout with the name box for that light.
pub(super) fn begin_save_light(app: &mut LightcraftApp, ctx: &egui::Context, photo: PhotoId, index: usize) {
    if !app.ui.flyout_open("relightPresets") {
        app.ui.toggle_flyout("relightPresets");
    }
    let mut edit: Edit = ctx.data(|x| x.get_temp(state_id())).unwrap_or_default();
    (edit.naming, edit.light) = (Some(String::new()), Some((photo, app.session.library_generation(), index)));
    ctx.data_mut(|x| x.remove::<bool>(state_id().with("named")));
    ctx.data_mut(|x| x.insert_temp(state_id(), edit));
}

/// The flyout's header row and, open, its cards and the Save button.
pub(super) fn presets(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, id: PhotoId) {
    let open = app.ui.flyout_open("relightPresets");
    if crate::widgets::flyout_row(ui, "relightPresets", "Light Presets", Icon::Presets, open).clicked() {
        app.ui.toggle_flyout("relightPresets");
    }
    let ctx = ui.ctx().clone();
    let mut edit: Edit = ctx.data(|x| x.get_temp(state_id())).unwrap_or_default();
    apply_pending(app, &ctx, &mut edit, id);
    // (a name being typed for a light belongs to the photo it was started on: another photo's light of that number is not it)
    if edit.light.is_some_and(|(on, library, _)| !same_target(on, library, id, app.session.library_generation())) {
        (edit.naming, edit.light) = (None, None);
        ctx.data_mut(|x| x.remove::<bool>(state_id().with("named")));
    }
    if !open {
        ctx.data_mut(|x| x.insert_temp(state_id(), edit));
        return;
    }
    let subject = app.session.relight_subject(id).0;
    let list = app.session.relight_preset_list();
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 14, top: 4, bottom: 4 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let (builtin, own): (Vec<&LightPreset>, Vec<&LightPreset>) = list.iter().partition(|p| p.builtin);
        cards(app, ui, (d, &subject), id, &builtin, "builtin", &mut edit);
        if !own.is_empty() {
            let t = Tokens::get(ui.ctx());
            ui.add_space(4.0);
            ui.label(egui::RichText::new(crate::i18n::tr("My Presets")).size(11.0).color(t.text_dim));
            cards(app, ui, (d, &subject), id, &own, "own", &mut edit);
        }
        ui.add_space(4.0);
        save_box(app, ui, d, &mut edit);
    });
    ctx.data_mut(|x| x.insert_temp(state_id(), edit));
}

/// Apply preset `preset` (`relight.applyPreset`), or with `add` add its lights to the photo's; when the photo's depth is
/// still being found nothing is applied and it is applied once the depth is there (the top-bar ring shows the finding).
fn apply(app: &mut LightcraftApp, ctx: &egui::Context, edit: &mut Edit, photo: PhotoId, preset: &str, add: bool) {
    match app.run("relight.applyPreset", json!({"preset": preset, "add": add})) {
        Ok(r) if r["finding"].as_bool() == Some(true) => {
            edit.pending = Some((photo, preset.to_string(), app.session.library_generation(), add));
            app.toast(ctx, crate::i18n::tr("Finding the photo's depth… the preset applies when it is found"));
        }
        Ok(r) => {
            // (adding to a photo with room for some: what was left out is said)
            let left = r["leftOut"].as_u64().unwrap_or(0);
            if left > 0 {
                let max = lightcraft_develop::relight::MAX_LIGHTS;
                app.toast(
                    ctx,
                    crate::i18n::tr_format!("{n} of the preset's lights left out: Relight holds at most {max} lights", n = left, max = max),
                );
            }
        }
        Err(e) => app.toast(ctx, e),
    }
}

/// Whether something started on photo `on` of library `library` is still about the photo `id` of library `now`.
fn same_target(on: PhotoId, library: u64, id: PhotoId, now: u64) -> bool {
    on == id && library == now
}

/// The preset clicked while the depth was being found, once it is (or can't be): applied then; dropped on another photo.
fn apply_pending(app: &mut LightcraftApp, ctx: &egui::Context, edit: &mut Edit, photo: PhotoId) {
    let Some((on, preset, library, add)) = edit.pending.clone() else { return };
    edit.pending = None;
    // (another photo, or another library whose active photo has the same id, is not the one clicked on)
    if on != photo || library != app.session.library_generation() {
        return;
    }
    if app.session.relight_status(photo)["finding"].as_bool() == Some(true) {
        edit.pending = Some((on, preset, library, add));
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    apply(app, ctx, edit, photo, &preset, add);
}

/// The line under a preset's name: how many lights it sets (a single light says so), and Underwater when it sets that.
fn sub_text(p: &LightPreset) -> String {
    let n = p.lights.lights.len();
    let mut parts = vec![if p.is_single() {
        crate::i18n::tr("Single Light").to_string()
    } else {
        crate::i18n::tr_format!("{n} light{s}", n = n, s = crate::i18n::plural_s(n))
    }];
    if p.has_water() {
        parts.push(crate::i18n::tr("Underwater").to_string());
    }
    parts.join(" · ")
}

/// The cards of `presets`: each one's picture, its name and what it sets (how many lights, Underwater).
fn cards(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    (d, subject): (&DevelopSettings, &Subject),
    photo: PhotoId,
    presets: &[&LightPreset],
    salt: &str,
    edit: &mut Edit,
) {
    let items: Vec<Item> = presets
        .iter()
        .map(|p| Item {
            key: format!("relight:preset:{}", p.id),
            // (the name wraps, whatever its length: a long one makes the card taller)
            name: if p.builtin { crate::i18n::builtin_label(&p.name, true).to_string() } else { p.name.clone() },
            label: sub_text(p),
            applied: false,
            dim: false,
            editing: edit.renaming.as_ref().is_some_and(|(id, _)| *id == p.id),
        })
        .collect();
    Grid::new(("relightPresets", salt)).show(ui, &items, |ui, i, card| {
        if let Some(p) = presets.get(i) {
            card_actions(app, ui, photo, p, &p.onto(d, subject), card, items.get(i).is_some_and(|it| it.editing), edit);
        }
    });
}

/// What a card does: shows its look as the picture, previews it on the photo while the pointer rests on it, applies it on
/// a click, adds its lights from the Add button, and opens its menu.
#[allow(clippy::too_many_arguments)]
fn card_actions(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    photo: PhotoId,
    p: &LightPreset,
    look: &DevelopSettings,
    card: &Card,
    renaming: bool,
    edit: &mut Edit,
) {
    let t = Tokens::get(ui.ctx());
    let resp = &card.resp;
    if let Some(job) = app.session.variant_job(photo, look, THUMB_EDGE)
        && let Some(tex) = app.renderer.variant(job)
    {
        card.paint_picture(ui, tex.id(), tex.size);
    }
    if renaming {
        let field = Rect::from_min_size(card.text.min, vec2(card.text.width(), 24.0));
        rename_field(app, ui, p, field, edit);
    }
    // resting on a preset shows it on the photo (no undo step)
    if resp.hovered() && !renaming {
        let label = crate::i18n::tr_format!("Light Preset: {name}", name = p.name);
        app.hover_preview = Some(HoverPreview { label, settings: look.clone() });
    }
    // (a small Add button on the card under the pointer, over the card's own click)
    let mut add = false;
    if (resp.hovered() || resp.contains_pointer()) && !renaming {
        let btn = card.corner(ADD_SIZE, 4.0);
        let br = ui.interact(btn, resp.id.with("add"), egui::Sense::click());
        crate::widgets::register(ui, format!("relight:presetAdd:{}", p.id), btn);
        let fill = if br.hovered() { t.pressed } else { t.inset };
        ui.painter().rect_filled(btn, 4.0, fill);
        ui.painter().text(btn.center(), Align2::CENTER_CENTER, "+", t.font(13.0), t.text);
        let br = br.on_hover_text(crate::i18n::tr("Add to Lights"));
        add = br.clicked();
    }
    if (resp.clicked() && !renaming && !add) || add {
        let ctx = ui.ctx().clone();
        apply(app, &ctx, edit, photo, &p.id, add);
    }
    crate::widgets::context_menu(resp, |ui| {
        if ui.button(crate::i18n::tr("Add to Lights")).clicked() {
            let ctx = ui.ctx().clone();
            apply(app, &ctx, edit, photo, &p.id, true);
            ui.close();
        }
        if !p.builtin {
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
        }
    });
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
    let light = edit.light;
    let Some(mut text) = edit.naming.take() else {
        let on = d.relight.is_on();
        ui.add_enabled_ui(on, |ui| {
            if crate::widgets::text_button(ui, "relightSavePreset", crate::i18n::tr("Save Lights as Preset…"), false).clicked() {
                (edit.naming, edit.light) = (Some(String::new()), None);
            }
        });
        return;
    };
    let first = !ui.ctx().data(|x| x.get_temp::<bool>(state_id().with("named"))).unwrap_or(false);
    ui.ctx().data_mut(|x| x.insert_temp(state_id().with("named"), true));
    let list = app.session.relight_preset_list();
    let verdict = crate::inline_rename::verdict(&text, list.iter().map(|q| q.name.as_str()));
    if let Some((_, _, i)) = light {
        let line = crate::i18n::tr_format!("Preset of Light {n}", n = i + 1);
        ui.label(egui::RichText::new(line).size(11.0).color(t.text_dim));
    }
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
        let run = match light {
            Some((_, _, i)) => app.run("relight.saveLightPreset", json!({"name": text, "index": i})),
            None => app.run("relight.savePreset", json!({"name": text})),
        };
        match run {
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
