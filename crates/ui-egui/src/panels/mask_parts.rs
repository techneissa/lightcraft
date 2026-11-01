//! The parts of a mask in the Masking panel (fork feature, issue #26): one Add · Subtract · Intersect control and a
//! button for each kind of part, a row for each part that selects it, and the helpers that tell the Brush which part it
//! paints into and what a stroke does to the mask.

use egui::{Rect, Sense, vec2};
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
use serde_json::{Value, json};

use super::masking::{kind_label, menu_label, range_controls, start_describe, start_object};
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{announce_selected, icon_button, one_line, register, segmented};

/// The name `mask.addComponent` and `mask.component` know a mode by.
pub fn op_name(op: MaskOp) -> &'static str {
    match op {
        MaskOp::Add => "add",
        MaskOp::Subtract => "subtract",
        MaskOp::Intersect => "intersect",
    }
}

fn is_brush(c: &MaskComponent) -> bool {
    matches!(c.shape, MaskShape::Brush { .. })
}

/// The brush part strokes go into: the `selected` part when it is a brush part, else the mask's last one (as
/// `mask.brushStroke` picks it).
pub fn brush_part(m: &Mask, selected: Option<usize>) -> Option<usize> {
    selected.filter(|k| m.components.get(*k).is_some_and(is_brush)).or_else(|| m.components.iter().rposition(is_brush))
}

/// What part `k` does to the mask. The first part is the base, whatever it says.
pub fn part_mode(m: &Mask, k: usize) -> MaskOp {
    if k == 0 { MaskOp::Add } else { m.components.get(k).map(|c| c.op).unwrap_or_default() }
}

/// Whether painting part `k` of `m` does the opposite of what its name says to the mask: coverage in a Subtract part
/// means less of the mask, and Invert (applied by the engine before the part is combined) turns every part around.
pub fn turned_around(m: &Mask, k: usize) -> bool {
    (part_mode(m, k) == MaskOp::Subtract) != m.components.get(k).is_some_and(|c| c.invert)
}

/// Whether a stroke takes coverage away from its brush part, when the user wants to `take_away` from the mask (Erase
/// chosen, or ⌥ held) and the part is `turned_around`. Add and Erase are named by what they do to the mask.
pub fn stroke_erase(take_away: bool, turned_around: bool) -> bool {
    take_away != turned_around
}

/// The part the Brush paints into on the selected mask, and whether it is turned around (not when the stroke starts a
/// part of its own, which is a plain Add part).
pub fn painting_into(app: &LightcraftApp, d: &lightcraft_develop::DevelopSettings) -> (Option<usize>, bool) {
    let Some(m) = app.session.active_mask.and_then(|id| d.masks.iter().find(|m| m.id == id)) else { return (None, false) };
    let k = brush_part(m, app.session.active_part());
    (k, k.is_some_and(|k| turned_around(m, k)))
}

/// Whether the brush's bar (Add · Erase, Auto Mask, size…) is shown: while painting, or while a brush part is selected.
pub fn brush_bar_shown(app: &LightcraftApp, m: &Mask) -> bool {
    app.ui.tool == "brush" || app.session.active_part().and_then(|k| m.components.get(k)).is_some_and(is_brush)
}

