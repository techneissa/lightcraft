//! Fork feature (`fork/glare-highlights.md`, issues #73 and #74): the Reflections section's mode, area and glare.
//!
//! - **Mode**: Reflections (a model separates a reflection in glass) or Highlights (glare is found and filled).
//! - **Area**: the whole photo, a box drawn on the photo, or a mask of the Masks panel. A model runs on the area at the
//!   panel's Quality; Highlights looks for glare only there.
//! - **Highlights**: Threshold and Expand for the finder, the fill (LaMa or Classic), and strokes painted on the photo
//!   to add to or erase from the found glare (each stroke recalculates).
//!
//! The panel keeps these as what the next run uses; a finished run saves them with its result, and opening a photo
//! takes them back from it. Commands: `reflection.mode`, `reflection.area`, `reflection.glare`, `reflection.paint`,
//! `reflection.tool`, and their parameters on `reflection.apply`. Coordinates are percent of the original photo before
//! crop or rotation.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Glare, GlareStroke, Reflection, ReflectionArea, ReflectionMode, SpotFill};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::panels::detail::geometry::CanvasMap;

/// What the photo's tool does while the Reflections section uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Box,
    Paint,
    Erase,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Box => "box",
            Tool::Paint => "paint",
            Tool::Erase => "erase",
        }
    }
}

/// The panel's choices for the next run, and the tool on the photo.
#[derive(Clone, Debug, Default)]
pub struct GlarePanel {
    pub mode: ReflectionMode,
    pub area: Option<ReflectionArea>,
    pub glare: Glare,
    pub tool: Option<Tool>,
    /// The drag in progress: points as fractions of the source.
    pub drag: Vec<Point>,
    /// A stroke came while a run was going: run again when it ends.
    pub rerun: bool,
}

/// Take the panel's choices back from a photo's saved result.
pub fn load(app: &mut LightcraftApp, r: &Reflection) {
    let p = &mut app.reflection_glare;
    p.tool = None;
    p.drag.clear();
    p.rerun = false;
    p.mode = r.mode;
    p.area = r.area.clone();
    p.glare = r.glare.clone().sanitized();
}

fn area_json(a: &Option<ReflectionArea>) -> Value {
    match a {
        None => json!("whole"),
        Some(a) => serde_json::to_value(a).unwrap_or(Value::Null),
    }
}

/// The panel's state, for `reflection.status`.
pub fn status(app: &LightcraftApp) -> Value {
    let p = &app.reflection_glare;
    json!({"mode": p.mode.name(), "area": area_json(&p.area), "glare": p.glare, "tool": p.tool.map(Tool::name)})
}

/// An area from a command's `area` value: `"whole"` or null, `{left, top, right, bottom}` (percent), or `{mask: id}`.
fn parse_area(v: &Value, d: &DevelopSettings) -> Result<Option<ReflectionArea>, String> {
    if v.is_null() || v.as_str() == Some("whole") || v.get("whole").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    if let Some(id) = v.get("mask") {
        let id = id.as_u64().and_then(|i| u32::try_from(i).ok()).ok_or("mask must be a mask id")?;
        if !d.masks.iter().any(|m| m.id == id) {
            return Err(format!("No mask {id} on this photo"));
        }
        return Ok(Some(ReflectionArea { mask: Some(id), ..Default::default() }));
    }
    let n = |k: &str| v.get(k).and_then(Value::as_f64).ok_or(format!("area needs left, top, right and bottom (percent) or mask: {k} is missing"));
    let a = ReflectionArea::boxed(n("left")?, n("top")?, n("right")?, n("bottom")?);
    a.map(Some).ok_or_else(|| "area: each side 0–100, left below right and top below bottom".into())
}

