//! The Refine brush of every Portrait feature (fork, `fork/refine-brush.md`, issue #99): one row under each feature's
//! sliders (Reduce, Restore and, for a feature that reads a mask, Add), the brush on the photo, and the Show Refine
//! Strokes overlay. Every feature gets it from the tab's section loop (`portrait.rs`), with nothing of its own.
//!
//! The brush is the Masking brush's: its Size, Feather and Flow (`UiState::brush_*`, the right-click brush panel, the
//! ⌃⌥-drag resize), pen pressure, and ⌥ held to swap Reduce and Restore (Add with ⌥ restores). A stroke ends in
//! `retouch.refine.stroke` (one undo step), for every face or the picked one; points are on the photo before its
//! crop, after lens corrections and transform, as Remove spots. UI command: `portrait.refine {feature?, mode?,
//! strength?, show?}`.

use egui::{Color32, Pos2, Stroke, vec2};
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{FEATURES, FeatureInfo, Input, KeyMap, Refine, RefineMode};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::panels::detail::{CanvasMap, Gesture};
use crate::widgets::register;

/// The tool: which feature's brush is in hand and how it paints, and whether the strokes show.
#[derive(Clone, Debug)]
pub struct RefineTool {
    /// The feature the brush paints for (`None`: no brush; the photo takes clicks as usual).
    pub feature: Option<&'static str>,
    pub mode: RefineMode,
    /// 0…100.
    pub strength: f32,
    /// Show Refine Strokes.
    pub show: bool,
}

impl Default for RefineTool {
    fn default() -> Self {
        RefineTool { feature: None, mode: RefineMode::Reduce, strength: 100.0, show: false }
    }
}

/// The strokes the picked face renders with (its own, else those for every face).
pub(super) fn strokes_for<'a>(d: &'a DevelopSettings, input: &Input, face: Option<usize>, keys: Option<&dyn KeyMap>) -> &'a Refine {
    let own = face.and_then(|i| lightcraft_develop::retouch::overrides::resolve_in(&d.retouch, &input.faces, keys).get(i).copied().flatten());
    own.and_then(|o| d.retouch.faces.get(o)).map_or(&d.retouch.refine, |o| &o.refine)
}

/// Whether the Refine brush works on `f`: every feature the retouch run applies (not the geometry step).
pub(super) fn refinable(f: &FeatureInfo) -> bool {
    f.stage != lightcraft_develop::retouch::Stage::Geometry
}

/// The mode a stroke paints in: `mode`, or with ⌥ held its other one.
fn swapped(mode: RefineMode, alt: bool) -> RefineMode {
    match (mode, alt) {
        (m, false) => m,
        (RefineMode::Restore, true) => RefineMode::Reduce,
        (_, true) => RefineMode::Restore,
    }
}

fn colour(mode: RefineMode) -> Color32 {
    match mode {
        RefineMode::Reduce => Color32::from_rgba_unmultiplied(230, 80, 80, 110),
        RefineMode::Restore => Color32::from_rgba_unmultiplied(90, 150, 240, 110),
        RefineMode::Add => Color32::from_rgba_unmultiplied(90, 205, 120, 110),
    }
}