/// The `tool.brush` command: paint on the selected mask (into its selected brush part), or `new: true` for a mask of
/// its own, `op` for a new brush part, `component` to paint into that part, `erase` to choose Add or Erase.
pub fn start_brush(app: &mut LightcraftApp, p: &Value) -> Result<(), String> {
    // a malformed `component` is refused before anything changes
    let component = match p.get("component").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(
            v.as_u64().and_then(|k| usize::try_from(k).ok()).ok_or("tool.brush: `component` must be a non-negative integer that fits in usize")?,
        ),
    };
    // A new mask and Add/Subtract are selection operations, not the brush's Erase mode. Paint positively into a part
    // with the requested mode.
    let create = if p.get("new").and_then(Value::as_bool) == Some(true) {
        Some(app.run("mask.add", json!({"kind": "brush"})))
    } else {
        p.get("op").map(|op| app.run("mask.addComponent", json!({"kind": "brush", "op": op})))
    };
    if let Some(result) = create {
        result?;
        // a Subtract part starts on Erase (it takes away from the mask), any other on Add
        app.ui.brush_erase = p.get("op").and_then(Value::as_str) == Some("subtract");
        app.ui.mask_overlay = true;
        select_last_part(app);
    }
    if let Some(k) = component {
        let mid = app.session.active_mask.ok_or("tool.brush: no mask selected")?;
        let id = app.session.active().ok_or("tool.brush: no active photo")?;
        let is_brush_part = app
            .session
            .develop_of(id)
            .and_then(|d| d.masks.iter().find(|m| m.id == mid).and_then(|m| m.components.get(k)).map(is_brush))
            .unwrap_or(false);
        if !is_brush_part {
            return Err(format!("tool.brush: part {k} of mask {mid} is not a brush part"));
        }
        app.run("mask.select", json!({"id": mid, "component": k}))?;
        // picking a part starts on Add
        app.ui.brush_erase = false;
    } else if let Some((id, m)) = app.session.active().zip(app.session.active_mask) {
        // the part the strokes go into is the selected one: when another kind of part is selected, that is the last brush part
        let target =
            app.session.develop_of(id).and_then(|d| d.masks.iter().find(|x| x.id == m).and_then(|x| brush_part(x, app.session.active_part())));
        if let Some(k) = target.filter(|k| app.session.active_part() != Some(*k)) {
            app.run("mask.select", json!({"id": m, "component": k}))?;
        }
    }
    if let Some(erase) = p.get("erase").and_then(Value::as_bool) {
        app.ui.brush_erase = erase;
    }
    app.ui.right = crate::state::RightPanel::Masking;
    app.ui.view = crate::state::ViewMode::Detail;
    app.ui.tool = "brush".into();
    Ok(())
}

/// Select the last part of the selected mask (the one just added).
fn select_last_part(app: &mut LightcraftApp) {
    let (Some(id), Some(mid)) = (app.session.active(), app.session.active_mask) else { return };
    let n = app.session.develop_of(id).and_then(|d| d.masks.iter().find(|m| m.id == mid).map(|m| m.components.len())).unwrap_or(0);
    if n > 0 {
        let _ = app.run("mask.select", json!({"id": mid, "component": n - 1}));
    }
}

/// Esc, one step: leave the part (keeping its mask) when the brush is off. Whether it did.
pub fn leave_part(app: &mut LightcraftApp) -> bool {
    if app.ui.right != crate::state::RightPanel::Masking || app.session.active_part().is_none() {
        return false;
    }
    match app.session.active_mask {
        Some(id) => app.run("mask.select", json!({"id": id})).is_ok(),
        None => false,
    }
}

/// Delete: the selected part of the selected mask, else the mask. Whether anything was selected to delete.
pub fn delete_selected(app: &mut LightcraftApp) -> bool {
    if let Some(k) = app.session.active_part() {
        let selected_is_brush = app.session.active_mask.zip(app.session.active()).and_then(|(mid, id)| {
            app.session.develop_of(id).and_then(|d| d.masks.iter().find(|m| m.id == mid).and_then(|m| m.components.get(k)).map(is_brush))
        });
        let _ = app.run("mask.component", json!({"component": k, "action": "delete"}));
        if selected_is_brush == Some(true) {
            app.ui.tool.clear();
        }
        return true;
    }
    if app.session.active_mask.is_some() {
        let _ = app.run("mask.delete", json!({}));
        return true;
    }
    false
}

/// The kinds of part, each a button: id, label, icon.
const KINDS: [(&str, &str, Icon); 10] = [
    ("brush", "Brush", Icon::Brush),
    ("linear", "Linear Gradient", Icon::Linear),
    ("radial", "Radial Gradient", Icon::Radial),
    ("colorRange", "Color Range", Icon::Picker),
    ("luminanceRange", "Luminance Range", Icon::Sliders),
    ("subject", "Subject", Icon::Subject),
    ("sky", "Sky", Icon::Sky),
    ("background", "Background", Icon::Background),
    ("object", "Object", Icon::ObjectSelect),
    ("prompt", "Describe…", Icon::Describe),
];