/// Apply a command's `mode`, `area`, `threshold`, `expand` and `fill` to the panel.
pub fn take_params(app: &mut LightcraftApp, id: PhotoId, p: &Value) -> Result<(), String> {
    if let Some(m) = p.get("mode") {
        let m = m.as_str().and_then(ReflectionMode::from_name).ok_or("mode is reflections or highlights")?;
        app.reflection_glare.mode = m;
    }
    if let Some(a) = p.get("area") {
        let d = app.session.develop_of(id).ok_or("Photo missing")?;
        app.reflection_glare.area = parse_area(a, &d)?;
    }
    let g = &mut app.reflection_glare.glare;
    for (key, slot) in [("threshold", &mut g.threshold), ("expand", &mut g.expand)] {
        if let Some(v) = p.get(key) {
            let v = v.as_f64().filter(|v| v.is_finite() && (0.0..=100.0).contains(v)).ok_or(format!("{key} is a number 0–100"))?;
            *slot = v;
        }
    }
    if let Some(f) = p.get("fill") {
        g.fill = match f.as_str() {
            Some("lama") => SpotFill::Lama,
            Some("classic") => SpotFill::Classic,
            _ => return Err("fill is lama or classic".into()),
        };
    }
    Ok(())
}

/// The commands of this module; `None` for another name.
pub fn command(app: &mut LightcraftApp, name: &str, p: &Value) -> Option<Result<Value, String>> {
    let id = match app.session.active() {
        Some(id) => id,
        None => {
            return matches!(name, "reflection.mode" | "reflection.area" | "reflection.glare" | "reflection.paint" | "reflection.tool")
                .then(|| Err("No photo selected".into()));
        }
    };
    let result = match name {
        "reflection.mode" | "reflection.area" | "reflection.glare" => {
            let key = name.trim_start_matches("reflection.");
            let params = if key == "glare" || p.get(key).is_some() { p.clone() } else { json!({ key: p.clone() }) };
            take_params(app, id, &params).map(|_| status(app))
        }
        "reflection.tool" => match p.get("tool").and_then(Value::as_str) {
            Some("box") => Ok(Some(Tool::Box)),
            Some("paint") => Ok(Some(Tool::Paint)),
            Some("erase") => Ok(Some(Tool::Erase)),
            Some("none") | None => Ok(None),
            Some(other) => Err(format!("Unknown tool {other}: box, paint, erase or none")),
        }
        .map(|t| {
            app.reflection_glare.tool = t;
            app.reflection_glare.drag.clear();
            status(app)
        }),
        "reflection.paint" => paint(app, id, p),
        _ => return None,
    };
    Some(result)
}

/// `reflection.paint`: add a stroke (`points` [[x, y], …] in percent of the photo, `size` the radius in percent of its
/// long edge, `erase`), or `clear` them all; in Highlights mode the glare is found and filled again.
fn paint(app: &mut LightcraftApp, id: PhotoId, p: &Value) -> Result<Value, String> {
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        app.reflection_glare.glare.paint.clear();
    } else {
        let points: Vec<Point> = p
            .get("points")
            .and_then(Value::as_array)
            .ok_or("points: [[x, y], …] in percent of the photo")?
            .iter()
            .filter_map(|q| Some(Point::new(q.get(0)?.as_f64()? / 100.0, q.get(1)?.as_f64()? / 100.0)))
            .collect();
        let size = p.get("size").and_then(Value::as_f64).map(|s| s / 100.0).unwrap_or(app.ui.remove_size as f64);
        let erase = p.get("erase").and_then(Value::as_bool).unwrap_or(false);
        let stroke = GlareStroke { erase, size, points }.sanitized().ok_or("The stroke has no usable points")?;
        let strokes = &mut app.reflection_glare.glare.paint;
        if strokes.len() >= lightcraft_develop::reflection_area::MAX_GLARE_STROKES {
            return Err("Too many strokes: clear them first".into());
        }
        strokes.push(stroke);
    }
    app.reflection_glare.mode = ReflectionMode::Highlights;
    let running = app.tasks.is_running(crate::reflections::TASK);
    if running {
        app.reflection_glare.rerun = true;
    } else {
        crate::reflections::run(app, id, &json!({}))?;
    }
    Ok(json!({"strokes": app.reflection_glare.glare.paint.len(), "background": true}))
}

