//! Fork (issue #329, `fork/mask-presets.md`): the Mask Presets list of the Masking panel. One row for each preset of
//! your own with its name (wrapped, never cut) and what it holds; a click adds its mask to the photo
//! (`mask.applyPreset`, one undo step). Rename… and Delete Preset are in the row's menu. Save Mask as Preset… (the
//! button here and the item in a mask's menu) names the selected mask (`mask.savePreset`).

use egui::{Rect, Sense, pos2, vec2};
use lightcraft_develop::mask_presets::MaskPreset;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::register;

/// A row's least height (points) and the height of its second line.
const ROW_H: f32 = 40.0;
const SUB_H: f32 = 14.0;

/// What the list remembers between frames: a name being typed for a new preset (and the mask it is for), and a rename.
#[derive(Clone, Default)]
struct Edit {
    naming: Option<(u32, String)>,
    renaming: Option<(String, String)>,
}

fn state_id() -> egui::Id {
    egui::Id::unique("maskPresetsEdit")
}

/// Start naming a preset of mask `mask` (the mask menu's Save Mask as Preset…): the list opens with its name field.
pub fn start_naming(app: &mut LightcraftApp, ctx: &egui::Context, mask: u32) {
    if !app.ui.flyout_open("maskPresets") {
        app.ui.toggle_flyout("maskPresets");
    }
    let mut edit: Edit = ctx.data(|x| x.get_temp(state_id())).unwrap_or_default();
    edit.naming = Some((mask, String::new()));
    ctx.data_mut(|x| {
        x.insert_temp(state_id(), edit);
        x.remove::<bool>(state_id().with("named"));
    });
}

/// What a preset holds, in a line: its parts' kinds, and that it has an effect.
fn what(p: &MaskPreset) -> String {
    let label = |k: &str| {
        crate::i18n::tr(match k {
            "subject" => "Subject",
            "sky" => "Sky",
            "background" => "Background",
            "object" => "Object",
            "prompt" => "Describe",
            "people" => "People",
            "brush" => "Brush",
            "linear" => "Linear",
            "radial" => "Radial",
            "luminanceRange" => "Luminance",
            "colorRange" => "Color",
            "depthRange" => "Depth",
            _ => "Mask",
        })
        .to_string()
    };
    p.kinds().iter().map(|k| label(k)).collect::<Vec<_>>().join(" + ")
}

/// The list's header row and, open, its rows and the Save button.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let open = app.ui.flyout_open("maskPresets");
    if crate::widgets::flyout_row(ui, "maskPresets", "Mask Presets", Icon::Presets, open).clicked() {
        app.ui.toggle_flyout("maskPresets");
    }
    let ctx = ui.ctx().clone();
    let mut edit: Edit = ctx.data(|x| x.get_temp(state_id())).unwrap_or_default();
    if open {
        let list = app.session.mask_presets.clone();
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 8 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if list.is_empty() && edit.naming.is_none() {
                let t = Tokens::get(ui.ctx());
                ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tr("No Mask Presets Yet")).size(11.0).color(t.text_dim)).wrap());
            }
            for p in &list {
                row(app, ui, p, &mut edit);
            }
            ui.add_space(4.0);
            save_box(app, ui, &mut edit);
        });
    } else {
        edit.naming = None;
        edit.renaming = None;
    }
    ctx.data_mut(|x| x.insert_temp(state_id(), edit));
}

/// Apply preset `preset` (`mask.applyPreset`), and say what a model could not find.
fn apply(app: &mut LightcraftApp, ctx: &egui::Context, preset: &str) {
    match app.run("mask.applyPreset", json!({"preset": preset})) {
        Ok(r) => {
            if let Some(why) = r["missing"].as_array().and_then(|a| a.first()).and_then(|w| w.as_str()) {
                app.toast(ctx, crate::i18n::tr_format!("Mask added, but a part could not be found: {why}", why = why));
            } else if r["foundNothing"].as_array().is_some_and(|a| !a.is_empty()) {
                app.toast(ctx, crate::i18n::tr("Mask added, but nothing was found in this photo for one of its parts"));
            }
        }
        Err(e) => app.toast(ctx, e),
    }
}

/// Draws one list item: a name (wrapped, never cut) and a label under it (what the preset holds); a mask preset has no
/// picture. Returns the item's rect and its response, and the rect where a rename field goes while `renaming`. Only
/// the drawing is here (the click, the menu and the rename are in [`row`]), so the item can become a card of the shared
/// preset grid (issue #344: picture on top, name and label under it) by replacing this function.
fn item(ui: &mut egui::Ui, p: &MaskPreset, renaming: bool) -> (Rect, egui::Response, Rect) {
    let t = Tokens::get(ui.ctx());
    let wrap_w = (ui.available_width() - 12.0).max(40.0);
    let galley = ui.painter().layout(p.name.clone(), t.font(13.0), t.text_label, wrap_w);
    let sub = ui.painter().layout(what(p), t.font(11.0), t.text_dim, wrap_w);
    let row_h = ROW_H.max(galley.size().y + sub.size().y.max(SUB_H) + 16.0);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
    crate::widgets::announce(&resp, p.name.clone());
    register(ui, format!("maskPreset:{}", p.id), r);
    if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, t.hover.gamma_multiply(0.7));
    }
    if !renaming {
        let below = r.top() + 8.0 + galley.size().y + 4.0;
        ui.painter().galley(pos2(r.left() + 6.0, r.top() + 8.0), galley, t.text_label);
        ui.painter().galley(pos2(r.left() + 6.0, below), sub, t.text_dim);
    }
    let field = Rect::from_min_max(pos2(r.left() + 6.0, r.top() + 6.0), pos2(r.right() - 6.0, r.top() + 30.0));
    (r, resp, field)
}