/// Under the selected mask: Add · Subtract · Intersect for the next part, and a button for each kind of part.
pub fn add_controls(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ops = [(MaskOp::Add, "Add"), (MaskOp::Subtract, "Subtract"), (MaskOp::Intersect, "Intersect")];
    let items: Vec<(&str, &str)> = ops.iter().map(|(op, label)| (*label, op_name(*op))).collect();
    let active = ops.iter().position(|(op, _)| *op == app.ui.mask_op);
    ui.add_space(4.0);
    if let Some(i) = segmented(ui, "maskMode", &items, active, 3)
        && let Some((op, _)) = ops.get(i)
    {
        app.ui.mask_op = *op;
    }
    ui.add_space(4.0);
    let op = app.ui.mask_op;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        for (kind, label, icon) in KINDS {
            // painting only adds or erases
            let enabled = !(kind == "brush" && op == MaskOp::Intersect);
            let tip = crate::i18n::tr(label);
            if icon_button(ui, &format!("maskPart:{kind}"), icon, vec2(28.0, 28.0), false, enabled, tip).clicked() {
                add_part(app, ui.ctx(), kind, op);
            }
        }
    });
}

fn add_part(app: &mut LightcraftApp, ctx: &egui::Context, kind: &str, op: MaskOp) {
    let name = op_name(op);
    match kind {
        "object" => start_object(app, ctx, name),
        "prompt" => start_describe(app, name),
        "brush" => {
            let _ = app.run("tool.brush", json!({"op": name}));
        }
        k => {
            if app.run("mask.addComponent", json!({"op": name, "kind": k})).is_ok() {
                select_last_part(app);
                // a gradient or range has no brush to paint with
                if app.ui.tool == "brush" {
                    app.ui.tool.clear();
                }
            }
        }
    }
}

/// Pick part `k` of mask `mask`: it is selected, and a brush part gets the brush, on Add.
fn pick_part(app: &mut LightcraftApp, mask: u32, k: usize, brush: bool) {
    let _ = app.run("mask.select", json!({"id": mask, "component": k}));
    // the mode control shows the selected part's mode
    let mode =
        app.session.active().and_then(|id| app.session.develop_of(id)).and_then(|d| d.masks.iter().find(|m| m.id == mask).map(|m| part_mode(m, k)));
    if let Some(mode) = mode {
        app.ui.mask_op = mode;
    }
    if brush {
        let _ = app.run("tool.brush", json!({"component": k}));
    } else if app.ui.tool == "brush" {
        app.ui.tool.clear();
    }
}

/// The parts of mask `m`, a row each: a click selects (a brush part also starts painting into it), double-click
/// renames; the selected or hovered row has its mode, Invert and Delete buttons.
pub fn part_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, m: &Mask) {
    let t = Tokens::get(ui.ctx());
    let selected = app.session.active_part();
    for (i, c) in m.components.iter().enumerate() {
        let (kind, icon) = kind_label(&c.shape);
        let label = c.name.clone().unwrap_or_else(|| match &c.shape {
            MaskShape::Prompt { text, .. } => format!("“{text}”"),
            _ => menu_label(kind).to_string(),
        });
        if rename_row(app, ui, m.id, i, c) {
            continue;
        }
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        announce_selected(&resp, label.clone(), selected == Some(i));
        register(ui, format!("component:{i}"), r);
        let sel = selected == Some(i);
        let hot = sel || ui.rect_contains_pointer(r);
        ui.painter().rect_filled(
            r,
            4.0,
            if sel {
                t.tool_active
            } else if resp.hovered() {
                t.hover.gamma_multiply(0.7)
            } else {
                t.chrome
            },
        );
        paint(ui.painter(), Rect::from_min_size(r.min + vec2(6.0, 5.0), vec2(16.0, 16.0)), icon, t.text_label);
        // the buttons' room: the three modes (not on the base), Invert and Delete
        let room = if hot { if i == 0 { 2.0 * 22.0 } else { 5.0 * 22.0 } } else { 0.0 };
        let mode = match part_mode(m, i) {
            MaskOp::Add => "",
            MaskOp::Subtract => "− ",
            MaskOp::Intersect => "∩ ",
        };
        let text = format!("{mode}{label}{}", if c.invert { crate::i18n::tr(" (inverted)") } else { "" });
        let g = one_line(ui.painter(), &text, t.font(13.0), t.text, (r.width() - 28.0 - room - 4.0).max(0.0));
        let elided = g.elided;
        ui.painter().galley(egui::pos2(r.left() + 28.0, r.center().y - g.size().y / 2.0), g, t.text);
        let resp = if elided { resp.on_hover_text(&text) } else { resp };
        let mut used = false;
        if hot {
            used = part_buttons(app, ui, m, i, c, r);
        }
        if resp.clicked() && !used {
            pick_part(app, m.id, i, matches!(c.shape, MaskShape::Brush { .. }));
        }
        if resp.double_clicked() {
            app.ui.renaming_component = Some((m.id, i, label.clone()));
        }
        crate::widgets::context_menu(&resp, |ui| part_menu(app, ui, m.id, i, &label, m.components.len()));
        range_controls(app, ui, i, &c.shape);
    }
}