/// After a run: start the one a stroke asked for while it went.
pub fn after_run(app: &mut LightcraftApp) {
    if std::mem::take(&mut app.reflection_glare.rerun)
        && let Some(id) = app.session.active()
        && let Err(e) = crate::reflections::run(app, id, &json!({}))
    {
        log::warn!("reflection rerun: {e}");
    }
}

// ------------------------------------------------------------------------ runs

/// Start the run the panel asks for, when it is not the whole-photo reflection model (that one stays in
/// `crate::reflections`). `None`: not ours.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(app: &mut LightcraftApp, id: PhotoId, edge: u32, model: &str) -> Option<Result<Value, String>> {
    let panel = app.reflection_glare.clone();
    if panel.mode == ReflectionMode::Reflections && panel.area.is_none() {
        return None;
    }
    Some(start_job(app, id, edge, model.to_string(), panel))
}

#[cfg(not(target_arch = "wasm32"))]
fn start_job(app: &mut LightcraftApp, id: PhotoId, edge: u32, model: String, panel: GlarePanel) -> Result<Value, String> {
    use lightcraft_engine::reflections::{area, highlights};
    use std::sync::atomic::AtomicBool;
    // the whole photo at its own size: an area keeps all its detail
    let job = app.session.render_job(id, 1 << 15, 1 << 15, false, false).ok_or("Photo cannot be loaded")?;
    let learned = (panel.glare.fill == SpotFill::Lama && app.session.lama.installed()).then(lightcraft_pipeline::learned_fill::current).flatten();
    let generation = app.session.library_generation();
    let key = job.source_key;
    let identity = lightcraft_engine::reflections::identity(key);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let highlights = panel.mode == ReflectionMode::Highlights;
    crate::tasks::spawn_tracked(
        app,
        crate::reflections::TASK,
        "reflection",
        cancel,
        move |handle| -> Result<(Reflection, Option<String>), String> {
            handle.detail(if highlights { "Finding glare" } else { "Area" });
            let settings = job.settings.clone();
            let source = job.source.load_source()?;
            let src = source.image_for(settings.denoise_amount());
            let info = source.info_or(job.info.clone());
            let region = match &panel.area {
                Some(a) => area::resolve(a, &settings, &src, info.mattes.as_deref())?,
                None => area::Region::whole(),
            };
            if highlights {
                let made = highlights::make(&src, &panel.glare, &region, learned, &flag)?;
                let note = (panel.glare.fill == SpotFill::Lama && made.fill == SpotFill::Classic)
                    .then(|| "LaMa is not installed: the Classic fill was used".to_string());
                let mut r = made.reflection;
                r.area = panel.area.clone();
                Ok((area::save(&made.tiles, key, r)?, note))
            } else {
                let a = panel.area.clone().unwrap_or_default();
                Ok((area::make(&src, key, edge, &model, &region, a)?, None))
            }
        },
        move |app, ctx, result| {
            if generation == app.session.library_generation() {
                finish(app, ctx, id, &identity, result);
            }
            after_run(app);
            ctx.request_repaint();
        },
    )?;
    Ok(json!({"background": true, "photo": id.0, "mode": panel_mode_name(highlights)}))
}

fn panel_mode_name(highlights: bool) -> &'static str {
    if highlights { "highlights" } else { "reflections" }
}

