//! Fork (issue #288, `fork/relight.md`): the Relight section. A list of the photo's lights (up to eight; Add Light,
//! Duplicate, Delete; a click on a row selects it), a depth box (the photo's depth, near light and far dark, with
//! every light as a handle, the selected one marked, to drag across it, and a Near–Far track below for the selected
//! light's depth), its Amount, Softness and Warmth, and an Advanced disclosure with Darken Surroundings (the whole
//! photo's) and the selected light's Wrap. Opening the section starts finding the photo's depth
//! (`Session::relight_prepare`); while the section is open the lights' handles can also be dragged on the photo
//! (`panels/detail/relight.rs`). Every drag is one undo step.

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use serde_json::{Value, json};

use super::{Icon, LightcraftApp, Tokens, control, register};

/// How near a press must be to a handle to take it (points).
const HIT: f32 = 12.0;

/// The handle a press at `q` takes: the one drawn on top within `reach` (points). The selected light is drawn last, so
/// it comes first; the others are drawn in their order, so the later one comes first.
pub(crate) fn topmost(spots: &[Pos2], selected: usize, q: Pos2, reach: f32) -> Option<usize> {
    let near = |i: &usize| spots.get(*i).is_some_and(|p| p.distance(q) <= reach);
    Some(selected).filter(near).or_else(|| (0..spots.len()).rev().find(near))
}

/// The depth box's largest height (points).
const BOX_MAX_H: f32 = 190.0;

/// A light's colour as the handle and the list show it: white, towards amber for a warm light and blue for a cool one.
pub(crate) fn light_tint(warmth: f64) -> Color32 {
    let w = (warmth / 100.0).clamp(-1.0, 1.0) as f32;
    if w >= 0.0 {
        Color32::from_rgb(255, (250.0 - 50.0 * w) as u8, (235.0 - 150.0 * w) as u8)
    } else {
        Color32::from_rgb((235.0 + 60.0 * w) as u8, (245.0 + 20.0 * w) as u8, 255)
    }
}

/// The light's handle: a small sun, drawn in code (original work).
pub(crate) fn paint_light(p: &egui::Painter, c: Pos2, r: f32, warmth: f64, active: bool) {
    let tint = light_tint(warmth);
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let (s, co) = a.sin_cos();
        let (a0, a1) = (c + vec2(co, s) * (r + 2.5), c + vec2(co, s) * (r + 6.0));
        p.line_segment([a0, a1], Stroke::new(3.2, Color32::from_black_alpha(130)));
        p.line_segment([a0, a1], Stroke::new(1.6, tint));
    }
    p.circle_filled(c, r + 1.5, Color32::from_black_alpha(140));
    p.circle_filled(c, r, tint);
    if active {
        p.circle_stroke(c, r + 9.0, Stroke::new(1.0, Color32::from_white_alpha(160)));
    }
}

/// The depth picture's texture for photo field `key`, made once.
fn texture(ctx: &egui::Context, key: u64, thumb: &(usize, usize, Vec<u8>)) -> egui::TextureHandle {
    let id = egui::Id::unique(("relightDepthTexture", key));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let (w, h, px) = thumb;
    let img = egui::ColorImage::from_gray([*w, *h], px);
    let t = ctx.load_texture(format!("relight-depth-{key:x}"), img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Set Relight controls `values` (`relight.x` …) in a drag that began with `develop.beginInteraction`.
fn set(app: &mut LightcraftApp, ctx: &egui::Context, values: Value) {
    if let Err(e) = app.run("develop.set", json!({"values": values})) {
        app.toast(ctx, e);
    }
}

/// A drag's start, steps and end on `resp` as one undo step labelled `label`; `step` gets the pointer. While it goes
/// `control` is the dragged control (⌥ then shows where the light reaches).
pub(crate) fn drag(
    app: &mut LightcraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    (label, control): (&str, &str),
    mut step: impl FnMut(&mut LightcraftApp, Pos2),
) {
    if resp.drag_started() {
        let _ = app.run("develop.beginInteraction", json!({"label": label}));
        app.ui.dragging_control = Some(control.to_string());
    }
    if (resp.dragged() || resp.drag_started() || resp.clicked())
        && let Some(q) = resp.interact_pointer_pos()
    {
        if resp.clicked() {
            let _ = app.run("develop.beginInteraction", json!({"label": label}));
        }
        step(app, q);
        if resp.clicked() {
            let _ = app.run("develop.endInteraction", json!({}));
        }
    }
    if resp.drag_stopped() {
        let _ = app.run("develop.endInteraction", json!({}));
        app.ui.dragging_control = None;
    }
    let _ = ui;
}

/// The section's body.
pub(super) fn relight(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, id: PhotoId) {
    if cfg!(target_arch = "wasm32") {
        // (the depth would be found on the page's thread: the web build has no Relight)
        let t = Tokens::get(ui.ctx());
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 6 }).show(ui, |ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Relight needs the desktop app.")).size(12.0).color(t.text_label));
        });
        return;
    }
    let made = app.session.relight_made(id);
    if made.is_none() {
        // (opening the section finds the depth, so the box shows it before the light is turned up)
        app.session.relight_prepare(id);
    }
    let sel = app.session.relight_selected(id);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 4 }).show(ui, |ui| {
        lights_list(app, ui, d, sel);
        ui.add_space(8.0);
        depth_box(app, ui, d, id, made.as_deref(), sel);
    });
    // (the sliders act on the selected light, named by its own ids)
    for field in ["amount", "softness", "warmth"] {
        control(app, ui, d, &format!("relight.{sel}.{field}"), true);
    }
    ui.add_space(4.0);
    let open = app.ui.flyout_open("relightAdvanced");
    if crate::widgets::flyout_row(ui, "relightAdvanced", "Advanced", Icon::Sliders, open).clicked() {
        app.ui.toggle_flyout("relightAdvanced");
    }
    if open {
        control(app, ui, d, "relight.darken", true);
        control(app, ui, d, &format!("relight.{sel}.wrap"), true);
    }
}

