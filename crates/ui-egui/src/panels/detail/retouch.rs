use super::geometry::CanvasMap;
use super::masks::{PIN_HIT, nearest_within, pin_lit, pins_live};
use super::{Color32, DevelopSettings, Gesture, LightcraftApp, Point, Pos2, Rect, Stroke, ToolOverlay, json, register, vec2};

// ------------------------------------------------------------------------ remove

/// Screen points a painted spot's pointer must move before the stroke takes another point.
pub(crate) const SPOT_POINT_SPACING: f32 = 1.0;

/// The Remove tool on the photo: every spot's outline and pin (the selected one with its source),
/// click a pin to select its spot, drag a target or source to move it, paint elsewhere to add one.
/// A spot's own circle setting, else the Tool Overlay mode (Shift+H), says whether its circle shows; a spot whose
/// circle is not showing can't be grabbed, so a click there adds a new one. A spot hidden with its eye (no effect)
/// is grabbed like any other while its circle shows.
pub(super) fn remove_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    let long = (map.rect.width().max(map.rect.height())) as f64;
    let active = app.session.active_spot.filter(|i| *i < d.spots.len());
    let (mode, live) = (app.ui.tool_overlay, pins_live(app, resp, map));
    // Auto also hides them while a new spot is painted
    let painting = matches!(app.gesture, Some(Gesture::Spot { .. }));
    let p = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    let mut quiet = p.clone();
    quiet.set_invisible();
    // (spot, is source, screen centre, radius) of everything that can be grabbed
    let mut grips: Vec<(usize, bool, Pos2, f32)> = Vec::new();
    // each pin to draw (spot, position, selected, painter), after the loop so the one under the pointer shows lit
    let mut drawn: Vec<(usize, Pos2, bool, &egui::Painter)> = Vec::new();
    for (i, s) in d.spots.iter().enumerate() {
        let r = (s.size * long) as f32;
        let sel = Some(i) == active;
        // the spot's own circle setting (its row in the panel) beats the Tool Overlay
        let own = crate::panels::right::spot_overlay(s, mode);
        if !own.shows(sel, live) {
            continue;
        }
        let p = if painting && own == ToolOverlay::Auto { &quiet } else { &p };
        let col = Color32::from_white_alpha(if sel { 230 } else { 150 });
        // (a spot in an object's shape shows its outline, `object_remove`)
        if !super::object_remove::outline(ui.ctx(), p, map, s, sel) {
            for q in &s.points {
                p.circle_stroke(map.screen(*q), r, Stroke::new(if sel { 1.5 } else { 1.0 }, col));
            }
        }
        let Some(&t) = s.points.first() else { continue };
        let tq = map.screen(t);
        if let Some(o) = s.source_offset.filter(|_| sel) {
            let sq = map.screen(Point::new(t.x + o.x, t.y + o.y));
            for q in &s.points {
                p.circle_stroke(map.screen(Point::new(q.x + o.x, q.y + o.y)), r, Stroke::new(1.0, Color32::from_white_alpha(170)));
            }
            let dir = (tq - sq).normalized();
            p.arrow(sq + dir * r, (tq - sq) - dir * (2.0 * r).min((tq - sq).length()), Stroke::new(1.0, Color32::from_white_alpha(200)));
            register(ui, format!("spotSource:{i}"), Rect::from_center_size(sq, vec2(14.0, 14.0)));
            grips.push((i, true, sq, r));
        }
        drawn.push((i, tq, sel, p));
        register(ui, format!("spotPin:{i}"), Rect::from_center_size(tq, vec2(14.0, 14.0)));
        grips.push((i, false, tq, r));
    }
    let hit = |q: Pos2| spot_grip_at(&grips, active, q);
    // the pin under the pointer shows lit (a spot's source has none)
    draw_spot_pins(&drawn, resp.hover_pos().and_then(hit).filter(|g| !g.1).map(|g| g.0));
    if spot_keys(app, ui, map, d, &grips) {
        return;
    }
    if super::find_overlay::show(app, ui, resp, map) {
        return;
    }
    // ⌃⌥-drag resizes the brush (sideways its size, up and down its feather) instead of painting or moving a pin
    if super::brush_resize::resize_drag(app, ui, resp, &p, long as f32, 1000.0, true) {
        return;
    }
    let r = (app.ui.remove_size as f64 * long) as f32;
    let hover = resp.hover_pos().and_then(hit);
    if let Some(h) = resp.hover_pos() {
        if hover.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if mode == ToolOverlay::Never {
            // nothing covers the photo at Never: a small pointer shows where the brush is
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            register(ui, "removeBrushRing", Rect::from_center_size(h, vec2(2.0 * r, 2.0 * r)));
            p.circle_stroke(h, r, Stroke::new(1.0, Color32::WHITE));
            p.circle_stroke(h, r * (1.0 - app.ui.remove_feather / 100.0).max(0.05), Stroke::new(1.0, Color32::from_white_alpha(110)));
        }
    }
    let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if resp.drag_started()
        && let Some((spot, source, ..)) = press.and_then(hit)
    {
        if Some(spot) != active {
            let _ = app.run("spot.select", json!({"index": spot}));
        }
        let _ = app.run("develop.beginInteraction", json!({"label": "Edit Spot"}));
        app.gesture = Some(Gesture::SpotMove { spot, source });
    }
    if let Some(Gesture::SpotMove { spot, source }) = app.gesture.clone() {
        if resp.dragged()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let (n, n0) = (map.norm(q), map.norm(q - resp.drag_delta()));
            let dn = [n.x - n0.x, n.y - n0.y];
            if dn != [0.0, 0.0] {
                let _ = app.run("spot.update", json!({"index": spot, if source { "moveSource" } else { "move" }: dn}));
            }
        }
        if resp.drag_stopped() {
            app.gesture = None;
            let _ = app.run("develop.endInteraction", json!({}));
        }
        return;
    }
    if resp.clicked()
        && let Some((spot, ..)) = press.and_then(hit)
    {
        let _ = app.run("spot.select", json!({"index": spot}));
        return;
    }
    if let Some(q) = resp.interact_pointer_pos()
        && (resp.drag_started() || resp.dragged() || resp.clicked())
    {
        let n = map.norm(q);
        match &mut app.gesture {
            // (a press held still adds no point: a long press made a spot of hundreds of the same point, and every
            // pixel of its box is measured against each of them)
            Some(Gesture::Spot { points }) => {
                if points.last().is_none_or(|l| map.screen(*l).distance(q) >= SPOT_POINT_SPACING) {
                    points.push(n);
                }
            }
            _ => app.gesture = Some(Gesture::Spot { points: vec![n] }),
        }
    }
    if mode != ToolOverlay::Never
        && let Some(Gesture::Spot { points }) = &app.gesture
    {
        for q in points {
            p.circle_filled(map.screen(*q), r, Color32::from_white_alpha(60));
        }
        register(ui, "removePaintFill", map.rect);
    }
    if (resp.drag_stopped() || resp.clicked())
        && let Some(Gesture::Spot { points }) = app.gesture.take()
    {
        // Object mode: the object under the click or stroke (`object_remove`)
        if super::object_remove::commit(app, ui.ctx(), &points, resp.clicked()) {
            return;
        }
        let mode = match app.ui.tool.as_str() {
            "heal" => "heal",
            "clone" => "clone",
            "fill" => "fill",
            _ => "remove",
        };
        let pts: Vec<[f64; 2]> = points.iter().map(|q| [q.x, q.y]).collect();
        let params = json!({"mode": mode, "points": pts, "size": app.ui.remove_size, "feather": app.ui.remove_feather, "opacity": app.ui.remove_opacity, "background": true});
        add_spot(app, mode, params);
    }
}