#[cfg(not(target_arch = "wasm32"))]
fn finish(app: &mut LightcraftApp, ctx: &egui::Context, id: PhotoId, identity: &str, result: Result<(Reflection, Option<String>), String>) {
    match result {
        Ok((mut made, note)) => {
            let current = app.session.render_job(id, 384, 384, false, false).map(|j| lightcraft_engine::reflections::identity(j.source_key));
            if current.as_deref() != Some(identity) {
                app.toast(ctx, tr("The photo changed; calculate reflections again"));
                return;
            }
            let Some(d) = app.session.develop_of(id) else { return };
            let mut d = (*d).clone();
            // a recalculation of the same kind keeps the chosen Amount
            if !d.reflection.correction.is_empty() && d.reflection.mode == made.mode {
                made.amount = d.reflection.amount;
                made.limit_region = d.reflection.limit_region;
                (made.left, made.top, made.right, made.bottom) = (d.reflection.left, d.reflection.top, d.reflection.right, d.reflection.bottom);
            }
            let label = if made.mode == ReflectionMode::Highlights { "Remove Glare" } else { "Remove Reflections" };
            d.reflection = made;
            match app.session.set_develop(id, d, label) {
                Ok(()) => app.toast(ctx, note.unwrap_or_else(|| tr("Reflections calculated; adjust Amount on the right").into())),
                Err(e) => app.toast(ctx, e.to_string()),
            }
        }
        Err(e) if e == "Stopped" => {}
        Err(e) => app.toast(ctx, format!("{}: {e}", tr("Reflection removal failed"))),
    }
}

// ------------------------------------------------------------------------ panel

fn spec(id: &'static str, label: &'static str, default: f64) -> lightcraft_develop::ControlSpec {
    lightcraft_develop::ControlSpec {
        id,
        label,
        section: lightcraft_develop::Section::Detail,
        min: 0.0,
        max: 100.0,
        default,
        step: 1.0,
        decimals: 0,
        track: lightcraft_develop::Track::Plain,
    }
}

/// The Mode row.
pub fn mode_row(app: &mut LightcraftApp, ui: &mut egui::Ui, busy: bool) {
    use crate::widgets::register;
    ui.horizontal(|ui| {
        ui.add_sized([58.0, 22.0], egui::Label::new(tr("Mode")));
        let mut mode = app.reflection_glare.mode;
        let label = |m: ReflectionMode| if m == ReflectionMode::Highlights { tr("Highlights") } else { tr("Reflections") };
        let r = crate::widgets::combo_box("reflection-mode").selected_text(label(mode)).show_ui(ui, |ui| {
            for m in [ReflectionMode::Reflections, ReflectionMode::Highlights] {
                let r = ui.add_enabled(!busy, egui::Button::selectable(mode == m, label(m)));
                if r.clicked() {
                    mode = m;
                }
                register(ui, format!("reflection:mode:{}", m.name()), r.rect);
            }
        });
        crate::widgets::name_combo(&r.response, tr("Mode"));
        register(ui, "reflection:mode", r.response.rect);
        app.reflection_glare.mode = mode;
    });
}

/// The Area row, and its Draw Box button.
pub fn area_row(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, busy: bool) {
    use crate::widgets::register;
    let mask_name =
        |id: u32| d.masks.iter().find(|m| m.id == id).map(|m| if m.name.is_empty() { format!("{} {id}", tr("Mask")) } else { m.name.clone() });
    ui.horizontal(|ui| {
        ui.add_sized([58.0, 22.0], egui::Label::new(tr("Area")));
        let current = match &app.reflection_glare.area {
            None => tr("Whole Photo").to_string(),
            Some(a) => match a.mask {
                Some(id) => mask_name(id).unwrap_or_else(|| tr("Missing Mask").into()),
                None => tr("Box").to_string(),
            },
        };
        let mut pick: Option<Option<ReflectionArea>> = None;
        let mut draw = false;
        let r = crate::widgets::combo_box("reflection-area").selected_text(current).show_ui(ui, |ui| {
            let r = ui.add_enabled(!busy, egui::Button::selectable(app.reflection_glare.area.is_none(), tr("Whole Photo")));
            register(ui, "reflection:area:whole", r.rect);
            if r.clicked() {
                pick = Some(None);
            }
            let boxed = app.reflection_glare.area.as_ref().is_some_and(|a| a.mask.is_none());
            let r = ui.add_enabled(!busy, egui::Button::selectable(boxed, tr("Box")));
            register(ui, "reflection:area:box", r.rect);
            if r.clicked() {
                draw = true;
            }
            if !d.masks.is_empty() {
                ui.separator();
            }
            for m in &d.masks {
                let on = app.reflection_glare.area.as_ref().and_then(|a| a.mask) == Some(m.id);
                let r = ui.add_enabled(!busy, egui::Button::selectable(on, mask_name(m.id).unwrap_or_default()));
                register(ui, format!("reflection:area:mask:{}", m.id), r.rect);
                if r.clicked() {
                    pick = Some(Some(ReflectionArea { mask: Some(m.id), ..Default::default() }));
                }
            }
        });
        crate::widgets::name_combo(&r.response, tr("Area"));
        register(ui, "reflection:area", r.response.rect);
        if let Some(a) = pick {
            app.reflection_glare.area = a;
            app.reflection_glare.tool = None;
        }
        if draw {
            app.reflection_glare.tool = Some(Tool::Box);
        }
    });
    if app.reflection_glare.tool == Some(Tool::Box) {
        ui.add(egui::Label::new(tr("Drag a box on the photo.")).wrap());
    }
}

