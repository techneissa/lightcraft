//! Fork feature (`fork/glare-highlights.md`, issues #363 and #364): the Reflections section's boxes.
//!
//! The tool row (Box, Paint Glare, Erase Glare), the list of boxes, and the boxes on the photo, which are drawn only
//! while the Box tool is in hand: a click selects, a drag of the selected box moves it, a drag of its handle resizes
//! it, and a drag anywhere else draws one more. Boxes are percent of the original photo before crop or rotation, so everything here
//! edits them there and only draws them on the screen. Command: `reflection.box`.

use egui::{Color32, CursorIcon, Key, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, ReflectionArea, ReflectionMode, reflection_area::MAX_AREAS};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::icons::{Icon, paint};
use crate::panels::detail::geometry::CanvasMap;
use crate::reflections_glare::{self as glare, Area, Tool};
use crate::theme::Tokens;
use crate::widgets::register;

/// The boxes' colour on the photo: the same in every theme, as it lies on the picture, not the chrome (no theme token
/// is a yellow meant for that).
const BOX_YELLOW: Color32 = Color32::from_rgb(255, 196, 64);
/// A selected box's handles: corners and edge middles, as (across, down) from its middle.
const HANDLES: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];
/// How near a handle the pointer takes it, screen points.
const HANDLE_REACH: f32 = 7.0;
/// A drag shorter than this, either way, draws no box.
const MIN_DRAG: f32 = 4.0;
const ROW_H: f32 = 28.0;

/// A box being drawn or changed on the photo.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    /// Where it started, on the screen.
    from: Pos2,
    what: Grab,
}

/// What a press on the photo takes hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    New,
    Move(usize),
    Resize(usize, (i8, i8)),
}

/// `reflection.box`: `{add: {left, top, right, bottom}}`, `{update: index, left?, top?, right?, bottom?}`,
/// `{remove: index}`, `{clear: true}` or `{select: index | null}` (percent; indexes from 0). In Highlights mode a
/// changed box finds and fills the glare again.
pub fn command(app: &mut LightcraftApp, id: PhotoId, p: &Value) -> Result<Value, String> {
    let mut boxes = app.reflection_glare.area.boxes().to_vec();
    let index = |v: &Value| {
        v.as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < boxes.len())
            .ok_or_else(|| tr_format!("No box {n}: indexes start at 0", n = v))
    };
    if let Some(v) = p.get("select") {
        let selected = if v.is_null() { None } else { Some(index(v)?) };
        if selected.is_some() {
            glare::hold(app, Tool::Box);
        }
        app.reflection_glare.selected = selected;
        return Ok(glare::status(app));
    }
    let mut selected = app.reflection_glare.selected;
    if let Some(v) = p.get("add") {
        if boxes.len() >= MAX_AREAS {
            return Err(tr("Too many boxes").into());
        }
        boxes.push(glare::parse_box(v)?);
        selected = Some(boxes.len() - 1);
        glare::hold(app, Tool::Box);
    } else if let Some(v) = p.get("update") {
        let i = index(v)?;
        let Some(old) = boxes.get(i).cloned() else { return Err(tr("No such box").into()) };
        let mut side = serde_json::to_value(&old).map_err(|e| e.to_string())?;
        for key in ["left", "top", "right", "bottom"] {
            if let Some(v) = p.get(key) {
                side[key] = v.clone();
            }
        }
        let new = glare::parse_box(&side)?;
        selected = Some(i);
        glare::hold(app, Tool::Box);
        if new == old {
            app.reflection_glare.selected = selected;
            return Ok(glare::status(app));
        }
        if let Some(b) = boxes.get_mut(i) {
            *b = new;
        }
    } else if let Some(v) = p.get("remove") {
        let i = index(v)?;
        boxes.remove(i);
        selected = match selected {
            Some(s) if s == i => None,
            Some(s) if s > i => Some(s - 1),
            other => other,
        };
    } else if p.get("clear").and_then(Value::as_bool) == Some(true) {
        if boxes.is_empty() {
            return Ok(glare::status(app));
        }
        boxes.clear();
        selected = None;
    } else {
        return Err("reflection.box takes add, update, remove, clear or select".into());
    }
    app.reflection_glare.area = Area::Boxes(boxes);
    app.reflection_glare.selected = selected;
    glare::boxes_changed(app, id)?;
    Ok(glare::status(app))
}

