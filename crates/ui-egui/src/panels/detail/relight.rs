//! Fork (issues #288, #299, `fork/relight.md`): Relight's lights on the photo. While the Relight section is open and
//! the lights change the photo, each light's handle is drawn where the light is (on the photo before the lens
//! corrections, as a mask's coordinates are), the selected one marked with a ring that sets its size. A press on a
//! handle selects that light and a drag from it moves it; a drag of the ring changes its Softness; a scroll over a
//! handle moves its light nearer or farther (its depth). Every other press goes on to the loupe as before. One drag,
//! and one run of scrolling, is one undo step.

use egui::{Color32, Pos2, Stroke};
use serde_json::json;

use super::geometry::CanvasMap;
use super::{DevelopSettings, LightcraftApp, Rect, register, vec2};
use crate::panels::edit::{light_tint, nearest};

mod kinds;

/// How near a press must be to a handle to take it (points).
const HIT: f32 = 16.0;
/// The size ring's radius at Softness 0, and what each point of Softness adds (points).
const RING_MIN: f32 = 24.0;
const RING_PER_SOFTNESS: f32 = 0.4;
/// How near a press must be to the ring's line to take it (points).
const RING_HIT: f32 = 7.0;
/// How long after the last scroll the run of scrolling ends (seconds): one undo step.
const SCROLL_IDLE: f64 = 0.4;
/// Depth (of 100) a scroll of one point moves the light.
const SCROLL_DEPTH: f64 = 0.12;

/// What a drag of the handles is doing (kept in the context while it goes).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grab {
    Move,
    Size,
}

fn grab_id() -> egui::Id {
    egui::Id::unique("relightOnPhotoDrag")
}

fn scroll_id() -> egui::Id {
    egui::Id::unique("relightOnPhotoScroll")
}

/// Whether the lights' handles are on the photo now: the Edit panel's Relight section is open, the lights change the
/// photo (an Amount or Darken Surroundings above 0, the section's eye on) and no tool has the photo.
pub(super) fn shown(app: &LightcraftApp, d: &DevelopSettings) -> bool {
    app.ui.section_open(lightcraft_develop::relight::SECTION) && lightcraft_pipeline::relight::scene_of(d).is_some() && app.ui.tool.is_empty()
}

/// A drag of a handle (or a run of scrolling) that is going when the handles stop showing (the section closed, the
/// lights turned off, another panel opened) ends there: its undo step closes and the dragged control is let go (⌥'s
/// preview would stay).
pub(super) fn release_if_hidden(app: &mut LightcraftApp, ui: &egui::Ui, d: &DevelopSettings) {
    let showing = app.ui.right == crate::state::RightPanel::Edit && shown(app, d);
    kinds::release(app, ui, showing);
    let dragging = ui.ctx().data(|m| m.get_temp::<Grab>(grab_id())).is_some();
    if dragging && !showing {
        ui.ctx().data_mut(|m| m.remove::<Grab>(grab_id()));
        app.ui.dragging_control = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
    // a run of scrolling ends when it has been quiet for a moment, or the handles stop showing
    if let Some(until) = ui.ctx().data(|m| m.get_temp::<f64>(scroll_id())) {
        let now = ui.input(|i| i.time);
        if !showing || now >= until {
            ui.ctx().data_mut(|m| m.remove::<f64>(scroll_id()));
            app.ui.dragging_control = None;
            let _ = app.run("develop.endInteraction", json!({}));
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(until - now + 0.02));
        }
    }
}

/// The handles' places on screen, in the lights' order.
fn spots(map: &CanvasMap, d: &DevelopSettings) -> Vec<Pos2> {
    d.relight.lights.iter().map(|l| kinds::spot_of(map, l)).collect()
}

/// The size ring's radius (points) for Softness `softness` (0..100).
fn ring_radius(softness: f64) -> f32 {
    RING_MIN + RING_PER_SOFTNESS * softness.clamp(0.0, 100.0) as f32
}