/// The Highlights rows: Threshold, Expand, Fill and the brush buttons.
pub fn highlights_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, busy: bool) {
    use crate::widgets::register;
    for (id, label, default) in [
        ("reflection.threshold", "Threshold", lightcraft_develop::reflection_area::DEFAULT_GLARE_THRESHOLD),
        ("reflection.expand", "Expand", lightcraft_develop::reflection_area::DEFAULT_GLARE_EXPAND),
    ] {
        let s = spec(id, label, default);
        let g = &app.reflection_glare.glare;
        let value = if id == "reflection.threshold" { g.threshold } else { g.expand };
        let out = crate::widgets::slider(ui, &s, value, !busy, None);
        if let Some(v) = out.value.map(|v| s.clamp(v)).or(out.reset.then_some(default)) {
            let g = &mut app.reflection_glare.glare;
            if id == "reflection.threshold" { g.threshold = v } else { g.expand = v }
        }
    }
    ui.horizontal(|ui| {
        ui.add_sized([58.0, 22.0], egui::Label::new(tr("Fill")));
        let mut fill = app.reflection_glare.glare.fill;
        let name = |f: SpotFill| if f == SpotFill::Lama { "LaMa" } else { "Classic" };
        let r = crate::widgets::combo_box("reflection-fill").selected_text(tr(name(fill))).show_ui(ui, |ui| {
            for f in [SpotFill::Lama, SpotFill::Classic] {
                let r = ui.selectable_value(&mut fill, f, tr(name(f)));
                register(ui, format!("reflection:fill:{}", name(f).to_lowercase()), r.rect);
            }
        });
        crate::widgets::name_combo(&r.response, tr("Fill"));
        register(ui, "reflection:fill", r.response.rect);
        app.reflection_glare.glare.fill = fill;
    });
    if app.reflection_glare.glare.fill == SpotFill::Lama && !app.session.lama.installed() {
        ui.add(egui::Label::new(tr("LaMa is not installed: the Classic fill is used.")).wrap());
    }
    ui.horizontal(|ui| {
        for (tool, label) in [(Tool::Paint, "Paint Glare"), (Tool::Erase, "Erase Glare")] {
            let on = app.reflection_glare.tool == Some(tool);
            let r = ui.add_enabled(!busy, egui::Button::selectable(on, tr(label)));
            register(ui, format!("reflection:tool:{}", tool.name()), r.rect);
            if r.clicked() {
                app.reflection_glare.tool = if on { None } else { Some(tool) };
            }
        }
        let has = !app.reflection_glare.glare.paint.is_empty();
        let r = ui.add_enabled(!busy && has, egui::Button::new(tr("Clear")));
        register(ui, "reflection:paint:clear", r.rect);
        if r.clicked()
            && let Err(e) = app.run("reflection.paint", json!({"clear": true}))
        {
            app.toast(ui.ctx(), e);
        }
    });
}