/// The Refine row of feature `f`'s section.
pub(super) fn row(cx: &mut super::Ctx<'_>, f: &FeatureInfo) {
    // (Face Reshape moves pixels in the geometry step, outside the run Refine wraps: no brush for it yet)
    if !refinable(f) {
        return;
    }
    let keys = cx.keys.as_ref().map(|k| k as &dyn KeyMap);
    let count = strokes_for(cx.d, &cx.input, cx.face, keys).count(Some(f.key));
    let tool = cx.app.portrait_refine.clone();
    let mut pick: Option<Option<RefineMode>> = None;
    let mut clear = false;
    cx.ui.horizontal(|ui| {
        // (the sliders' rows keep 10 pt from the panel's edge)
        ui.add_space(10.0);
        ui.label(egui::RichText::new(tr("Refine")).size(12.0));
        for mode in RefineMode::ALL {
            if mode == RefineMode::Add && f.add_parts.is_empty() {
                continue;
            }
            let on = tool.feature == Some(f.key) && tool.mode == mode;
            let r = ui.selectable_label(on, tr(mode.label()));
            register(ui, format!("portrait:refine.{}.{}", f.key, mode.name()), r.rect);
            if r.clicked() {
                pick = Some((!on).then_some(mode));
            }
        }
    });
    if count > 0 {
        cx.ui.horizontal(|ui| {
            ui.add_space(10.0);
            let n = if count == 1 { tr("1 Stroke").to_string() } else { crate::i18n::tr_format!("{n} Strokes", n = count) };
            ui.label(egui::RichText::new(n).size(11.5).weak());
            let r = ui.small_button(tr("Clear")).on_hover_text(tr("Clear Refine Strokes"));
            register(ui, format!("portrait:refine.{}.clear", f.key), r.rect);
            clear = r.clicked();
        });
    }
    if let Some(choice) = pick {
        let p = match choice {
            Some(m) => json!({"feature": f.key, "mode": m.name()}),
            None => json!({"feature": null}),
        };
        if let Err(e) = cx.app.run("portrait.refine", p) {
            cx.app.toast(cx.ui.ctx(), e);
        }
    }
    if clear && let Err(e) = cx.app.run("retouch.refine.clear", json!({"feature": f.key, "face": cx.face})) {
        cx.app.toast(cx.ui.ctx(), e);
    }
    if cx.app.portrait_refine.feature == Some(f.key) {
        if cx.app.portrait_refine.mode == RefineMode::Add && cx.input.faces.is_empty() {
            cx.ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(egui::RichText::new(tr("Add works on or near a face, and none was found.")).size(11.5).weak());
            });
        }
        brush_sliders(cx.app, cx.ui);
    }
}

/// Size, Feather and Flow (the Masking brush's, as its panel draws them) and Strength, while the brush is in hand.
fn brush_sliders(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    use lightcraft_develop::controls::{ControlSpec, Section, Track};
    for (id, label, min, max, get) in [
        ("ui.brushSize", "Size", 1.0, 100.0, f64::from(app.ui.brush_size * 400.0)),
        ("ui.brushFeather", "Feather", 0.0, 100.0, f64::from(app.ui.brush_feather)),
        ("ui.brushFlow", "Flow", 1.0, 100.0, f64::from(app.ui.brush_flow)),
        ("portrait.refineStrength", "Strength", 0.0, 100.0, f64::from(app.portrait_refine.strength)),
    ] {
        let s = ControlSpec { id, label, section: Section::Portrait, min, max, default: min, step: 1.0, decimals: 0, track: Track::Plain };
        let out = crate::widgets::slider(ui, &s, get, true, None);
        if let Some(v) = out.value {
            match id {
                "ui.brushSize" => app.ui.brush_size = (v / 400.0) as f32,
                "ui.brushFeather" => app.ui.brush_feather = v as f32,
                "ui.brushFlow" => app.ui.brush_flow = v as f32,
                _ => app.portrait_refine.strength = v as f32,
            }
        }
    }
}

/// The "Show Refine Strokes" switch, under the line that says which faces the sliders change.
pub(super) fn show_switch(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let mut show = app.portrait_refine.show;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 22, top: 2, bottom: 0 }).show(ui, |ui| {
        let r = ui.checkbox(&mut show, tr("Show Refine Strokes"));
        register(ui, "portrait:refineStrokes", r.rect);
    });
    if show != app.portrait_refine.show {
        let _ = app.run("portrait.refine", json!({"show": show}));
    }
}

