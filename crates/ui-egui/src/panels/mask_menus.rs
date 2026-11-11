//! The right-click menus of the Masking panel and the submask selection (fork feature, issue #61): a mask's menu
//! combines (Add to Mask, Subtract from Mask, Intersect Mask with), cleans up (Delete Empty Masks, Delete All Masks)
//! and names the mask in its items; a submask's menu acts on every selected submask. Every item runs a command.

use egui::vec2;
use lightcraft_develop::{Mask, MaskOp};
use serde_json::{Value, json};

use super::mask_parts::{add_part, op_name};
use super::masking::menu_label;
use crate::LightcraftApp;
use crate::widgets::register;

/// What the panel remembers besides the engine's one selected part: the submasks selected together (⌘-click, ⇧-click)
/// in mask `mask`, and whether Delete All Masks is asking. The set is trusted only while the photo, the engine's selected
/// part and the number of submasks are still what they were when it was made: an edit that moves indexes (a delete, an
/// undo, another photo) makes it stale, and the selection falls back to the engine's one submask.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaskUi {
    pub mask: u32,
    pub parts: Vec<usize>,
    pub photo: Option<lightcraft_catalog::PhotoId>,
    pub active: usize,
    pub count: usize,
    pub confirm_delete_all: bool,
}

/// The kinds of submask in the Add to Mask menus, grouped like the panel's buttons: id, label.
const GROUPS: [&[(&str, &str)]; 3] = [
    &[("subject", "Subject"), ("sky", "Sky"), ("background", "Background"), ("object", "Object"), ("prompt", "Describe…")],
    &[("brush", "Brush"), ("linear", "Linear Gradient"), ("radial", "Radial Gradient")],
    &[("colorRange", "Color Range"), ("luminanceRange", "Luminance Range")],
];

/// The submasks of mask `mask` (`count` of them) that are selected, sorted: the engine's selected one, with the ones
/// ⌘-clicked or ⇧-clicked along with it. Empty when no submask is selected.
pub fn selected_parts(app: &LightcraftApp, mask: u32, count: usize) -> Vec<usize> {
    let Some(active) = app.session.active_mask.filter(|m| *m == mask).and_then(|_| app.session.active_part()) else { return Vec::new() };
    let ui = &app.ui.mask_ui;
    let fresh = ui.mask == mask && ui.photo == app.session.active() && ui.active == active && ui.count == count;
    let mut parts: Vec<usize> = if fresh { ui.parts.iter().copied().filter(|k| *k < count).collect() } else { Vec::new() };
    if !parts.contains(&active) {
        parts = vec![active];
    }
    parts.sort_unstable();
    parts.dedup();
    parts
}

/// Select exactly `parts` of mask `mask`, `last` being the one the Brush and the mode control follow.
pub fn select_parts(app: &mut LightcraftApp, mask: u32, parts: Vec<usize>, last: usize) {
    let _ = app.run("mask.select", json!({"id": mask, "component": last}));
    remember(app, mask, parts);
}

/// Remember `parts` of mask `mask` as the set selected together, with what makes it stale (see [`MaskUi`]). Call it
/// after the engine's selected part is set.
pub fn remember(app: &mut LightcraftApp, mask: u32, parts: Vec<usize>) {
    let count = app
        .session
        .active()
        .and_then(|p| app.session.develop_of(p))
        .and_then(|d| d.masks.iter().find(|m| m.id == mask).map(|m| m.components.len()))
        .unwrap_or(0);
    app.ui.mask_ui = MaskUi {
        mask,
        parts,
        photo: app.session.active(),
        active: app.session.active_part().unwrap_or(usize::MAX),
        count,
        confirm_delete_all: app.ui.mask_ui.confirm_delete_all,
    };
}

/// A menu item that runs command `cmd` with `p` and closes the menu.
fn run(app: &mut LightcraftApp, ui: &mut egui::Ui, label: &str, enabled: bool, cmd: &str, p: Value) {
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        let _ = app.run(cmd, p);
        ui.close();
    }
}