// ------------------------------------------------------------------------ on the photo

/// Source fractions of a point of the photo as shown (`map`'s coordinates on the photo as shot).
fn to_source(map: &CanvasMap, d: &DevelopSettings, q: egui::Pos2) -> Point {
    let n = map.for_mask(true).norm(q);
    lightcraft_pipeline::redeye::oriented_to_source(d.orientation, n)
}

/// Screen position of source fractions `p`.
fn to_screen(map: &CanvasMap, d: &DevelopSettings, p: Point) -> egui::Pos2 {
    map.for_mask(true).screen(lightcraft_pipeline::redeye::oriented_to_source(d.orientation.inverse(), p))
}

/// The section's tool on the photo, while it is on: draws the area box and the strokes, takes a dragged box or a painted
/// stroke. Says whether it took the pointer.
pub fn overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) -> bool {
    use egui::{Color32, Stroke};
    if !app.reflection_open {
        return false;
    }
    let p = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    let col = Color32::from_rgb(255, 196, 64);
    if let Some(a) = app.reflection_glare.area.as_ref().filter(|a| a.mask.is_none()) {
        let (l, t, r, b) = a.fractions();
        let corners = [Point::new(l, t), Point::new(r, t), Point::new(r, b), Point::new(l, b)].map(|c| to_screen(map, d, c));
        p.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(1.5, col)));
    }
    let Some(tool) = app.reflection_glare.tool else { return false };
    let long = map.rect.width().max(map.rect.height());
    let radius = app.ui.remove_size * long;
    if let Some(h) = resp.hover_pos() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if tool != Tool::Box {
            p.circle_stroke(h, radius, Stroke::new(1.0, Color32::WHITE));
        }
    }
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        app.reflection_glare.drag = vec![to_source(map, d, q)];
    }
    if resp.dragged()
        && let Some(q) = resp.interact_pointer_pos()
        && !app.reflection_glare.drag.is_empty()
    {
        let n = to_source(map, d, q);
        let drag = &mut app.reflection_glare.drag;
        if tool == Tool::Box {
            drag.truncate(1);
        }
        if drag.len() < lightcraft_develop::reflection_area::MAX_STROKE_POINTS {
            drag.push(n);
        }
    }
    let drag = app.reflection_glare.drag.clone();
    match tool {
        Tool::Box => {
            if let (Some(a), Some(b)) = (drag.first(), drag.last()) {
                p.rect_stroke(
                    egui::Rect::from_two_pos(to_screen(map, d, *a), to_screen(map, d, *b)),
                    0.0,
                    Stroke::new(1.0, Color32::WHITE),
                    egui::StrokeKind::Middle,
                );
            }
        }
        _ => {
            let pts: Vec<egui::Pos2> = drag.iter().map(|q| to_screen(map, d, *q)).collect();
            p.add(egui::Shape::line(pts, Stroke::new(2.0 * radius, Color32::from_white_alpha(70))));
        }
    }
    if resp.drag_stopped() {
        let drag = std::mem::take(&mut app.reflection_glare.drag);
        let result = match (tool, drag.first(), drag.last()) {
            (Tool::Box, Some(a), Some(b)) => {
                let pc = |v: f64| (v * 100.0).clamp(0.0, 100.0);
                let area = json!({"left": pc(a.x.min(b.x)), "top": pc(a.y.min(b.y)), "right": pc(a.x.max(b.x)), "bottom": pc(a.y.max(b.y))});
                app.run("reflection.area", json!({"area": area})).map(|_| app.reflection_glare.tool = None)
            }
            (Tool::Paint | Tool::Erase, Some(_), _) => {
                let size = app.ui.remove_size as f64 * 100.0;
                let points: Vec<[f64; 2]> = drag.iter().map(|q| [q.x * 100.0, q.y * 100.0]).collect();
                app.run("reflection.paint", json!({"points": points, "size": size, "erase": tool == Tool::Erase})).map(|_| ())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            app.toast(ui.ctx(), e);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> LightcraftApp {
        LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default())
    }

    #[test]
    fn mode_area_and_glare_are_set_by_commands_and_bad_values_are_refused() {
        let mut app = app();
        app.run("reflection.mode", json!({"mode": "highlights"})).unwrap();
        app.run("reflection.area", json!({"area": {"left": 10, "top": 20, "right": 40, "bottom": 50}})).unwrap();
        app.run("reflection.glare", json!({"threshold": 70, "expand": 5, "fill": "classic"})).unwrap();
        let s = app.run("reflection.status", json!({})).unwrap()["panel"].clone();
        assert_eq!(s["mode"], "highlights");
        assert_eq!(s["area"]["left"], 10.0);
        assert_eq!(s["glare"]["threshold"], 70.0);
        assert_eq!(s["glare"]["fill"], "classic");
        app.run("reflection.area", json!({"area": "whole"})).unwrap();
        assert_eq!(app.run("reflection.status", json!({})).unwrap()["panel"]["area"], "whole");
        for bad in [
            json!({"area": {"left": 50, "top": 0, "right": 10, "bottom": 100}}),
            json!({"area": {"left": 0}}),
            json!({"area": {"mask": 999}}),
            json!({"threshold": 101}),
            json!({"fill": "magic"}),
            json!({"mode": "glass"}),
        ] {
            assert!(app.run("reflection.glare", bad.clone()).is_err() || app.run("reflection.area", bad.clone()).is_err(), "{bad}");
        }
        assert!(app.run("reflection.tool", json!({"tool": "lasso"})).is_err());
        app.run("reflection.tool", json!({"tool": "paint"})).unwrap();
        assert_eq!(app.reflection_glare.tool, Some(Tool::Paint));
    }

    #[test]
    fn a_painted_stroke_is_kept_and_starts_a_run() {
        let mut app = app();
        let r = app.run("reflection.paint", json!({"points": [[50, 50], [52, 50]], "size": 2})).unwrap();
        assert_eq!(r["strokes"], 1);
        assert_eq!(app.reflection_glare.mode, ReflectionMode::Highlights);
        assert!((app.reflection_glare.glare.paint[0].size - 0.02).abs() < 1e-9);
        assert!(app.reflection_glare.glare.paint[0].points[0].x == 0.5);
        assert!(app.run("reflection.paint", json!({"points": []})).is_err());
        assert!(app.run("reflection.paint", json!({"points": [["a", 1]]})).is_err());
        assert!(app.tasks.is_running(crate::reflections::TASK));
        // a second stroke while the first runs waits for it
        app.run("reflection.paint", json!({"points": [[10, 10]], "erase": true})).unwrap();
        assert_eq!(app.reflection_glare.glare.paint.len(), 2);
        assert!(app.reflection_glare.rerun);
        app.run("reflection.paint", json!({"clear": true})).unwrap();
        assert!(app.reflection_glare.glare.paint.is_empty());
    }

    #[test]
    fn a_photo_takes_back_its_saved_choices() {
        let mut app = app();
        let r = Reflection {
            mode: ReflectionMode::Highlights,
            area: ReflectionArea::boxed(1.0, 2.0, 3.0, 4.0),
            glare: Glare { threshold: 80.0, ..Default::default() },
            ..Default::default()
        };
        app.reflection_glare.tool = Some(Tool::Box);
        load(&mut app, &r);
        assert_eq!(app.reflection_glare.mode, ReflectionMode::Highlights);
        assert_eq!(app.reflection_glare.area, r.area);
        assert_eq!(app.reflection_glare.glare.threshold, 80.0);
        assert_eq!(app.reflection_glare.tool, None);
    }
}