/// `portrait.refine {feature?: key | null, mode?, strength?, show?}`: take the Refine brush of a feature (`null`
/// puts it down), its mode and strength, and Show Refine Strokes. Opens the Portrait tab when a brush is taken.
pub(super) fn command(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let id = "portrait.refine";
    let mut tool = app.portrait_refine.clone();
    match p.get("feature") {
        None => {}
        Some(Value::Null) => tool.feature = None,
        Some(v) => {
            let key = v.as_str().ok_or_else(|| format!("{id}: `feature` is a feature's key or null"))?;
            let f = FEATURES
                .iter()
                .find(|f| f.key == key)
                .ok_or_else(|| format!("{id}: unknown feature `{key}` ({})", FEATURES.iter().map(|f| f.key).collect::<Vec<_>>().join("|")))?;
            if !refinable(f) {
                return Err(format!("{id}: {} has no Refine brush yet", f.label));
            }
            tool.feature = Some(f.key);
        }
    }
    if let Some(m) = p.get("mode") {
        let m = m.as_str().and_then(RefineMode::from_name).ok_or_else(|| format!("{id}: `mode` is reduce, restore or add"))?;
        tool.mode = m;
    }
    if let Some(f) = tool.feature.and_then(|k| FEATURES.iter().find(|f| f.key == k))
        && tool.mode == RefineMode::Add
        && f.add_parts.is_empty()
    {
        return Err(format!("{id}: {} reads no mask, so it has no Add", f.label));
    }
    if let Some(v) = p.get("strength") {
        let v = v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("{id}: `strength` is a number 0–100"))?;
        tool.strength = v.clamp(0.0, 100.0) as f32;
    }
    if let Some(v) = p.get("show") {
        tool.show = v.as_bool().ok_or_else(|| format!("{id}: `show` is true or false"))?;
    }
    if tool.feature.is_some() {
        app.ui.right = crate::state::RightPanel::Portrait;
    }
    app.portrait_refine = tool.clone();
    Ok(json!({"feature": tool.feature, "mode": tool.mode.name(), "strength": tool.strength, "show": tool.show}))
}

/// The photo's long side on screen (its frame before the crop, in which brush sizes are kept).
fn frame_long(map: &CanvasMap) -> f32 {
    use lightcraft_geom::Point;
    let o = map.screen(Point::new(0.0, 0.0));
    let (x, y) = (map.screen(Point::new(1.0, 0.0)), map.screen(Point::new(0.0, 1.0)));
    (x - o).length().max((y - o).length())
}

/// The strokes of the picked face (or every face) over the photo, while Show Refine Strokes is on.
pub(crate) fn overlay(app: &LightcraftApp, ui: &egui::Ui, map: &CanvasMap, d: &DevelopSettings, photo: lightcraft_catalog::PhotoId) {
    if !app.portrait_refine.show || app.ui.right != crate::state::RightPanel::Portrait {
        return;
    }
    let st = app.session.retouch_faces_of(photo);
    let input = st.found.as_ref().map(|f| f.input.clone()).unwrap_or_default();
    let face = super::picked(ui.ctx(), photo).filter(|i| *i < input.faces.len());
    let keys = app.session.retouch_key_map(photo);
    let refine = strokes_for(d, &input, face, keys.as_ref().map(|k| k as &dyn KeyMap));
    let p = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    let long = frame_long(map);
    // the brush's feature first when it is in hand; the others fainter
    for layer in &refine.0 {
        let faint = app.portrait_refine.feature.is_some_and(|k| k != layer.feature);
        for s in &layer.strokes {
            let pts: Vec<Pos2> = s.points.iter().map(|(x, y)| map.screen(lightcraft_geom::Point::new(*x, *y))).collect();
            let c = colour(s.mode);
            path(&p, &pts, s.size as f32 * long, if faint { c.gamma_multiply(0.4) } else { c });
        }
    }
}

/// A stroke `r` points wide on each side of `pts`, with round ends: a band along the points and a half disc past
/// each end (no part painted twice at the ends, so a translucent colour stays even there).
fn path(p: &egui::Painter, pts: &[Pos2], r: f32, c: Color32) {
    // (points closer than a pixel add nothing but kinks)
    let mut line: Vec<Pos2> = Vec::with_capacity(pts.len());
    for q in pts {
        if line.last().is_none_or(|l: &Pos2| l.distance(*q) >= 1.0) {
            line.push(*q);
        }
    }
    match line.as_slice() {
        [] => {}
        [one] => {
            p.circle_filled(*one, r, c);
        }
        _ => {
            p.add(egui::Shape::line(line.clone(), Stroke::new(2.0 * r, c)));
            if let (Some(a), Some(b), Some(y), Some(z)) = (line.first(), line.get(1), line.get(line.len().saturating_sub(2)), line.last()) {
                cap(p, *a, *a - *b, r, c);
                cap(p, *z, *z - *y, r, c);
            }
        }
    }
}