/// The right-click menu of a mask in the Masks list.
pub fn mask_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u32, name: &str, visible: bool, index: usize, count: usize) {
    let empty_masks = app
        .session
        .active()
        .and_then(|p| app.session.develop_of(p))
        .is_some_and(|d| d.masks.iter().any(lightcraft_engine::cmd::mask_multi::is_empty));
    run(app, ui, &crate::i18n::tr_format!("Duplicate \"{name}\"", name = name), true, "mask.duplicate", json!({"id": id}));
    run(app, ui, &crate::i18n::tr_format!("Duplicate and Invert \"{name}\"", name = name), true, "mask.duplicate", json!({"id": id, "invert": true}));
    run(app, ui, &crate::i18n::tr_format!("Invert \"{name}\"", name = name), true, "mask.invert", json!({"id": id}));
    run(app, ui, menu_label(if visible { "Hide Mask" } else { "Show Mask" }), true, "mask.visible", json!({"id": id}));
    crate::panels::right::remove_object::remove_this_area(app, ui, id);
    ui.separator();
    let can_paste = app.session.mask_effect_clipboard.is_some();
    run(app, ui, menu_label("Copy Mask Effect"), true, "mask.copyEffect", json!({"id": id}));
    run(app, ui, menu_label("Paste Mask Effect"), can_paste, "mask.pasteEffect", json!({"id": id}));
    ui.separator();
    for (label, op) in [("Add to Mask", MaskOp::Add), ("Subtract from Mask", MaskOp::Subtract), ("Intersect Mask with", MaskOp::Intersect)] {
        let button = crate::widgets::menu_button(ui, crate::i18n::tr(label), |ui| kinds_menu(app, ui, id, op));
        register(ui, format!("maskMenu:{}", op_name(op)), button.response.rect);
    }
    ui.separator();
    run(app, ui, menu_label("Move Up"), index > 0, "mask.move", json!({"id": id, "delta": -1}));
    run(app, ui, menu_label("Move Down"), index + 1 < count, "mask.move", json!({"id": id, "delta": 1}));
    ui.separator();
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        app.ui.renaming_mask = Some((id, name.to_string()));
        ui.close();
    }
    run(app, ui, &crate::i18n::tr_format!("Delete \"{name}\"", name = name), true, "mask.delete", json!({"id": id}));
    let b = ui.add_enabled(empty_masks, egui::Button::new(crate::i18n::tr("Delete Empty Masks")));
    register(ui, "maskMenu:deleteEmpty", b.rect);
    if b.clicked() {
        let _ = app.run("mask.deleteEmpty", json!({}));
        ui.close();
    }
    let b = ui.button(crate::i18n::tr("Delete All Masks…"));
    register(ui, "maskMenu:deleteAll", b.rect);
    if b.clicked() {
        app.ui.mask_ui.confirm_delete_all = true;
        ui.close();
    }
}

/// The kinds of submask, to add to mask `mask` with `op`.
fn kinds_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, mask: u32, op: MaskOp) {
    for (g, group) in GROUPS.iter().enumerate() {
        if g > 0 {
            ui.separator();
        }
        for (kind, label) in group.iter() {
            let b = ui.button(menu_label(label));
            register(ui, format!("maskMenu:{}:{kind}", op_name(op)), b.rect);
            if b.clicked() {
                // the mask is selected first: the new submask is added to it, selected, and its tool starts
                let _ = app.run("mask.select", json!({"id": mask}));
                add_part(app, ui.ctx(), kind, op);
                super::mask_parts::start_tool(app, mask, kind);
                ui.close();
            }
        }
    }
}

/// Delete All Masks asks first: a window over the panel, drawn while [`MaskUi::confirm_delete_all`] is set.
pub fn confirm_window(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.ui.mask_ui.confirm_delete_all {
        return;
    }
    let count = app.session.active().and_then(|p| app.session.develop_of(p)).map(|d| d.masks.len()).unwrap_or(0);
    let mut answer = None;
    let shown = egui::Window::new(crate::i18n::tr("Delete All Masks"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(crate::i18n::tr_format!("Delete all {n} masks of this photo? Undo brings them back.", n = count));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let cancel = ui.button(crate::i18n::tr("Cancel"));
                register(ui, "button:maskDeleteAllCancel", cancel.rect);
                let delete = ui.button(crate::i18n::tr("Delete All Masks"));
                register(ui, "button:maskDeleteAllConfirm", delete.rect);
                if cancel.clicked() {
                    answer = Some(false);
                }
                if delete.clicked() {
                    answer = Some(true);
                }
            });
        });
    if let Some(w) = &shown {
        crate::widgets::announce_dialog(ctx, &w.response, crate::widgets::Modal::Alert);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    if let Some(yes) = answer {
        app.ui.mask_ui.confirm_delete_all = false;
        if yes {
            let _ = app.run("mask.deleteAll", json!({}));
        }
    }
}