fn run(app: &mut LightcraftApp, ctx: &egui::Context, params: Value) {
    if let Err(e) = app.run("reflection.box", params) {
        app.toast(ctx, e);
    }
}

fn box_name(i: usize) -> String {
    tr_format!("Box {n}", n = i + 1)
}

// ------------------------------------------------------------------------ panel

/// The tool row: Box, and in Highlights mode the glare brushes.
pub fn tool_row(app: &mut LightcraftApp, ui: &mut Ui) {
    let highlights = app.reflection_glare.mode == ReflectionMode::Highlights;
    let on = |t: Tool| app.reflection_glare.tool == Some(t);
    let tools = [(Tool::Box, "box"), (Tool::Paint, "paint"), (Tool::Erase, "erase")];
    // (the web build runs none of them: the section says so)
    let here = !cfg!(target_arch = "wasm32");
    let items = [
        ("reflection:tool:box", "Box", "Draw boxes on the photo (Shift+B)", on(Tool::Box), here),
        ("reflection:tool:paint", "Paint Glare", "Paint where there is glare", on(Tool::Paint), highlights && here),
        ("reflection:tool:erase", "Erase Glare", "Paint where there is no glare", on(Tool::Erase), highlights && here),
    ];
    let clicked = crate::panels::right::tile_row(ui, &items, &[Icon::GridSquare, Icon::Brush, Icon::Eraser]);
    if let Some((tool, name)) = clicked.and_then(|i| tools.get(i).copied()) {
        let name = if app.reflection_glare.tool == Some(tool) { "none" } else { name };
        if let Err(e) = app.run("reflection.tool", json!({"tool": name})) {
            app.toast(ui.ctx(), e);
        }
    }
}

fn row_id(i: usize) -> egui::Id {
    egui::Id::unique("reflectionBoxRow").with(i)
}

/// The boxes, one row each (a click or the arrows select, ✕ or Delete removes), and Clear All.
pub fn list(app: &mut LightcraftApp, ui: &mut Ui) {
    let n = app.reflection_glare.area.boxes().len();
    if n == 0 {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tr("Boxes")).size(11.5).color(t.text_dim));
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for i in 0..n {
            row(app, ui, i, n);
        }
    });
    if crate::widgets::text_button(ui, "reflection:boxes:clear", "Clear All", false).clicked() {
        run(app, ui.ctx(), json!({"clear": true}));
    }
}