/// The photo's lights, one row each (a click selects it), and Add Light, Duplicate and Delete under them.
fn lights_list(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, sel: usize) {
    let t = Tokens::get(ui.ctx());
    let count = d.relight.lights.len();
    let mut picked = None;
    for (i, l) in d.relight.lights.iter().enumerate() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let resp = ui.interact(r, ui.scope_id().with(("relightRow", i)), Sense::click());
        register(ui, format!("relight:row:{i}"), r);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, i == sel, format!("Light {}", i + 1)));
        let p = ui.painter();
        p.rect_filled(
            r,
            4.0,
            if i == sel {
                t.pressed
            } else if resp.hovered() {
                t.hover.gamma_multiply(0.8)
            } else {
                t.inset
            },
        );
        p.circle_filled(pos2(r.left() + 14.0, r.center().y), 5.0, light_tint(l.warmth));
        let label = crate::i18n::tr_format!("Light {n}", n = i + 1);
        p.text(pos2(r.left() + 28.0, r.center().y), Align2::LEFT_CENTER, label, t.font(12.0), t.text);
        let amount = if l.amount > 0.0 { format!("{}", l.amount.round()) } else { crate::i18n::tr("Off").to_string() };
        p.text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, amount, t.font(11.0), t.text_dim);
        if resp.clicked() {
            picked = Some(i);
        }
    }
    if let Some(i) = picked
        && let Err(e) = app.run("relight.selectLight", json!({"index": i}))
    {
        app.toast(ui.ctx(), e);
    }
    ui.add_space(6.0);
    let room = count < lightcraft_develop::relight::MAX_LIGHTS;
    let mut act = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.add_enabled_ui(room, |ui| {
            if crate::widgets::text_button(ui, "relightAddLight", "Add Light", false).clicked() {
                act = Some("relight.addLight");
            }
            if crate::widgets::text_button(ui, "relightDuplicate", "Duplicate", false).clicked() {
                act = Some("relight.duplicateLight");
            }
        });
        if crate::widgets::text_button(ui, "relightDelete", "Delete", false).clicked() {
            act = Some("relight.deleteLight");
        }
    });
    if let Some(c) = act
        && let Err(e) = app.run(c, json!({}))
    {
        app.toast(ui.ctx(), e);
    }
}