/// The inline rename field of part `i`. Whether it is showing in place of the row.
fn rename_row(app: &mut LightcraftApp, ui: &mut egui::Ui, mask: u32, i: usize, c: &MaskComponent) -> bool {
    let Some((_, _, name)) = app.ui.renaming_component.as_mut().filter(|(m, k, _)| *m == mask && *k == i) else { return false };
    // Enter (or leaving the field) commits, Escape cancels
    let r = ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width()).id_salt(("compRename", mask, i)));
    register(ui, format!("componentRename:{i}"), r.rect);
    if !r.has_focus() && !r.lost_focus() {
        r.request_focus();
    }
    if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
        app.ui.renaming_component = None;
    } else if r.lost_focus() {
        let name = name.trim().to_string();
        app.ui.renaming_component = None;
        if Some(&name) != c.name.as_ref() {
            let _ = app.run("mask.component", json!({"id": mask, "component": i, "action": "rename", "name": name}));
        }
    }
    true
}

/// The buttons at the right of part `i`'s row `r`. Whether one was pressed (the row must not also be picked).
fn part_buttons(app: &mut LightcraftApp, ui: &mut egui::Ui, m: &Mask, i: usize, c: &MaskComponent, r: Rect) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut used = false;
    let mut x = r.right() - 4.0;
    let mut button = |ui: &mut egui::Ui, id: String, name: &str, active: bool, draw: &dyn Fn(&egui::Painter, Rect, egui::Color32)| -> bool {
        let br = Rect::from_min_size(egui::pos2(x - 20.0, r.center().y - 10.0), vec2(20.0, 20.0));
        x -= 22.0;
        register(ui, id.clone(), br);
        let resp = ui.interact(br, egui::Id::unique((id, m.id, i)), Sense::click());
        announce_selected(&resp, crate::i18n::tr(name), active);
        if active {
            ui.painter().rect(br, 3.0, t.pressed, egui::Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(br, 3.0, t.hover);
        }
        draw(ui.painter(), br, if resp.hovered() || active { t.text } else { t.text_dim });
        resp.clicked()
    };
    if button(ui, format!("partDelete:{i}"), "Delete part", false, &|p, br, col| paint(p, br.shrink(3.0), Icon::Trash, col)) {
        let _ = app.run("mask.component", json!({"id": m.id, "component": i, "action": "delete"}));
        used = true;
    }
    if button(ui, format!("partInvert:{i}"), "Invert part", c.invert, &|p, br, col| paint(p, br.shrink(3.0), Icon::Invert, col)) {
        let _ = app.run("mask.component", json!({"id": m.id, "component": i, "action": "invert"}));
        used = true;
    }
    if i > 0 {
        let now = part_mode(m, i);
        for (op, name) in [(MaskOp::Intersect, "Intersect"), (MaskOp::Subtract, "Subtract"), (MaskOp::Add, "Add")] {
            let draw = move |p: &egui::Painter, br: Rect, col: egui::Color32| match op {
                MaskOp::Add => paint(p, br.shrink(4.0), Icon::Plus, col),
                MaskOp::Subtract => paint(p, br.shrink(4.0), Icon::Minus, col),
                MaskOp::Intersect => {
                    // ∩ drawn as a path: the font has no such glyph
                    let (c, r) = (br.center() + vec2(0.0, 1.0), 4.0);
                    let mut pts = vec![c + vec2(-r, r)];
                    pts.extend((0..=12).map(|i| {
                        let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                        c + vec2(r * a.cos(), r * a.sin())
                    }));
                    pts.push(c + vec2(r, r));
                    p.add(egui::Shape::line(pts, egui::Stroke::new(1.5, col)));
                }
            };
            if button(ui, format!("partMode:{i}:{}", op_name(op)), name, now == op, &draw) {
                used = true;
                if op != now {
                    let _ = app.run("mask.component", json!({"id": m.id, "component": i, "action": "op", "op": op_name(op)}));
                    app.ui.mask_op = op;
                }
            }
        }
    }
    used
}