fn row(app: &mut LightcraftApp, ui: &mut Ui, i: usize, n: usize) {
    let t = Tokens::get(ui.ctx());
    let selected = app.reflection_glare.selected == Some(i);
    let name = box_name(i);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let id = row_id(i);
    let resp = ui.interact(r, id, Sense::click());
    crate::widgets::announce_selected(&resp, name.clone(), selected);
    register(ui, format!("reflection:box:{i}"), r);
    let fill = if selected {
        t.tool_active
    } else if ui.rect_contains_pointer(r) {
        t.hover
    } else {
        t.inset
    };
    ui.painter().rect_filled(r, 4.0, fill);
    if resp.has_focus() {
        ui.painter().rect_stroke(r, 4.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    }
    paint(ui.painter(), Rect::from_min_size(r.min + vec2(6.0, (ROW_H - 16.0) / 2.0), vec2(16.0, 16.0)), Icon::GridSquare, t.text_label);
    ui.painter().text(pos2(r.left() + 28.0, r.center().y), egui::Align2::LEFT_CENTER, &name, t.font(13.0), t.text);

    let br = Rect::from_min_size(pos2(r.right() - 26.0, r.center().y - 11.0), vec2(22.0, 22.0));
    let remove = ui.interact(br, id.with("remove"), Sense::click());
    let label = tr_format!("Delete Box {n}", n = i + 1);
    remove.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
    register(ui, format!("reflection:box:{i}:remove"), br);
    if remove.hovered() || remove.has_focus() {
        ui.painter().rect_filled(br, 3.0, t.pressed);
    }
    paint(ui.painter(), br.shrink(5.0), Icon::Close, t.text);

    // a focused row holds the keyboard (the app's shortcuts wait): ↑ ↓ and Delete select and remove boxes here
    let (mut go, mut delete) = (None, false);
    if resp.has_focus() {
        let keys = egui::EventFilter { vertical_arrows: true, ..Default::default() };
        ui.memory_mut(|m| m.set_focus_lock_filter(id, keys));
        ui.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                go = i.checked_sub(1);
            }
            if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                go = Some(i + 1).filter(|k| *k < n);
            }
            delete = input.consume_key(Modifiers::NONE, Key::Delete) | input.consume_key(Modifiers::NONE, Key::Backspace);
        });
    }
    if remove.clicked() || delete {
        run(app, ui.ctx(), json!({"remove": i}));
        // the focus moves to a neighbouring row, which is selected with it
        if delete && n > 1 {
            let next = i.min(n - 2);
            ui.memory_mut(|m| m.request_focus(row_id(next)));
            run(app, ui.ctx(), json!({"select": next}));
        }
    } else if let Some(k) = go {
        ui.memory_mut(|m| m.request_focus(row_id(k)));
        run(app, ui.ctx(), json!({"select": k}));
    } else if resp.clicked() {
        resp.request_focus();
        run(app, ui.ctx(), json!({"select": i}));
    } else if resp.gained_focus() && !selected {
        // reached with Tab
        run(app, ui.ctx(), json!({"select": i}));
    }
}

// ------------------------------------------------------------------------ on the photo

/// Source fractions of the handle `(across, down)` of `a`.
fn handle_point(a: &ReflectionArea, (across, down): (i8, i8)) -> Point {
    let (l, t, r, b) = a.fractions();
    let pick = |side: i8, low: f64, high: f64| match side {
        -1 => low,
        1 => high,
        _ => (low + high) / 2.0,
    };
    Point::new(pick(across, l, r), pick(down, t, b))
}

/// `a` moved by `(dx, dy)` percent, stopping at the photo's edges.
fn moved(a: &ReflectionArea, dx: f64, dy: f64) -> ReflectionArea {
    let dx = if dx.is_finite() { dx.max(-a.left).min(100.0 - a.right) } else { 0.0 };
    let dy = if dy.is_finite() { dy.max(-a.top).min(100.0 - a.bottom) } else { 0.0 };
    ReflectionArea { left: a.left + dx, top: a.top + dy, right: a.right + dx, bottom: a.bottom + dy, mask: None }
}