/// The depth box, its handles, and what stands in for it while the depth is not there.
fn depth_box(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    d: &DevelopSettings,
    id: PhotoId,
    made: Option<&lightcraft_engine::relight::Made>,
    sel: usize,
) {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width().max(60.0);
    let Some(m) = made else {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 96.0), Sense::hover());
        ui.painter().rect_filled(rect, 4.0, t.inset);
        register(ui, "relight:depthBox", rect);
        let status = app.session.relight_status(id);
        missing(app, ui, rect, &status, lightcraft_pipeline::relight::scene_of(d).is_some());
        return;
    };
    let (tw, th) = (m.thumb.0.max(1) as f32, m.thumb.1.max(1) as f32);
    let h = (w * th / tw).min(BOX_MAX_H);
    let bw = h * tw / th;
    let (outer, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let rect = Rect::from_min_size(pos2(outer.center().x - bw / 2.0, outer.top()), vec2(bw, h));
    let tex = texture(ui.ctx(), m.input.key, &m.thumb);
    let p = ui.painter_at(outer.expand(10.0));
    p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, t.text_dim.gamma_multiply(0.4)), egui::StrokeKind::Outside);
    register(ui, "relight:depthBox", rect);
    // the lights: a press on a handle selects that light; the selected one is dragged anywhere in the box (a click puts
    // it there)
    let resp = ui.interact(rect, ui.scope_id().with("relightBox"), Sense::click_and_drag());
    let spots: Vec<Pos2> = d
        .relight
        .lights
        .iter()
        .map(|l| pos2(rect.left() + rect.width() * l.x.clamp(0.0, 1.0) as f32, rect.top() + rect.height() * l.y.clamp(0.0, 1.0) as f32))
        .collect();
    let mut sel = sel;
    if (resp.drag_started() || resp.clicked())
        && let Some(q) = resp.interact_pointer_pos()
        && let Some(i) = topmost(&spots, sel, q, HIT)
        && i != sel
    {
        match app.run("relight.selectLight", json!({"index": i})) {
            Ok(_) => sel = i,
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    let ctx = ui.ctx().clone();
    drag(app, ui, &resp, ("Relight Light", "relight.x"), |app, q| {
        let (x, y) = (((q.x - rect.left()) / rect.width()) as f64, ((q.y - rect.top()) / rect.height()) as f64);
        set(app, &ctx, json!({"relight.x": x.clamp(0.0, 1.0), "relight.y": y.clamp(0.0, 1.0)}));
    });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    for (i, (l, at)) in d.relight.lights.iter().zip(&spots).enumerate() {
        // (the selected light on top, and the only one with its ring)
        if i != sel {
            paint_light(&p, *at, 4.5, l.warmth, false);
            register(ui, format!("relight:light:{i}"), Rect::from_center_size(*at, vec2(14.0, 14.0)));
        }
    }
    if let (Some(l), Some(at)) = (d.relight.lights.get(sel), spots.get(sel)) {
        paint_light(&p, *at, 5.5, l.warmth, true);
        register(ui, "relight:light", Rect::from_center_size(*at, vec2(16.0, 16.0)));
        register(ui, format!("relight:light:{sel}"), Rect::from_center_size(*at, vec2(16.0, 16.0)));
    }
    ui.add_space(8.0);
    depth_track(app, ui, d, rect.left()..=rect.right(), sel);
    // (a switch under the track: the picture ⌥ shows while a Relight control is dragged)
    ui.add_space(4.0);
    look_switch(app, ui);
    // (only a depth map from the photo itself is named; the model's needs no caption)
    if m.source == lightcraft_engine::relight::Source::File {
        ui.add_space(2.0);
        let label = ui.label(egui::RichText::new(crate::i18n::tr("Depth from the photo")).size(11.0).color(t.text_dim));
        register(ui, "relight:source", label.rect);
    }
}

/// The Crosses / Contours switch under the Near–Far track.
fn look_switch(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    use crate::state::RelightLook;
    let items = [("Crosses", "crosses"), ("Contours", "contours")];
    let active = Some(usize::from(app.ui.relight_look == RelightLook::Contours));
    let tips = ["Hold ⌥ while dragging to see small crosses on the surfaces the light reaches", "Hold ⌥ while dragging to see lines of equal light"];
    if let Some(i) = crate::widgets::segmented_tips(ui, "relightLook", &items, &tips, active, 2) {
        app.ui.relight_look = if i == 1 { RelightLook::Contours } else { RelightLook::Crosses };
    }
}

/// The Near–Far track under the box: where the light is in depth.
fn depth_track(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, span: std::ops::RangeInclusive<f32>, sel: usize) {
    let t = Tokens::get(ui.ctx());
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let font = t.font(11.0);
    let p = ui.painter();
    let near = p.text(pos2(*span.start(), row.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Near"), font.clone(), t.text_dim);
    let far = p.text(pos2(*span.end(), row.center().y), Align2::RIGHT_CENTER, crate::i18n::tr("Far"), font, t.text_dim);
    let (x0, x1) = (near.right() + 10.0, far.left() - 10.0);
    if x1 <= x0 + 10.0 {
        return;
    }
    let y = row.center().y;
    // near light, far dark, as in the box
    let mut mesh = egui::Mesh::default();
    let track = Rect::from_min_max(pos2(x0, y - 2.0), pos2(x1, y + 2.0));
    mesh.colored_vertex(track.left_top(), Color32::from_gray(230));
    mesh.colored_vertex(track.right_top(), Color32::from_gray(45));
    mesh.colored_vertex(track.right_bottom(), Color32::from_gray(45));
    mesh.colored_vertex(track.left_bottom(), Color32::from_gray(230));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(mesh);
    let at = pos2(x0 + (x1 - x0) * (d.relight.lights.get(sel).map_or(0.0, |l| l.depth) / 100.0).clamp(0.0, 1.0) as f32, y);
    let hit = Rect::from_min_max(pos2(x0 - 8.0, row.top()), pos2(x1 + 8.0, row.bottom()));
    let resp = ui.interact(hit, ui.scope_id().with("relightDepth"), Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    drag(app, ui, &resp, ("Relight Depth", "relight.depth"), |app, q| {
        let v = (((q.x - x0) / (x1 - x0)) * 100.0).clamp(0.0, 100.0);
        set(app, &ctx, json!({"relight.depth": (v as f64).round()}));
    });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let p = ui.painter();
    p.circle_filled(at, 6.5, Color32::from_black_alpha(150));
    p.circle_filled(at, 5.0, Color32::WHITE);
    register(ui, "relight:depth", Rect::from_center_size(at, vec2(14.0, 14.0)));
}

/// What the box shows while the depth is not there: being found, the model to download, or why it can't be.
/// `on`: whether the light changes the photo (Find Depth needs it).
fn missing(app: &mut LightcraftApp, ui: &mut egui::Ui, rect: Rect, status: &Value, on: bool) {
    let t = Tokens::get(ui.ctx());
    let finding = status["finding"].as_bool().unwrap_or(false);
    let model = &status["model"];
    let error = status["error"].as_str();
    let p = ui.painter();
    let c = rect.center();
    if finding {
        p.text(c, Align2::CENTER_CENTER, crate::i18n::tr("Finding Depth…"), t.font(12.5), t.text_label);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    if status["cancelled"].as_bool() == Some(true) && !on {
        p.text(c, Align2::CENTER_CENTER, crate::i18n::tr("Finding Depth Cancelled"), t.font(12.5), t.text_label);
        return;
    }
    if status["cancelled"].as_bool() == Some(true) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_center_size(c, vec2(rect.width() - 16.0, 26.0)))
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        if crate::widgets::text_button(&mut child, "relightFindDepth", crate::i18n::tr("Find Depth"), false).clicked()
            && let Err(e) = app.run("relight.findDepth", json!({}))
        {
            app.toast(ui.ctx(), e);
        }
        return;
    }
    let needs_model = error.is_some_and(|e| e == lightcraft_engine::relight::MISSING) && model["available"].as_bool() == Some(true);
    if needs_model {
        if model["downloading"].as_bool() == Some(true) {
            let (done, total) = (model["done"].as_f64().unwrap_or(0.0), model["total"].as_f64().unwrap_or(1.0).max(1.0));
            let text = crate::i18n::tr_format!("Downloading Depth Model… {pct} %", pct = (100.0 * done / total).round() as i64);
            p.text(c, Align2::CENTER_CENTER, text, t.font(12.5), t.text_label);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        let gb = model["bytes"].as_f64().unwrap_or(0.0) / 1e9;
        let armed_id = egui::Id::unique("relightDownloadArmed");
        let armed = ui.ctx().data(|d| d.get_temp::<bool>(armed_id)).unwrap_or(false);
        let label = if armed {
            crate::i18n::tr_format!("Download {gb} GB From Hugging Face?", gb = format!("{gb:.2}"))
        } else {
            crate::i18n::tr_format!("Download Depth Model ({gb} GB)", gb = format!("{gb:.2}"))
        };
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_center_size(c, vec2(rect.width() - 16.0, 26.0)))
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        let b = crate::widgets::text_button(&mut child, "relightDownload", &label, armed);
        if b.clicked() {
            if armed {
                if let Err(e) = app.run("relight.downloadModel", json!({"confirm": true})) {
                    app.toast(ui.ctx(), e);
                }
                ui.ctx().data_mut(|d| d.remove::<bool>(armed_id));
            } else {
                ui.ctx().data_mut(|d| d.insert_temp(armed_id, true));
            }
        }
        if let Some(e) = model["error"].as_str() {
            let r = Rect::from_min_max(pos2(rect.left() + 8.0, c.y + 16.0), pos2(rect.right() - 8.0, rect.bottom() - 4.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            child.label(egui::RichText::new(e).size(11.0).color(t.text_dim));
        }
        return;
    }
    let text = match error {
        Some(e) => crate::i18n::tr_format!("No Depth: {e}", e = e),
        None => crate::i18n::tr("Finding Depth…").to_string(),
    };
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)));
    child.label(egui::RichText::new(text).size(12.0).color(t.text_label));
}
