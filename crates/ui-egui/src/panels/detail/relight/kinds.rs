//! Fork (issue #300, `fork/relight.md`): the selected light's handles for its type, on the photo. The light's own
//! handle (its place) is drawn and moved by `detail/relight.rs`; here are the others:
//!
//! - **Spot:** the aim (a second dot: the cone points at the surface under it) and a grip on the cone's edge (drag it to
//!   widen or narrow the cone, in degrees as the photo shows them).
//! - **Sun / Window:** an arrow from the light's handle to a tip: its direction on the photo is the sun's azimuth, its
//!   length the height (the longest arrow is a sun overhead, a short one a low sun in front of the scene). A hollow tip: the sun is
//!   behind the scene. With its rays between clouds on (issue #330), a sun symbol where they centre (placed, or found from
//!   the sky): drag it onto the sun in the photo (or above the frame) to place it.
//! - **Softbox:** the rectangle (the handle is its middle), a corner grip that resizes it and a knob above its top
//!   edge that turns it, and a tip on an arrow from its middle: the way it shines (issue #335; like the sun's arrow, its
//!   direction on the photo is the direction the box leans to and its length how far; hollow while it shines straight in).
//! - **Rim, Fill:** no place, so no handle (the row in the list selects them).
//!
//! A drag of one of these is one undo step; ⌥ while it is held shows where the light reaches (`relight_alt`).

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use serde_json::json;

use super::super::geometry::CanvasMap;
use super::super::{LightcraftApp, Point, register};
use crate::panels::edit::light_tint;
use lightcraft_develop::relight::LightSettings;

/// How near a press must be to a handle to take it (points).
const HIT: f32 = 12.0;
/// How far the sun's arrow reaches at a sun on the horizon (points).
const SUN_LEN: f32 = 110.0;
/// The shortest the sun's arrow is drawn (points).
const SUN_MIN: f32 = 26.0;
/// How far the softbox's turning knob sits above its top edge (points).
const KNOB: f32 = 26.0;
/// How far the softbox's aim arrow reaches at a box turned fully side on, and the shortest it is drawn (points).
const AIM_LEN: f32 = 110.0;
const AIM_MIN: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Handle {
    Aim,
    Cone,
    SunTip,
    BoxSize,
    BoxTurn,
    BoxAim,
    RaysCentre,
}

fn grab_id() -> egui::Id {
    egui::Id::unique("relightKindDrag")
}

/// The aim arrow's true tip and the pointer's place at the press, both from the light's handle (kept while the drag goes): the
/// arrow is drawn at least [`AIM_MIN`] out, so a drag takes the pointer's moves from the press and not its place (issue #335).
fn aim_grab_id() -> egui::Id {
    egui::Id::unique("relightKindAimGrab")
}