/// One preset's row: [`item`], a click that applies it, its menu (Rename…, Delete Preset) and the rename field.
fn row(app: &mut LightcraftApp, ui: &mut egui::Ui, p: &MaskPreset, edit: &mut Edit) {
    let renaming = edit.renaming.as_ref().is_some_and(|(id, _)| *id == p.id);
    let (_, resp, field) = item(ui, p, renaming);
    if renaming {
        rename_field(app, ui, p, field, edit);
    }
    if resp.clicked() && !renaming {
        let ctx = ui.ctx().clone();
        apply(app, &ctx, &p.id);
    }
    crate::widgets::context_menu(&resp, |ui| {
        if ui.button(crate::i18n::tr("Rename…")).clicked() {
            edit.renaming = Some((p.id.clone(), p.name.clone()));
            ui.close();
        }
        if ui.button(crate::i18n::tr("Delete Preset")).clicked() {
            if let Err(e) = app.run("mask.deletePreset", json!({"preset": p.id})) {
                app.toast(ui.ctx(), e);
            }
            ui.close();
        }
    });
}

/// The name of a preset being typed in its row.
fn rename_field(app: &mut LightcraftApp, ui: &mut egui::Ui, p: &MaskPreset, rect: Rect, edit: &mut Edit) {
    let Some((id, mut text)) = edit.renaming.take() else { return };
    let list = app.session.mask_presets.clone();
    let verdict = crate::inline_rename::verdict(&text, list.iter().filter(|q| q.id != id).map(|q| q.name.as_str()));
    let unchanged = text.trim() == p.name;
    let done = crate::inline_rename::edit(ui, &format!("field:maskPresetRename:{id}"), &mut text, rect, &verdict, unchanged);
    let ctx = ui.ctx().clone();
    let mut keep = None;
    crate::inline_rename::finish_with(app, &ctx, done, "mask.renamePreset", json!({"preset": id}), "name", text, |_, still| keep = still);
    edit.renaming = keep.map(|t| (id, t));
}

/// Save Mask as Preset…: a button (on while a mask is selected), then a name field with Save.
fn save_box(app: &mut LightcraftApp, ui: &mut egui::Ui, edit: &mut Edit) {
    let t = Tokens::get(ui.ctx());
    let Some((mask, mut text)) = edit.naming.take() else {
        let selected = app.session.active_mask;
        ui.add_enabled_ui(selected.is_some(), |ui| {
            if crate::widgets::text_button(ui, "maskSavePreset", crate::i18n::tr("Save Mask as Preset…"), false).clicked()
                && let Some(m) = selected
            {
                edit.naming = Some((m, String::new()));
            }
        });
        return;
    };
    let first = !ui.ctx().data(|x| x.get_temp::<bool>(state_id().with("named"))).unwrap_or(false);
    ui.ctx().data_mut(|x| x.insert_temp(state_id().with("named"), true));
    let verdict = crate::inline_rename::verdict(&text, app.session.mask_presets.iter().map(|q| q.name.as_str()));
    let w = ui.available_width();
    let field = crate::text_field::TextField::singleline("field:maskPresetName", &mut text)
        .hint(crate::i18n::tr("Preset Name"))
        .label(crate::i18n::tr("Preset Name"))
        .width(w)
        .begin_edit(first)
        .show(ui);
    let mut save = field.ending == Some(crate::text_field::Ending::Return);
    let mut cancel = field.cancelled();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        save |= crate::widgets::text_button(ui, "maskSavePresetOk", crate::i18n::tr("Save"), false).clicked();
        cancel |= crate::widgets::text_button(ui, "maskSavePresetCancel", crate::i18n::tr("Cancel"), false).clicked();
    });
    if let Err(why) = &verdict
        && !text.trim().is_empty()
    {
        ui.label(egui::RichText::new(why.as_str()).size(11.0).color(t.text_dim));
    }
    if save {
        match app.run("mask.savePreset", json!({"id": mask, "name": text})) {
            Ok(_) => cancel = true,
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    if cancel {
        ui.ctx().data_mut(|x| x.remove::<bool>(state_id().with("named")));
    } else {
        edit.naming = Some((mask, text));
    }
}