/// The half disc of radius `r` at `at` on the side `out` points to.
fn cap(p: &egui::Painter, at: Pos2, out: egui::Vec2, r: f32, c: Color32) {
    let d = out.normalized();
    if !d.x.is_finite() || !d.y.is_finite() || r <= 0.0 {
        return;
    }
    let n = egui::vec2(-d.y, d.x);
    let steps = 16;
    let pts: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / steps as f32 - 0.5);
            at + n * (r * a.sin()) + d * (r * a.cos())
        })
        .collect();
    p.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
}

/// The brush on the photo while a Refine brush is in hand: its cursor, the stroke being painted and its commit.
/// False (nothing done) without a brush in hand.
pub(crate) fn canvas(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap) -> bool {
    let Some(key) = app.portrait_refine.feature else { return false };
    let Some(photo) = app.session.active() else { return false };
    let p = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    let long = frame_long(map);
    let r = app.ui.brush_size * long;
    let alt = ui.input(|i| i.modifiers.alt);
    let mode = swapped(app.portrait_refine.mode, alt);
    // ⌃⌥-drag resizes the brush (sideways its size, up and down its feather) instead of painting
    if crate::panels::detail::brush_resize::resize_drag(app, ui, resp, &p, long, 400.0, false) {
        return true;
    }
    if let Some(h) = resp.hover_pos() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        p.circle_stroke(h, r, Stroke::new(1.0, Color32::WHITE));
        p.circle_stroke(h, r * (1.0 - app.ui.brush_feather / 100.0).max(0.05), Stroke::new(1.0, Color32::from_white_alpha(120)));
        // − takes the feature away, + brings it back or adds it
        let arm = (r * 0.25).clamp(3.0, 7.0);
        for (w, c) in [(3.0, Color32::from_black_alpha(160)), (1.5, Color32::WHITE)] {
            let line = Stroke::new(w, c);
            p.line_segment([h - vec2(arm, 0.0), h + vec2(arm, 0.0)], line);
            if mode != RefineMode::Reduce {
                p.line_segment([h - vec2(0.0, arm), h + vec2(0.0, arm)], line);
            }
        }
    }
    if let Some(q) = resp.interact_pointer_pos()
        && (resp.drag_started() || resp.dragged() || resp.clicked())
    {
        let n = map.norm(q);
        let force = if app.ui.brush_pressure { ui.input(crate::panels::detail::masks::pen_force) } else { None };
        match &mut app.gesture {
            // a held, unmoving pointer records nothing
            Some(Gesture::Brush { points, .. }) if points.last() == Some(&n) => {}
            Some(Gesture::Brush { points, pressure }) => {
                crate::panels::detail::masks::record_pressure(pressure, points.len(), force);
                points.push(n);
            }
            _ => {
                let mut pressure = Vec::new();
                crate::panels::detail::masks::record_pressure(&mut pressure, 0, force);
                app.gesture = Some(Gesture::Brush { points: vec![n], pressure });
            }
        }
    }
    if let Some(Gesture::Brush { points, .. }) = &app.gesture {
        let pts: Vec<Pos2> = points.iter().map(|q| map.screen(*q)).collect();
        path(&p, &pts, r, colour(mode));
    }
    if (resp.drag_stopped() || resp.clicked())
        && let Some(Gesture::Brush { points, pressure }) = app.gesture.take()
    {
        let pts: Vec<[f64; 2]> = points.iter().map(|q| [q.x, q.y]).collect();
        let face = super::picked(ui.ctx(), photo);
        let params = json!({"feature": key, "mode": mode.name(), "points": pts, "size": app.ui.brush_size, "feather": app.ui.brush_feather,
            "flow": app.ui.brush_flow, "strength": app.portrait_refine.strength, "pressure": pressure, "face": face});
        if let Err(e) = app.run("retouch.refine.stroke", params) {
            app.toast(ui.ctx(), e);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_swaps_reduce_and_restore() {
        assert_eq!(swapped(RefineMode::Reduce, true), RefineMode::Restore);
        assert_eq!(swapped(RefineMode::Restore, true), RefineMode::Reduce);
        assert_eq!(swapped(RefineMode::Add, true), RefineMode::Restore);
        assert_eq!(swapped(RefineMode::Add, false), RefineMode::Add);
    }
}