/// A drag of one of these handles that is going when the handles stop showing ends there (like the light's own).
pub(super) fn release(app: &mut LightcraftApp, ui: &egui::Ui, showing: bool) {
    if !showing && ui.ctx().data(|m| m.get_temp::<Handle>(grab_id())).is_some() {
        ui.ctx().data_mut(|m| m.remove::<Handle>(grab_id()));
        app.ui.dragging_control = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
}

/// `v` turned by `degrees` (clockwise on the screen, whose y runs down).
fn turned(v: egui::Vec2, degrees: f32) -> egui::Vec2 {
    let (s, c) = degrees.to_radians().sin_cos();
    vec2(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// The photo's width and height on the screen (points).
fn photo_size(map: &CanvasMap) -> (f32, f32) {
    let (o, x, y) = (map.screen(Point::new(0.0, 0.0)), map.screen(Point::new(1.0, 0.0)), map.screen(Point::new(0.0, 1.0)));
    ((x - o).length().max(1.0), (y - o).length().max(1.0))
}

/// The spot's axis on the screen: the unit vector from the light to its aim, and the cone's reach (points).
fn spot_axis(map: &CanvasMap, l: &LightSettings, c: Pos2) -> (egui::Vec2, f32) {
    let aim = map.screen(Point::new(l.kind.spot_aim_x, l.kind.spot_aim_y));
    let v = aim - c;
    let len = v.length();
    let dir = if len > 1.0 { v / len } else { vec2(0.0, 1.0) };
    (dir, len.max(48.0))
}

/// The sun's arrow's tip on the screen. A sun in front of the scene near the horizon has no arrow on the photo; its
/// tip then shows a little above the handle.
fn sun_tip(l: &LightSettings, c: Pos2) -> Pos2 {
    let v = lightcraft_relight::kinds::sun_vector(l.kind.sun_azimuth, l.kind.sun_elevation);
    let arrow = vec2(v[0], v[1]) * SUN_LEN;
    let len = arrow.length();
    if len >= SUN_MIN {
        c + arrow
    } else if len > 1e-3 {
        c + arrow / len * SUN_MIN
    } else {
        c + vec2(0.0, -SUN_MIN)
    }
}

/// The tip of the softbox's aim arrow on the screen: the box's axis seen from the camera times [`AIM_LEN`], kept a little off
/// the handle so a box shining straight in still has a tip to take (a hollow one, below its middle).
fn aim_tip(l: &LightSettings, c: Pos2) -> Pos2 {
    let arrow = aim_offset(l.kind.box_aim_h, l.kind.box_aim_v);
    let len = arrow.length();
    if len >= AIM_MIN {
        c + arrow
    } else if len > 1e-3 {
        c + arrow / len * AIM_MIN
    } else {
        c + vec2(0.0, AIM_MIN)
    }
}

/// The box's axis seen from the camera times [`AIM_LEN`]: where the aim arrow's tip is from the light's handle, at its true
/// length (a tip nearer than [`AIM_MIN`] is drawn pinned out, [`aim_tip`]).
fn aim_offset(aim_h: f64, aim_v: f64) -> egui::Vec2 {
    let n = lightcraft_relight::kinds::box_normal(aim_h, aim_v);
    vec2(n[0], n[1]) * AIM_LEN
}

/// The aim (tilts in degrees, right and down) for an arrow tip `p` from the light's handle (points).
fn aim_of_offset(mut p: egui::Vec2) -> (f64, f64) {
    p /= AIM_LEN;
    let reach = lightcraft_relight::kinds::AIM_MAX.to_radians().sin() as f32;
    if p.length() > reach {
        p *= reach / p.length();
    }
    let z = (1.0 - p.length_sq()).max(0.0).sqrt();
    let deg = |a: f32| f64::from(a.atan2(z).to_degrees()).round().clamp(-80.0, 80.0);
    (deg(p.x), deg(p.y))
}

/// The aim a drag gives: the arrow's true tip `base` at the press `from`, moved as the pointer has moved to `to` (all from
/// the light's handle). A press that doesn't move changes nothing, a small move a small change.
fn aim_dragged(base: egui::Vec2, from: egui::Vec2, to: egui::Vec2) -> (f64, f64) {
    aim_of_offset(base + (to - from))
}

/// Where a sun's rays centre on the screen (issue #330): where they are placed, or the centre found from the sky (`auto`,
/// once a render has found it); none while the rays are off.
fn rays_centre(map: &CanvasMap, l: &LightSettings, auto: Option<(f32, f32)>) -> Option<Pos2> {
    let e = &l.effects;
    if e.sky_rays <= 0.0 && e.opposite_rays <= 0.0 {
        return None;
    }
    if e.rays_placed >= 0.5 {
        return Some(map.screen(Point::new(e.rays_x, e.rays_y)));
    }
    auto.map(|(x, y)| map.screen(Point::new(f64::from(x), f64::from(y))))
}

/// The centre a sun's rays take from the sky on the active photo, once a render has found it.
fn auto_centre(app: &LightcraftApp, l: &LightSettings) -> Option<(f32, f32)> {
    let rays = if l.effects.sky_rays > 0.0 { lightcraft_relight::Rays::Sun } else { lightcraft_relight::Rays::Opposite };
    let made = app.session.relight_made(app.session.active()?)?;
    made.input.field.rays_centre_known(rays)
}

/// The softbox's half-sizes on the screen, and its corner, turning knob and stem on it.
fn box_frame(map: &CanvasMap, l: &LightSettings, c: Pos2) -> (egui::Vec2, [Pos2; 4], Pos2) {
    let (w, h) = photo_size(map);
    let half = vec2(0.5 * l.kind.box_width as f32 * w, 0.5 * l.kind.box_height as f32 * h);
    let rot = l.kind.box_rotation as f32;
    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(sx, sy)| c + turned(vec2(sx * half.x, sy * half.y), rot));
    let knob = c + turned(vec2(0.0, -(half.y + KNOB)), rot);
    (half, corners, knob)
}

/// The handles of the selected light's type, where they are on the screen.
fn handles(map: &CanvasMap, l: &LightSettings, c: Pos2, auto: Option<(f32, f32)>) -> Vec<(Handle, Pos2)> {
    match l.kind.index() {
        1 => {
            let (dir, len) = spot_axis(map, l, c);
            vec![
                (Handle::Aim, map.screen(Point::new(l.kind.spot_aim_x, l.kind.spot_aim_y))),
                (Handle::Cone, c + turned(dir, l.kind.spot_angle as f32) * len),
            ]
        }
        2 => {
            let mut v = vec![(Handle::SunTip, sun_tip(l, c))];
            if let Some(at) = rays_centre(map, l, auto) {
                v.push((Handle::RaysCentre, at));
            }
            v
        }
        3 => {
            let (_, corners, knob) = box_frame(map, l, c);
            vec![(Handle::BoxSize, corners[2]), (Handle::BoxTurn, knob), (Handle::BoxAim, aim_tip(l, c))]
        }
        _ => Vec::new(),
    }
}

fn halo(p: &egui::Painter, points: Vec<Pos2>, tint: Color32, closed: bool) {
    let (back, line) = (Stroke::new(3.2, Color32::from_black_alpha(110)), Stroke::new(1.4, tint));
    if closed {
        p.add(egui::Shape::closed_line(points.clone(), back));
        p.add(egui::Shape::closed_line(points, line));
    } else {
        p.add(egui::Shape::line(points.clone(), back));
        p.add(egui::Shape::line(points, line));
    }
}

fn grip(p: &egui::Painter, at: Pos2, tint: Color32, hot: bool, hollow: bool) {
    p.circle_filled(at, 5.6, Color32::from_black_alpha(140));
    if hollow {
        p.circle_stroke(at, 4.0, Stroke::new(1.6, tint));
    } else {
        p.circle_filled(at, 4.0, tint);
    }
    if hot {
        p.circle_stroke(at, 8.5, Stroke::new(1.0, Color32::from_white_alpha(160)));
    }
}

fn paint(
    p: &egui::Painter,
    map: &CanvasMap,
    l: &LightSettings,
    c: Pos2,
    hot: Option<Handle>,
    auto: Option<(f32, f32)>,
    cone: Option<&lightcraft_relight::SpotCone>,
) {
    let tint = light_tint(l.warmth);
    let on = |h: Handle| hot == Some(h);
    match l.kind.index() {
        1 => {
            let (dir, len) = spot_axis(map, l, c);
            let a = l.kind.spot_angle as f32;
            if let Some(cone) = cone {
                // (issue #346: the cone as the photo shows it, from the photo's depth: a cone coming towards the camera looks much
                // wider than its angle, and its pool lands along the line past the aim)
                let at = |q: (f64, f64)| map.screen(Point::new(q.0, q.1));
                for side in cone.sides {
                    halo(p, vec![c, at(side)], tint.gamma_multiply(0.85), false);
                }
                let mut ring: Vec<Pos2> = cone.ring.iter().map(|q| at(*q)).collect();
                if let Some(first) = ring.first().copied() {
                    ring.push(first);
                }
                halo(p, ring, tint.gamma_multiply(0.6), false);
            } else {
                let reach = len * 1.35;
                for sign in [-1.0, 1.0] {
                    halo(p, vec![c, c + turned(dir, sign * a) * reach], tint.gamma_multiply(0.85), false);
                }
                let arc: Vec<Pos2> = (0..=16).map(|i| c + turned(dir, a * (i as f32 / 8.0 - 1.0)) * len).collect();
                halo(p, arc, tint.gamma_multiply(0.6), false);
            }
            let aim = map.screen(Point::new(l.kind.spot_aim_x, l.kind.spot_aim_y));
            p.line_segment([c, aim], Stroke::new(1.0, tint.gamma_multiply(0.5)));
            grip(p, aim, tint, on(Handle::Aim), false);
            p.line_segment([aim - vec2(9.0, 0.0), aim + vec2(9.0, 0.0)], Stroke::new(1.0, tint));
            p.line_segment([aim - vec2(0.0, 9.0), aim + vec2(0.0, 9.0)], Stroke::new(1.0, tint));
            grip(p, c + turned(dir, a) * len, tint, on(Handle::Cone), false);
        }
        2 => {
            let tip = sun_tip(l, c);
            let v = tip - c;
            if v.length() > 20.0 {
                let dir = v / v.length();
                let side = vec2(-dir.y, dir.x) * 6.0;
                halo(p, vec![tip, c + dir * 8.0], tint, false);
                let head = vec![c + dir * 9.0, c + dir * 21.0 + side, c + dir * 21.0 - side];
                p.add(egui::Shape::convex_polygon(head, tint, Stroke::new(1.0, Color32::from_black_alpha(140))));
            }
            // (a hollow tip: the sun is behind the scene)
            grip(p, tip, tint, on(Handle::SunTip), l.kind.sun_azimuth.abs() > 90.0);
            // (the rays' centre: a small sun)
            if let Some(at) = rays_centre(map, l, auto) {
                for k in 0..8 {
                    let d = turned(vec2(0.0, -1.0), 45.0 * k as f32);
                    halo(p, vec![at + d * 8.0, at + d * 13.0], tint, false);
                }
                grip(p, at, tint, on(Handle::RaysCentre), true);
            }
        }
        3 => {
            let (_, corners, knob) = box_frame(map, l, c);
            halo(p, corners.to_vec(), tint, true);
            let top = corners[0].lerp(corners[1], 0.5);
            p.line_segment([top, knob], Stroke::new(1.0, tint.gamma_multiply(0.6)));
            grip(p, knob, tint, on(Handle::BoxTurn), false);
            let tip = aim_tip(l, c);
            let straight = l.kind.box_aim_h == 0.0 && l.kind.box_aim_v == 0.0;
            if !straight {
                halo(p, vec![c, tip], tint.gamma_multiply(0.85), false);
            }
            grip(p, tip, tint, on(Handle::BoxAim), straight);
            let g = corners[2];
            p.rect_filled(Rect::from_center_size(g, vec2(11.0, 11.0)), 2.0, Color32::from_black_alpha(140));
            p.rect_filled(Rect::from_center_size(g, vec2(8.0, 8.0)), 1.5, tint);
            if on(Handle::BoxSize) {
                p.rect_stroke(
                    Rect::from_center_size(g, vec2(17.0, 17.0)),
                    3.0,
                    Stroke::new(1.0, Color32::from_white_alpha(160)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        _ => {}
    }
}

/// What dragging `h` to the screen point `q` sets: the control ids and values (the selected light's).
fn values(map: &CanvasMap, l: &LightSettings, c: Pos2, h: Handle, q: Pos2, aim_grab: (egui::Vec2, egui::Vec2)) -> serde_json::Value {
    match h {
        Handle::Aim => {
            let n = map.norm(q);
            json!({"relight.spotAimX": n.x.clamp(-0.5, 1.5), "relight.spotAimY": n.y.clamp(-0.5, 1.5)})
        }
        Handle::Cone => {
            let (dir, _) = spot_axis(map, l, c);
            let v = q - c;
            let len = v.length();
            let cos = if len > 1.0 { (v.dot(dir) / len).clamp(-1.0, 1.0) } else { 1.0 };
            json!({"relight.spotAngle": f64::from(cos.acos().to_degrees()).clamp(5.0, 85.0).round()})
        }
        Handle::SunTip => {
            let mut p = (q - c) / SUN_LEN;
            if p.length() > 1.0 {
                p /= p.length();
            }
            // (the front half of the sky: a drag never puts the sun behind the scene; the Sun Direction slider does)
            let sin_el = (-p.y).clamp(0.0, 1.0);
            let cos_el = (1.0 - sin_el * sin_el).sqrt().max(1e-3);
            let az = (p.x / cos_el).clamp(-1.0, 1.0).asin().to_degrees();
            json!({"relight.sunAzimuth": f64::from(az).round(), "relight.sunElevation": f64::from(sin_el.asin().to_degrees()).round()})
        }
        Handle::RaysCentre => {
            let n = map.norm(q);
            // (a drag places the centre: it leaves the one found from the sky)
            json!({"relight.raysPlaced": 1, "relight.raysX": (n.x.clamp(-0.5, 1.5) * 1000.0).round() / 1000.0, "relight.raysY": (n.y.clamp(-1.0, 1.5) * 1000.0).round() / 1000.0})
        }
        Handle::BoxSize => {
            let (w, h) = photo_size(map);
            let local = turned(q - c, -(l.kind.box_rotation as f32));
            json!({
                "relight.boxWidth": (f64::from(2.0 * local.x.abs() / w) * 1000.0).round().clamp(20.0, 1500.0) / 1000.0,
                "relight.boxHeight": (f64::from(2.0 * local.y.abs() / h) * 1000.0).round().clamp(20.0, 1500.0) / 1000.0,
            })
        }
        Handle::BoxAim => {
            let (aim_h, aim_v) = aim_dragged(aim_grab.0, aim_grab.1, q - c);
            json!({"relight.boxAimH": aim_h, "relight.boxAimV": aim_v})
        }
        Handle::BoxTurn => {
            let v = q - c;
            json!({"relight.boxRotation": f64::from(v.x.atan2(-v.y).to_degrees()).round().clamp(-180.0, 180.0)})
        }
    }
}

fn control_of(h: Handle) -> &'static str {
    match h {
        Handle::Aim => "relight.spotAimX",
        Handle::Cone => "relight.spotAngle",
        Handle::SunTip => "relight.sunAzimuth",
        Handle::BoxSize => "relight.boxWidth",
        Handle::BoxTurn => "relight.boxRotation",
        Handle::BoxAim => "relight.boxAimH",
        Handle::RaysCentre => "relight.raysX",
    }
}

fn label_of(h: Handle) -> &'static str {
    match h {
        Handle::Aim => "Relight Aim",
        Handle::Cone => "Relight Cone",
        Handle::SunTip => "Relight Sun",
        Handle::BoxSize => "Relight Box Size",
        Handle::BoxTurn => "Relight Box Turn",
        Handle::BoxAim => "Relight Box Aim",
        Handle::RaysCentre => "Relight Rays Centre",
    }
}

/// Draw the selected light's handles for its type and drag them. True while the pointer belongs to them (a press or a
/// drag on one): the light's own handle and the loupe wait. `map` is the photo's mask-coordinate mapping, `c` the
/// light's place on the screen.
pub(super) fn overlay(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    resp: &egui::Response,
    map: &CanvasMap,
    l: &LightSettings,
    c: Pos2,
    painter: &egui::Painter,
) -> bool {
    let clip = painter.clip_rect();
    // (the web build has no Relight)
    if cfg!(target_arch = "wasm32") {
        return false;
    }
    let auto = auto_centre(app, l);
    let found = handles(map, l, c, auto);
    if found.is_empty() || !clip.contains(c) {
        // (a drag that began on a handle ends where its light's handles stop)
        return ui.ctx().data(|m| m.get_temp::<Handle>(grab_id())).is_some() && finish(app, ui, resp);
    }
    let grab = ui.ctx().data(|m| m.get_temp::<Handle>(grab_id()));
    let hover = resp.hover_pos();
    let near = |q: Pos2| {
        found
            .iter()
            .filter(|(_, at)| at.distance(q) <= HIT && at.distance(q) < c.distance(q))
            .min_by(|a, b| a.1.distance(q).total_cmp(&b.1.distance(q)))
            .map(|(h, _)| *h)
    };
    // (issue #466: nothing is drawn while the outline is hidden; the handles still take presses)
    if painter.opacity() > 0.0 {
        let cone = (l.kind.index() == 1)
            .then(|| app.session.active().and_then(|photo| app.session.relight_made(photo)))
            .flatten()
            .and_then(|made| lightcraft_pipeline::relight::spot_cone(&made.input.field, l));
        paint(painter, map, l, c, grab.or_else(|| hover.and_then(near)), auto, cone.as_ref());
    }
    for (h, at) in &found {
        register(ui, format!("relight:kind:{}", control_of(*h).trim_start_matches("relight.")), Rect::from_center_size(*at, vec2(16.0, 16.0)));
    }
    if grab.is_none() {
        if hover.and_then(near).is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        if (resp.drag_started() || resp.clicked())
            && let Some(h) = press.and_then(near)
        {
            if resp.drag_started() {
                let _ = app.run("develop.beginInteraction", json!({"label": label_of(h)}));
                ui.ctx().data_mut(|m| m.insert_temp(grab_id(), h));
                if let Some(p) = press {
                    let grab = (aim_offset(l.kind.box_aim_h, l.kind.box_aim_v), p - c);
                    ui.ctx().data_mut(|m| m.insert_temp(aim_grab_id(), grab));
                }
                app.ui.dragging_control = Some(control_of(h).into());
            }
            return true;
        }
        return false;
    }
    let Some(h) = grab else { return false };
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    let aim_grab = ui.ctx().data(|m| m.get_temp::<(egui::Vec2, egui::Vec2)>(aim_grab_id())).unwrap_or_default();
    if resp.dragged()
        && let Some(q) = resp.interact_pointer_pos()
        && let Err(e) = app.run("develop.set", json!({"values": values(map, l, c, h, q, aim_grab)}))
    {
        app.toast(ui.ctx(), e);
    }
    if resp.drag_stopped() || !resp.dragged() {
        return finish(app, ui, resp);
    }
    true
}

/// End a drag of these handles: its undo step closes and the dragged control is let go.
fn finish(app: &mut LightcraftApp, ui: &egui::Ui, _resp: &egui::Response) -> bool {
    ui.ctx().data_mut(|m| m.remove::<Handle>(grab_id()));
    app.ui.dragging_control = None;
    let _ = app.run("develop.endInteraction", json!({}));
    true
}

/// Whether the light's own size ring (its Softness) is drawn and dragged: only the point light has one.
pub(super) fn has_ring(l: &LightSettings) -> bool {
    l.kind.index() == 0
}

/// The light's handle's place on the screen; none (NaN: never near, never drawn) for a light without a place.
pub(super) fn spot_of(map: &CanvasMap, l: &LightSettings) -> Pos2 {
    if l.kind.has_place() { map.screen(Point::new(l.x, l.y)) } else { pos2(f32::NAN, f32::NAN) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressing_the_aim_arrow_of_a_straight_in_softbox_and_moving_a_little_aims_a_little() {
        // (issue #335) a softbox shining straight in draws its arrow 30 points out; a press on it and a 2 point move
        // aimed it by 16° and more, because the arrow was taken at the pointer's place and not by its moves
        let drawn = vec2(0.0, AIM_MIN);
        let base = aim_offset(0.0, 0.0);
        assert_eq!(aim_dragged(base, drawn, drawn), (0.0, 0.0), "a press that does not move");
        for step in [vec2(2.0, 0.0), vec2(0.0, 2.0), vec2(-2.0, 0.0), vec2(0.0, -2.0)] {
            let (h, v) = aim_dragged(base, drawn, drawn + step);
            assert!(h.abs() < 2.0 && v.abs() < 2.0, "2 points moved the aim to {h}, {v}");
        }
        // and the aim it gives for the arrow's true tip is the aim it came from
        assert_eq!(aim_of_offset(aim_offset(30.0, -15.0)), (30.0, -15.0));
    }
}
