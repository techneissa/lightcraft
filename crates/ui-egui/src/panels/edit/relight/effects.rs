//! Fork (issue #302, `fork/relight.md`): the Effects group of the Relight section, for the selected light: a gel (the
//! named gels as swatches, then any hue and a strength), a pattern (blinds, window panes, leaves, dapple, with
//! Scale, Rotation, Softness and Openness) and Haze. Each is a control by id, `relight.<n>.gelHue`, `.gelStrength`,
//! `.pattern`, `.patternScale`, `.patternRotation`, `.patternSoftness`, `.patternOpen` (issue #316) and `.haze`.

use egui::{Align2, Color32, Sense, Stroke, vec2};
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight_effects::{GELS, LightEffects, PATTERNS};
use serde_json::json;

use super::super::{Icon, LightcraftApp, Tokens, control, register};

/// A gel's colour as its swatch shows it: the hue at full chroma, towards white as the strength drops.
fn swatch(hue: f64, strength: f64) -> Color32 {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let k = (strength / 100.0).clamp(0.0, 1.0);
    let mix = |c: f64| ((1.0 - k * (1.0 - 0.9 * c)) * 255.0).round() as u8;
    Color32::from_rgb(mix(r), mix(g), mix(b))
}

/// A light's dot in the list: its Warmth colour, towards its gel's colour as the gel's strength grows.
pub(super) fn dot(warmth_colour: Color32, e: &LightEffects) -> Color32 {
    if e.gel_strength <= 0.0 {
        return warmth_colour;
    }
    let gel = swatch(e.gel_hue, e.gel_strength);
    let k = (e.gel_strength / 100.0).clamp(0.0, 1.0) as f32;
    warmth_colour.lerp_to_gamma(gel, k.max(0.5))
}

/// The row of named gels: None, then each gel as a swatch (a click puts the gel on the light).
fn gels(app: &mut LightcraftApp, ui: &mut egui::Ui, e: &LightEffects) {
    let t = Tokens::get(ui.ctx());
    let mut picked: Option<serde_json::Value> = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let none = e.gel_strength <= 0.0;
        let (r, resp) = ui.allocate_exact_size(vec2(44.0, 24.0), Sense::click());
        register(ui, "relight:gel:none", r);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, none, "No Gel"));
        let p = ui.painter();
        p.rect(r, 4.0, if none { t.pressed } else { t.button }, Stroke::new(1.0, t.button_border), egui::StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr("None"), t.semibold(11.0), t.text);
        if resp.clicked() {
            picked = Some(json!({"hue": e.gel_hue, "strength": 0.0}));
        }
        for (name, hue, strength) in GELS {
            let on = e.gel_strength > 0.0 && (e.gel_hue - hue).abs() < 0.5 && (e.gel_strength - strength).abs() < 0.5;
            let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
            register(ui, format!("relight:gel:{}", name.to_lowercase().replace(' ', "-")), r);
            let resp = resp.on_hover_text(crate::i18n::tr(name));
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, name.to_string()));
            let p = ui.painter();
            p.rect_filled(r, 4.0, swatch(hue, strength));
            p.rect_stroke(r, 4.0, if on { Stroke::new(2.0, Color32::WHITE) } else { Stroke::new(1.0, t.button_border) }, egui::StrokeKind::Inside);
            if resp.clicked() {
                picked = Some(json!({"gel": name}));
            }
        }
    });
    if let Some(p) = picked
        && let Err(err) = app.run("relight.setGel", p)
    {
        app.toast(ui.ctx(), err);
    }
}

/// The pattern's picker: None, Blinds, Window Panes, Leaves, Dapple.
fn patterns(app: &mut LightcraftApp, ui: &mut egui::Ui, e: &LightEffects, sel: usize) {
    let items = [("None", "none"), ("Blinds", "blinds"), ("Panes", "panes"), ("Leaves", "leaves"), ("Dapple", "dapple")];
    let tips: Vec<&str> = PATTERNS.to_vec();
    let active = Some((e.pattern.round().clamp(0.0, 4.0)) as usize);
    if let Some(i) = crate::widgets::segmented_tips(ui, "relightPattern", &items, &tips, active, 5)
        && let Err(err) = app.run("develop.set", json!({"control": format!("relight.{sel}.pattern"), "value": i}))
    {
        app.toast(ui.ctx(), err);
    }
}

/// A small dim heading above a part of the group.
fn heading(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 2 }).show(ui, |ui| {
        let r = ui.label(egui::RichText::new(crate::i18n::tr(text)).size(11.0).color(t.text_dim));
        register(ui, format!("relight:effects:{}", text.to_lowercase()), r.rect);
    });
}

/// The group: a flyout row "Effects" that opens the gel, pattern and haze controls of light `sel`.
pub(super) fn effects(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, sel: usize) {
    let open = app.ui.flyout_open("relightEffects");
    if crate::widgets::flyout_row(ui, "relightEffects", "Effects", Icon::SparkleFill, open).clicked() {
        app.ui.toggle_flyout("relightEffects");
    }
    if !open {
        return;
    }
    let Some(e) = d.relight.lights.get(sel).map(|l| l.effects) else { return };
    let margin = egui::Margin { left: 24, right: 22, top: 0, bottom: 4 };
    heading(ui, "Gel");
    egui::Frame::NONE.inner_margin(margin).show(ui, |ui| gels(app, ui, &e));
    control(app, ui, d, &format!("relight.{sel}.gelHue"), true);
    control(app, ui, d, &format!("relight.{sel}.gelStrength"), true);
    heading(ui, "Pattern");
    egui::Frame::NONE.inner_margin(margin).show(ui, |ui| patterns(app, ui, &e, sel));
    if e.pattern.round() >= 1.0 {
        for field in ["patternScale", "patternRotation", "patternSoftness", "patternOpen"] {
            control(app, ui, d, &format!("relight.{sel}.{field}"), true);
        }
    }
    heading(ui, "Haze");
    control(app, ui, d, &format!("relight.{sel}.haze"), true);
}