/// A scroll over a handle moves its light nearer (up) or farther (down), instead of zooming or panning the view. Called
/// before the view takes the scroll; the handle under the pointer is selected first. A run of scrolling is one undo step
/// (it ends after a quiet moment, in [`release_if_hidden`]).
pub(super) fn claim_scroll(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    if !resp.hovered() || !shown(app, d) || app.ui.right != crate::state::RightPanel::Edit {
        return;
    }
    let Some(q) = ui.input(|i| i.pointer.hover_pos()) else { return };
    let dy = ui.input(|i| if i.modifiers.is_none() { i.smooth_scroll_delta.y } else { 0.0 });
    if dy == 0.0 || !dy.is_finite() {
        return;
    }
    let map = map.for_mask(true);
    let Some(i) = nearest(&spots(&map, d), q, HIT) else { return };
    // (the view doesn't take this scroll)
    ui.input_mut(|inp| {
        inp.smooth_scroll_delta = egui::Vec2::ZERO;
        inp.events.retain(|e| !matches!(e, egui::Event::MouseWheel { modifiers, .. } if modifiers.is_none()));
    });
    let Some(photo) = app.session.active() else { return };
    let mut depth = d.relight.lights.get(i).map_or(0.0, |l| l.depth);
    if i != app.session.relight_selected(photo) {
        let _ = app.run("relight.selectLight", json!({"index": i}));
    }
    if ui.ctx().data(|m| m.get_temp::<f64>(scroll_id())).is_none() {
        let _ = app.run("develop.beginInteraction", json!({"label": "Relight Depth"}));
        app.ui.dragging_control = Some("relight.depth".into());
    }
    depth = (depth - f64::from(dy) * SCROLL_DEPTH).clamp(0.0, 100.0);
    if let Err(e) = app.run("develop.set", json!({"control": "relight.depth", "value": depth.round()})) {
        app.toast(ui.ctx(), e);
    }
    let until = ui.input(|i| i.time) + SCROLL_IDLE;
    ui.ctx().data_mut(|m| m.insert_temp(scroll_id(), until));
    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(SCROLL_IDLE + 0.02));
}

/// The size ring of the selected light: a thin line with a grip, drawn dark under light so it shows on any photo.
fn paint_ring(p: &egui::Painter, at: Pos2, radius: f32, warmth: f64, hot: bool) {
    let tint = light_tint(warmth);
    let (halo, line) = (
        Stroke::new(3.2, Color32::from_black_alpha(110)),
        Stroke::new(if hot { 2.0 } else { 1.4 }, tint.gamma_multiply(if hot { 1.0 } else { 0.8 })),
    );
    p.circle_stroke(at, radius, halo);
    p.circle_stroke(at, radius, line);
    let grip = at + vec2(radius, 0.0);
    p.circle_filled(grip, 4.6, Color32::from_black_alpha(140));
    p.circle_filled(grip, 3.4, tint);
}