/// The right-click menu of the submasks `targets` of mask `m` (one or several, sorted).
pub fn part_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, m: &Mask, targets: &[usize]) {
    let n = targets.len();
    let name = |k: usize| lightcraft_engine::cmd::mask_names::submask_name(m, k);
    let one = targets.first().map(|k| name(*k)).unwrap_or_default();
    let mask = m.id;
    let call = |app: &mut LightcraftApp, extra: Value| -> Option<Value> {
        let mut p = json!({"id": mask, "components": targets});
        if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(extra.clone());
        }
        app.run("mask.component", p).ok()
    };
    let item = |ui: &mut egui::Ui, label: &str, id: &str, enabled: bool| {
        let b = ui.add_enabled(enabled, egui::Button::new(label));
        register(ui, format!("partMenu:{id}"), b.rect);
        b.clicked()
    };
    if item(ui, crate::i18n::tr("Invert"), "invert", true) {
        call(app, json!({"action": "invert"}));
        ui.close();
    }
    let dup =
        if n == 1 { crate::i18n::tr_format!("Duplicate \"{label}\"", label = one) } else { crate::i18n::tr_format!("Duplicate {n} Submasks", n = n) };
    if item(ui, &dup, "duplicate", true) {
        let answer = call(app, json!({"action": "duplicate"}));
        duplicate(app, m, answer);
        ui.close();
    }
    let dup_inv = if n == 1 {
        crate::i18n::tr_format!("Duplicate and Invert \"{label}\"", label = one)
    } else {
        crate::i18n::tr_format!("Duplicate and Invert {n} Submasks", n = n)
    };
    if item(ui, &dup_inv, "duplicateInvert", true) {
        let answer = call(app, json!({"action": "duplicate", "invert": true}));
        duplicate(app, m, answer);
        ui.close();
    }
    if item(ui, crate::i18n::tr("Duplicate to New Mask"), "toMask", true) {
        call(app, json!({"action": "toMask"}));
        ui.close();
    }
    // the base submask is always Add: a menu of only it has no mode to change
    let button = crate::widgets::menu_button(ui, crate::i18n::tr("Mode"), |ui| {
        for (op, label) in [(MaskOp::Add, "Add"), (MaskOp::Subtract, "Subtract"), (MaskOp::Intersect, "Intersect")] {
            // (the mode the submasks already have is shown as the current one, and does nothing)
            let current = targets.iter().all(|k| super::mask_parts::part_mode(m, *k) == op);
            if item(ui, crate::i18n::tr(label), &format!("mode:{}", op_name(op)), !current) {
                call(app, json!({"action": "op", "op": op_name(op)}));
                if let Some(last) = targets.last() {
                    app.ui.mask_op = super::mask_parts::part_mode(m, *last);
                    app.ui.mask_op = if *last == 0 { app.ui.mask_op } else { op };
                }
                ui.close();
            }
        }
    });
    register(ui, "partMenu:mode", button.response.rect);
    ui.separator();
    if n == 1 && ui.button(crate::i18n::tr("Rename…")).clicked() {
        app.ui.renaming_component = Some((mask, targets[0], one.clone()));
        ui.close();
    }
    let del = match (n, n == m.components.len()) {
        (1, true) => crate::i18n::tr_format!("Delete \"{label}\" (and the mask)", label = one),
        (1, false) => crate::i18n::tr_format!("Delete \"{label}\"", label = one),
        (_, true) => crate::i18n::tr_format!("Delete {n} Submasks (and the mask)", n = n),
        (_, false) => crate::i18n::tr_format!("Delete {n} Submasks", n = n),
    };
    if item(ui, &del, "delete", true) {
        if call(app, json!({"action": "delete"})).is_some() {
            app.ui.mask_ui.parts.clear();
        }
        ui.close();
    }
}

/// After a duplicate: the copies the command answered become the selection.
fn duplicate(app: &mut LightcraftApp, m: &Mask, answer: Option<Value>) {
    let copies: Vec<usize> = answer
        .as_ref()
        .and_then(|v| v.get("components"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_u64().and_then(|k| usize::try_from(k).ok())).collect())
        .unwrap_or_default();
    if let Some(last) = copies.last().copied() {
        select_parts(app, m.id, copies, last);
    }
}