/// `a` with the sides the handle holds moved to `to` (source fractions); a side dragged past the other one swaps with
/// it. `None` when nothing is left inside.
fn resized(a: &ReflectionArea, (across, down): (i8, i8), to: Point) -> Option<ReflectionArea> {
    let pc = |v: f64| if v.is_finite() { (v * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    let (mut l, mut t, mut r, mut b) = (a.left, a.top, a.right, a.bottom);
    match across {
        -1 => l = pc(to.x),
        1 => r = pc(to.x),
        _ => {}
    }
    match down {
        -1 => t = pc(to.y),
        1 => b = pc(to.y),
        _ => {}
    }
    ReflectionArea::boxed(l.min(r), t.min(b), l.max(r), t.max(b))
}

/// What `drag` makes with the pointer at `now`: a new box (`None`) or box `i` changed.
fn edited(drag: Drag, boxes: &[ReflectionArea], map: &CanvasMap, d: &DevelopSettings, now: Pos2) -> Option<(Option<usize>, ReflectionArea)> {
    let source = |q: Pos2| glare::to_source(map, d, q);
    match drag.what {
        Grab::New => {
            let r = Rect::from_two_pos(drag.from, now);
            if r.width().min(r.height()) < MIN_DRAG {
                return None;
            }
            // the box around all four corners: straightened or lens-corrected, they are not a box in the source
            glare::box_of([r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(source)).map(|a| (None, a))
        }
        Grab::Move(i) => {
            let (from, to) = (source(drag.from), source(now));
            boxes.get(i).map(|a| (Some(i), moved(a, (to.x - from.x) * 100.0, (to.y - from.y) * 100.0)))
        }
        Grab::Resize(i, h) => boxes.get(i).and_then(|a| resized(a, h, source(now))).map(|a| (Some(i), a)),
    }
}

/// The resize pointer for a handle that lies `v` from its box's middle on the screen.
fn resize_cursor(v: egui::Vec2) -> CursorIcon {
    let degrees = v.y.atan2(v.x).to_degrees().rem_euclid(180.0);
    match degrees {
        d if !(22.5..157.5).contains(&d) => CursorIcon::ResizeHorizontal,
        d if d < 67.5 => CursorIcon::ResizeNwSe,
        d if d < 112.5 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// The boxes on the photo while the Box tool is in hand.
pub fn overlay(app: &mut LightcraftApp, ui: &mut Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    let painter = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    let boxes = app.reflection_glare.area.boxes().to_vec();
    let selected = app.reflection_glare.selected.filter(|i| *i < boxes.len());
    let at = |a: &ReflectionArea, h: (i8, i8)| glare::to_screen(map, d, handle_point(a, h));
    // what lies under the pointer: a handle of the selected box, or a box (the selected one first)
    let under = |q: Pos2| {
        if let Some((i, a)) = selected.and_then(|i| boxes.get(i).map(|a| (i, a)))
            && let Some(h) = HANDLES.into_iter().find(|h| at(a, *h).distance(q) <= HANDLE_REACH)
        {
            return Some(Grab::Resize(i, h));
        }
        let n = glare::to_source(map, d, q);
        let inside = |a: &ReflectionArea| {
            let (l, t, r, b) = a.fractions();
            (l..=r).contains(&n.x) && (t..=b).contains(&n.y)
        };
        selected.filter(|i| boxes.get(*i).is_some_and(inside)).or_else(|| boxes.iter().rposition(inside)).map(Grab::Move)
    };
    // what a drag from there does: only the selected box is moved, anywhere else draws one more
    let grab = |q: Pos2| match under(q) {
        Some(Grab::Move(i)) if selected != Some(i) => Grab::New,
        other => other.unwrap_or(Grab::New),
    };
    if let Some(what) = app.reflection_glare.box_drag.map(|drag| drag.what).or_else(|| resp.hover_pos().map(grab)) {
        ui.ctx().set_cursor_icon(match what {
            Grab::New => CursorIcon::Crosshair,
            Grab::Move(_) => CursorIcon::Move,
            Grab::Resize(i, h) => boxes.get(i).map_or(CursorIcon::Crosshair, |a| resize_cursor(at(a, h) - at(a, (0, 0)))),
        });
    }
    let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if resp.drag_started()
        && let Some(q) = press
    {
        app.reflection_glare.box_drag = Some(Drag { from: q, what: grab(q) });
    }
    let drag = app.reflection_glare.box_drag;
    let change = drag.zip(resp.interact_pointer_pos()).and_then(|(drag, now)| edited(drag, &boxes, map, d, now));

    let outline = |a: &ReflectionArea, stroke: Stroke| {
        let corners = [(-1, -1), (1, -1), (1, 1), (-1, 1)].map(|h| at(a, h));
        painter.add(egui::Shape::closed_line(corners.to_vec(), stroke));
    };
    let chosen = Stroke::new(1.5, BOX_YELLOW);
    let faint = Stroke::new(1.0, BOX_YELLOW.gamma_multiply(0.6));
    let picked = app.reflection_glare.selected;
    for (i, a) in boxes.iter().enumerate() {
        let a = match &change {
            Some((Some(k), changed)) if *k == i => changed,
            _ => a,
        };
        outline(a, if picked == Some(i) { chosen } else { faint });
        if picked == Some(i) {
            for h in HANDLES {
                let square = Rect::from_center_size(at(a, h), vec2(7.0, 7.0));
                painter.rect(square, 0.0, BOX_YELLOW, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
            }
        }
    }
    if let Some((None, a)) = &change {
        outline(a, chosen);
    }

    if resp.drag_stopped() {
        app.reflection_glare.box_drag = None;
        if let Some((which, a)) = change {
            let sides = json!({"left": a.left, "top": a.top, "right": a.right, "bottom": a.bottom});
            let params = match which {
                None => json!({"add": sides}),
                Some(i) => {
                    let mut params = sides;
                    params["update"] = json!(i);
                    params
                }
            };
            run(app, ui.ctx(), params);
        }
    } else if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
    {
        match under(q) {
            Some(Grab::Move(i) | Grab::Resize(i, _)) => run(app, ui.ctx(), json!({"select": i})),
            _ => run(app, ui.ctx(), json!({"select": null})),
        }
    } else if drag.is_none()
        && let Some((i, a)) = selected.and_then(|i| boxes.get(i).map(|a| (i, a)))
    {
        keyboard(app, ui, map, d, i, a);
    }
}

/// The selected box by keyboard: a stand-in at its middle moves it with the arrows, one at its corner resizes it.
fn keyboard(app: &mut LightcraftApp, ui: &mut Ui, map: &CanvasMap, d: &DevelopSettings, i: usize, a: &ReflectionArea) {
    let size = map.rect.size();
    for (key, h, name) in [("reflectionBox", (0i8, 0i8), box_name(i)), ("reflectionBoxSize", (1, 1), tr_format!("Box {n} Size", n = i + 1))] {
        let point = handle_point(a, h);
        let screen = glare::to_screen(map, d, point);
        let keys = crate::canvas_keys::handle(ui, key, &name, crate::canvas_keys::position_text(point.x, point.y), screen);
        let Some((dx, dy)) = keys.delta else { continue };
        let to = glare::to_source(map, d, screen + vec2(dx as f32 * size.x, dy as f32 * size.y));
        let new = if h == (0, 0) { Some(moved(a, (to.x - point.x) * 100.0, (to.y - point.y) * 100.0)) } else { resized(a, h, to) };
        if let Some(b) = new {
            run(app, ui.ctx(), json!({"update": i, "left": b.left, "top": b.top, "right": b.right, "bottom": b.bottom}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::Headless;
    use std::time::Duration;

    fn request(h: &mut Headless, method: &str, params: Value) {
        let r = h.request(method, params, Duration::from_secs(20));
        assert_eq!(r["ok"], true, "{method}: {r}");
        h.step();
        h.step();
    }

    #[test]
    fn a_focused_box_row_is_the_selected_box_and_delete_removes_it_alone() {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default());
        app.ui.settings.gpu = false;
        let mut h = Headless::new(app, [1200.0, 2000.0], 1.0);
        request(&mut h, "ui.set", json!({"view": "detail", "right": "edit"}));
        request(&mut h, "engine.execute", json!({"command": "panel.remove"}));
        let add = |h: &mut Headless, left: u32| {
            // Reflections mode: a changed box runs no model
            h.app.run("reflection.box", json!({"add": {"left": left, "top": 10, "right": left + 20, "bottom": 30}})).unwrap();
            h.step();
        };
        for left in [10, 40, 70] {
            add(&mut h, left);
        }
        let boxes = |h: &Headless| h.app.reflection_glare.area.boxes().iter().map(|a| a.left).collect::<Vec<_>>();
        let selected = |h: &Headless| h.app.reflection_glare.selected;
        request(&mut h, "ui.clickWidget", json!({"id": "reflection:box:1"}));
        request(&mut h, "ui.key", json!({"key": "Delete"}));
        assert_eq!((boxes(&h), selected(&h)), (vec![10.0, 70.0], Some(1)), "the neighbouring row is selected");
        request(&mut h, "ui.key", json!({"key": "Delete"}));
        assert_eq!((boxes(&h), selected(&h)), (vec![10.0], Some(0)));
        // focus that arrives without a click selects too: from row 0 past its ✕ to row 1
        add(&mut h, 40);
        add(&mut h, 70);
        request(&mut h, "ui.key", json!({"key": "Tab"}));
        request(&mut h, "ui.key", json!({"key": "Tab"}));
        assert_eq!(selected(&h), Some(1));
    }
}