/// Draw the lights' handles and move them when a drag starts on one. True while the pointer belongs to them (a press,
/// a click or a drag on a handle or the ring): the loupe's own interactions then wait.
pub(super) fn overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) -> bool {
    let map = map.for_mask(true);
    let Some(photo) = app.session.active() else { return false };
    let sel = app.session.relight_selected(photo);
    let at = spots(&map, d);
    let grab = ui.ctx().data(|m| m.get_temp::<Grab>(grab_id()));
    let Some(chosen) = d.relight.lights.get(sel) else { return false };
    let ring = ring_radius(chosen.softness);
    let ring_on = kinds::has_ring(chosen);
    let on_ring = |q: Pos2| ring_on && at.get(sel).is_some_and(|c| (q.distance(*c) - ring).abs() <= RING_HIT && q.distance(*c) > HIT);
    let hover = resp.hover_pos();
    let clip = app.canvas_rect.unwrap_or(map.rect);
    {
        let p = ui.painter_at(clip);
        for (i, (l, c)) in d.relight.lights.iter().zip(&at).enumerate() {
            if i == sel || !clip.contains(*c) {
                continue;
            }
            crate::panels::edit::paint_light(&p, *c, 5.5, l.warmth, hover.is_some_and(|q| nearest(&at, q, HIT) == Some(i)));
            register(ui, format!("relight:onPhoto:{i}"), Rect::from_center_size(*c, vec2(20.0, 20.0)));
        }
        if let Some(c) = at.get(sel) {
            if clip.contains(*c) {
                if ring_on {
                    paint_ring(&p, *c, ring, chosen.warmth, grab == Some(Grab::Size) || hover.is_some_and(on_ring));
                }
                crate::panels::edit::paint_light(
                    &p,
                    *c,
                    7.0,
                    chosen.warmth,
                    grab == Some(Grab::Move) || hover.is_some_and(|q| nearest(&at, q, HIT) == Some(sel)),
                );
                register(ui, "relight:onPhoto", Rect::from_center_size(*c, vec2(20.0, 20.0)));
                register(ui, format!("relight:onPhoto:{sel}"), Rect::from_center_size(*c, vec2(20.0, 20.0)));
            }
            if ring_on {
                register(ui, "relight:ring", Rect::from_center_size(*c + vec2(ring, 0.0), vec2(14.0, 14.0)));
            }
        }
    }
    // the handles of the light's type (a spot's aim and cone, a sun's arrow, a softbox's corner and knob)
    if let Some(c) = at.get(sel)
        && kinds::overlay(app, ui, resp, &map, chosen, *c, clip)
    {
        return true;
    }
    if grab.is_none()
        && let Some(q) = hover
    {
        if nearest(&at, q, HIT).is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if on_ring(q) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNeSw);
        }
    }
    // a press: a handle (selected, then moved), else the selected light's ring (its size)
    let press = || ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if (resp.drag_started() || resp.clicked())
        && let Some(q) = press()
    {
        let handle = nearest(&at, q, HIT);
        if handle.is_none() && !on_ring(q) {
            return false;
        }
        if let Some(i) = handle.filter(|i| *i != sel)
            && let Err(e) = app.run("relight.selectLight", json!({"index": i}))
        {
            app.toast(ui.ctx(), e);
            return true;
        }
        if resp.drag_started() {
            let (label, control, g) =
                if handle.is_some() { ("Relight Light", "relight.x", Grab::Move) } else { ("Relight Size", "relight.softness", Grab::Size) };
            let _ = app.run("develop.beginInteraction", json!({"label": label}));
            ui.ctx().data_mut(|m| m.insert_temp(grab_id(), g));
            // (⌥ while it is held shows where the light reaches: `relight_alt`)
            app.ui.dragging_control = Some(control.into());
        }
        return true;
    }
    let Some(grab) = grab else { return false };
    ui.ctx().set_cursor_icon(if grab == Grab::Move { egui::CursorIcon::Grabbing } else { egui::CursorIcon::ResizeNeSw });
    if resp.dragged()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let values = match grab {
            Grab::Move => {
                let n = map.norm(q);
                json!({"relight.x": n.x.clamp(-0.5, 1.5), "relight.y": n.y.clamp(-0.5, 1.5)})
            }
            Grab::Size => {
                let dist = at.get(sel).map_or(0.0, |c| q.distance(*c));
                json!({"relight.softness": (f64::from((dist - RING_MIN) / RING_PER_SOFTNESS)).clamp(0.0, 100.0).round()})
            }
        };
        if let Err(e) = app.run("develop.set", json!({"values": values})) {
            app.toast(ui.ctx(), e);
        }
    }
    if resp.drag_stopped() || !resp.dragged() {
        ui.ctx().data_mut(|m| m.remove::<Grab>(grab_id()));
        app.ui.dragging_control = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
    true
}