/// A target or source under `q`: the selected spot's first, then the nearest pin (at least [`PIN_HIT`] wide, so a
/// small spot's pin stays easy to hit).
fn spot_grip_at(grips: &[(usize, bool, Pos2, f32)], active: Option<usize>, q: Pos2) -> Option<(usize, bool, Pos2, f32)> {
    grips
        .iter()
        .filter(|g| g.2.distance(q) < g.3.max(PIN_HIT))
        .min_by(|a, b| {
            (Some(a.0) != active, a.2.distance(q)).partial_cmp(&(Some(b.0) != active, b.2.distance(q))).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
}

/// Draw each spot pin (spot, position, selected, painter); the `lit` spot's shows under the pointer.
fn draw_spot_pins(drawn: &[(usize, Pos2, bool, &egui::Painter)], lit: Option<usize>) {
    for &(i, q, sel, painter) in drawn {
        pin_lit(painter, q, sel, lit == Some(i));
    }
}

/// Keyboard stand-ins for the Remove tool: each spot's target and source (an arrow press moves
/// it a step, one undo step; Return selects the spot), and a placement point that the arrows move
/// and Return turns into a new spot of the brush's size there. True when a spot was added.
fn spot_keys(app: &mut LightcraftApp, ui: &mut egui::Ui, map: &CanvasMap, d: &DevelopSettings, grips: &[(usize, bool, Pos2, f32)]) -> bool {
    for &(spot, source, q, _) in grips {
        let at = map.norm(q);
        let name =
            format!("{} {}{}", crate::i18n::tr("Spot"), spot + 1, if source { format!(" {}", crate::i18n::tr("source")) } else { String::new() });
        let size = d.spots.get(spot).map(|s| s.size).unwrap_or_default();
        let value = format!("{}, {} {:.0}%", crate::canvas_keys::position_text(at.x, at.y), crate::i18n::tr("size"), size * 100.0);
        let keys = crate::canvas_keys::handle(ui, &format!("spot:{spot}:{}", if source { "source" } else { "target" }), &name, value, q);
        if (keys.activated || keys.delta.is_some()) && app.session.active_spot != Some(spot) {
            let _ = app.run("spot.select", json!({"index": spot}));
        }
        if let Some((dx, dy)) = keys.delta {
            let _ = app.run("develop.beginInteraction", json!({"label": "Edit Spot"}));
            let _ = app.run("spot.update", json!({"index": spot, if source { "moveSource" } else { "move" }: [dx, dy]}));
            let _ = app.run("develop.endInteraction", json!({}));
        }
    }
    // where a new spot goes: the photo's centre at first, then wherever the arrows took it
    let key = egui::Id::unique("lc-spot-placement");
    let place: (f64, f64) = ui.data(|m| m.get_temp(key)).unwrap_or((0.5, 0.5));
    let q = map.screen(Point::new(place.0, place.1));
    let keys = crate::canvas_keys::handle(
        ui,
        "spot:new",
        crate::i18n::tr("New spot (Return adds it here)"),
        crate::canvas_keys::position_text(place.0, place.1),
        q,
    );
    if let Some((dx, dy)) = keys.delta {
        let moved = ((place.0 + dx).clamp(0.0, 1.0), (place.1 + dy).clamp(0.0, 1.0));
        ui.data_mut(|m| m.insert_temp(key, moved));
    }
    if crate::canvas_keys::has_focus(ui.ctx()) && ui.memory(|m| m.has_focus(crate::canvas_keys::id("spot:new"))) {
        let r = (app.ui.remove_size as f64 * (map.rect.width().max(map.rect.height())) as f64) as f32;
        ui.painter().circle_stroke(q, r, Stroke::new(1.0, Color32::WHITE));
    }
    if keys.activated {
        let mode = match app.ui.tool.as_str() {
            "heal" => "heal",
            "clone" => "clone",
            "fill" => "fill",
            _ => "remove",
        };
        let params = json!({"mode": mode, "points": [[place.0, place.1]], "size": app.ui.remove_size, "feather": app.ui.remove_feather, "opacity": app.ui.remove_opacity, "background": true});
        add_spot(app, mode, params);
        return true;
    }
    false
}

// ------------------------------------------------------------------------ red eye

/// What a press in the Red Eye tool can grab.
#[derive(Clone, Copy, Debug, PartialEq)]
enum EyeGrip {
    /// An eye's pin (any eye): drag moves it, a click selects it.
    Pin(usize),
    /// The selected eye's edge handle: 1 right, 2 bottom, 3 left, 4 top; a drag sets that radius.
    Edge(usize, u8),
}

/// Red Eye tool: drag an ellipse over an eye (a new correction), click one to select it. Every eye has a pin
/// (click selects, drag moves); the selected one has a handle on each edge (drag resizes about its centre), and
/// Delete removes it (`redeye.delete`).
pub(super) fn eye_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    // screen points per long-edge unit
    let o = map.screen(Point::new(0.0, 0.0));
    let (sx, sy) = (map.screen(Point::new(1.0, 0.0)).distance(o), map.screen(Point::new(0.0, 1.0)).distance(o));
    let per_long = sx.max(sy);
    let p = ui.painter();
    let selected = app.ui.eye.min(d.red_eye.len().saturating_sub(1));
    // everything that can be grabbed: (what, screen position)
    let mut grips: Vec<(EyeGrip, Pos2)> = Vec::new();
    for (i, e) in d.red_eye.iter().enumerate() {
        let c = map.screen(e.center);
        let r = vec2(e.rx as f32 * per_long, e.ry as f32 * per_long);
        if i == selected {
            for (h, at) in [(1, vec2(r.x, 0.0)), (2, vec2(0.0, r.y)), (3, vec2(-r.x, 0.0)), (4, vec2(0.0, -r.y))] {
                grips.push((EyeGrip::Edge(i, h), c + at));
            }
        }
        grips.push((EyeGrip::Pin(i), c));
    }
    // the handles first (they sit on the edge, away from the pin unless the eye is tiny), then the selected eye's
    // pin, then the nearest pin
    let hit = |q: Pos2| {
        let rank = |g: &(EyeGrip, Pos2)| (matches!(g.0, EyeGrip::Pin(_)), !matches!(g.0, EyeGrip::Pin(i) if i == selected), g.1.distance(q));
        grips.iter().filter(|g| g.1.distance(q) < PIN_HIT).min_by(|a, b| rank(a).partial_cmp(&rank(b)).unwrap_or(std::cmp::Ordering::Equal)).copied()
    };
    let dragging = match app.gesture {
        Some(Gesture::EyeEdit { index, handle }) => Some((index, handle)),
        _ => None,
    };
    let lit = match dragging {
        Some((i, 0)) => Some(EyeGrip::Pin(i)),
        Some((i, h)) => Some(EyeGrip::Edge(i, h)),
        None => resp.hover_pos().and_then(hit).map(|g| g.0),
    };
    for (i, e) in d.red_eye.iter().enumerate() {
        let c = map.screen(e.center);
        let r = vec2(e.rx as f32 * per_long, e.ry as f32 * per_long);
        let sel = i == selected;
        let col = if sel { Color32::WHITE } else { Color32::from_white_alpha(150) };
        p.add(egui::Shape::ellipse_stroke(c, r, Stroke::new(if sel { 1.5 } else { 1.0 }, col)));
        pin_lit(p, c, sel, lit == Some(EyeGrip::Pin(i)));
        register(ui, format!("eyePin:{i}"), Rect::from_center_size(c, vec2(14.0, 14.0)));
    }
    for &(g, at) in &grips {
        if let EyeGrip::Edge(i, h) = g {
            let on = lit == Some(g);
            let blue = Color32::from_rgb(1, 101, 221);
            if on {
                p.circle_filled(at, 5.0, blue);
            }
            p.circle_stroke(at, 5.0, Stroke::new(1.5, if on { Color32::WHITE } else { blue }));
            register(ui, format!("eyeHandle:{i}:{h}"), Rect::from_center_size(at, vec2(14.0, 14.0)));
        }
    }
    if resp.hover_pos().is_some() {
        ui.ctx().set_cursor_icon(match lit {
            Some(EyeGrip::Pin(_)) => egui::CursorIcon::Grab,
            Some(EyeGrip::Edge(_, h)) if h % 2 == 1 => egui::CursorIcon::ResizeHorizontal,
            Some(EyeGrip::Edge(..)) => egui::CursorIcon::ResizeVertical,
            None => egui::CursorIcon::Crosshair,
        });
    }
    let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if resp.drag_started()
        && let Some(grip) = press.and_then(hit).map(|g| g.0)
    {
        let (index, handle) = match grip {
            EyeGrip::Pin(i) => (i, 0),
            EyeGrip::Edge(i, h) => (i, h),
        };
        app.ui.eye = index;
        let _ = app.run("develop.beginInteraction", json!({"label": "Edit Red Eye"}));
        app.gesture = Some(Gesture::EyeEdit { index, handle });
    } else if resp.drag_started()
        && let Some(q) = press
    {
        // from where the button went down (a drag is recognized only after some movement)
        app.gesture = Some(Gesture::Eye { a: map.norm(q) });
    }
    if let Some((index, handle)) = dragging {
        if resp.dragged()
            && let (Some(q), Some(e)) = (resp.interact_pointer_pos(), d.red_eye.get(index))
        {
            let c = map.screen(e.center);
            let params = match handle {
                0 => {
                    let (n, n0) = (map.norm(q), map.norm(q - resp.drag_delta()));
                    json!({"index": index, "center": [e.center.x + n.x - n0.x, e.center.y + n.y - n0.y]})
                }
                1 | 3 => json!({"index": index, "rx": f64::from((q.x - c.x).abs() / per_long.max(1e-3)).max(0.002)}),
                _ => json!({"index": index, "ry": f64::from((q.y - c.y).abs() / per_long.max(1e-3)).max(0.002)}),
            };
            let _ = app.run("redeye.update", params);
        }
        if resp.drag_stopped() {
            app.gesture = None;
            let _ = app.run("develop.endInteraction", json!({}));
        }
        return;
    }
    if let Some(Gesture::Eye { a }) = app.gesture
        && let Some(q) = resp.interact_pointer_pos()
    {
        let (a, b) = (map.screen(a), q);
        p.add(egui::Shape::ellipse_stroke(a.lerp(b, 0.5), ((b - a) / 2.0).abs(), Stroke::new(1.0, Color32::WHITE)));
    }
    if resp.drag_stopped()
        && let Some(Gesture::Eye { a }) = app.gesture.take()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let (sa, sb) = (map.screen(a), q);
        let half = ((sb - sa) / 2.0).abs() / per_long;
        if half.x > 0.002 && half.y > 0.002 {
            let c = map.norm(sa.lerp(sb, 0.5));
            let r = app.run("redeye.add", json!({"center": [c.x, c.y], "rx": half.x, "ry": half.y, "pet": app.ui.eye_pet}));
            if let Ok(v) = r {
                app.ui.eye = v["index"].as_u64().unwrap_or(0) as usize;
            }
        }
    } else if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
    {
        // a pin (any eye), else inside an ellipse; the pin's reach is at least PIN_HIT wide, so a tiny eye stays clickable
        let inside = |e: &lightcraft_develop::RedEye| {
            let c = map.screen(e.center);
            let (dx, dy) = ((q.x - c.x) / (e.rx as f32 * per_long).max(PIN_HIT), (q.y - c.y) / (e.ry as f32 * per_long).max(PIN_HIT));
            dx * dx + dy * dy <= 1.0
        };
        let at_pin = nearest_within(d.red_eye.iter().map(|e| map.screen(e.center)), q, PIN_HIT)
            .and_then(|c| d.red_eye.iter().position(|e| map.screen(e.center) == c));
        if let Some(i) = at_pin.or_else(|| d.red_eye.iter().position(inside)) {
            app.ui.eye = i;
        }
    }
}

/// Run `spot.add` with `params` (the fill of `mode` added), timed under `LIGHTCRAFT_PROFILE`.
fn add_spot(app: &mut LightcraftApp, mode: &str, params: serde_json::Value) {
    let t = lightcraft_engine::media::dirty::clock();
    let _ = app.run("spot.add", crate::panels::right::with_fill(app, mode, params));
    if lightcraft_pipeline::profiling() {
        use lightcraft_engine::media::dirty::{clock, ms};
        eprintln!("  ui: spot.add {:.1} ms (at {:.1})", ms(t, clock()), crate::render::stamp());
    }
}