/// The right-click menu of part `k`.
fn part_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, mask: u32, k: usize, label: &str, count: usize) {
    let mut run = |ui: &mut egui::Ui, text: &str, p: Value| {
        if ui.button(menu_label(text)).clicked() {
            let mut p = p;
            p["id"] = json!(mask);
            p["component"] = json!(k);
            let _ = app.run("mask.component", p);
            ui.close();
        }
    };
    run(ui, "Invert", json!({"action": "invert"}));
    run(ui, &crate::i18n::tr_format!("Duplicate \"{label}\"", label = label), json!({"action": "duplicate"}));
    ui.separator();
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        app.ui.renaming_component = Some((mask, k, label.to_string()));
        ui.close();
    }
    let del = if count == 1 {
        crate::i18n::tr_format!("Delete \"{label}\" (and the mask)", label = label)
    } else {
        crate::i18n::tr_format!("Delete \"{label}\"", label = label)
    };
    if ui.button(del).clicked() {
        let _ = app.run("mask.component", json!({"id": mask, "component": k, "action": "delete"}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(op: MaskOp, brush: bool) -> MaskComponent {
        let shape = if brush { MaskShape::Brush { strokes: vec![] } } else { MaskShape::Subject };
        MaskComponent { name: None, op, invert: false, shape }
    }

    #[test]
    fn add_and_erase_are_named_by_what_they_do_to_the_mask() {
        let with_part =
            |op, invert| Mask { components: vec![part(MaskOp::Add, true), MaskComponent { invert, ..part(op, true) }], ..Default::default() };
        let turned = |op, invert| turned_around(&with_part(op, invert), 1);
        // an Add part: Add paints it, Erase takes paint out
        assert!(!stroke_erase(false, turned(MaskOp::Add, false)));
        assert!(stroke_erase(true, turned(MaskOp::Add, false)));
        // a Subtract part: painting it removes the mask, so Add takes paint out of the part and Erase paints it
        assert!(stroke_erase(false, turned(MaskOp::Subtract, false)));
        assert!(!stroke_erase(true, turned(MaskOp::Subtract, false)));
        // Invert turns every part around, and a Subtract part back
        assert!(stroke_erase(false, turned(MaskOp::Add, true)));
        assert!(!stroke_erase(false, turned(MaskOp::Subtract, true)));
        // the first part is the base: Add, whatever it says
        let base = Mask { components: vec![part(MaskOp::Subtract, true)], ..Default::default() };
        assert!(!turned_around(&base, 0));
    }

    #[test]
    fn the_brush_paints_into_the_selected_brush_part_else_the_last_one() {
        let m = Mask { components: vec![part(MaskOp::Add, true), part(MaskOp::Subtract, false), part(MaskOp::Subtract, true)], ..Default::default() };
        assert_eq!(brush_part(&m, None), Some(2));
        assert_eq!(brush_part(&m, Some(0)), Some(0));
        assert_eq!(brush_part(&m, Some(1)), Some(2), "a part that is not a brush part is not painted into");
        assert_eq!(brush_part(&m, Some(7)), Some(2));
        let none = Mask { components: vec![part(MaskOp::Add, false)], ..Default::default() };
        assert_eq!(brush_part(&none, None), None);
    }

    #[test]
    fn the_first_part_is_the_base_whatever_it_says() {
        let m = Mask { components: vec![part(MaskOp::Subtract, true), part(MaskOp::Subtract, true)], ..Default::default() };
        assert_eq!(part_mode(&m, 0), MaskOp::Add);
        assert_eq!(part_mode(&m, 1), MaskOp::Subtract);
    }
}
