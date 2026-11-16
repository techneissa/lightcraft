//! Fork (issue #288, `fork/relight.md`): Relight's light on the photo. While the Relight section is open and the
//! light changes the photo, its handle is drawn where the light is (on the photo before the lens corrections, as a mask's
//! coordinates are) and a drag that starts on it moves the light; every other press goes on to the loupe as
//! before. One drag is one undo step.

use serde_json::json;

use super::geometry::CanvasMap;
use super::{DevelopSettings, LightcraftApp, Point, Rect, register, vec2};

/// Whether the light's handle is on the photo now: the Edit panel's Relight section is open, the light changes the
/// photo (Amount or Darken Surroundings above 0, the section's eye on) and no tool has the photo.
pub(super) fn shown(app: &LightcraftApp, d: &DevelopSettings) -> bool {
    app.ui.section_open(lightcraft_develop::relight::SECTION) && lightcraft_pipeline::relight::light_of(d).is_some() && app.ui.tool.is_empty()
}

/// Draw the light's handle and move it when a drag starts on it. True while the pointer belongs to it (a drag of
/// it): the loupe's own interactions then wait.
pub(super) fn overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) -> bool {
    let map = map.for_mask(true);
    let r = &d.relight;
    let at = map.screen(Point::new(r.x, r.y));
    let clip = app.canvas_rect.unwrap_or(map.rect);
    let dragging_id = egui::Id::unique("relightOnPhotoDrag");
    let dragging = ui.ctx().data(|m| m.get_temp::<bool>(dragging_id)).unwrap_or(false);
    let near = |q: egui::Pos2| q.distance(at) < 16.0;
    if clip.contains(at) {
        let p = ui.painter_at(clip);
        crate::panels::edit::paint_light(&p, at, 7.0, r.warmth, dragging || resp.hover_pos().is_some_and(near));
        register(ui, "relight:onPhoto", Rect::from_center_size(at, vec2(20.0, 20.0)));
    }
    if resp.hover_pos().is_some_and(near) && !dragging {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        && near(q)
    {
        let _ = app.run("develop.beginInteraction", json!({"label": "Relight Light"}));
        ui.ctx().data_mut(|m| m.insert_temp(dragging_id, true));
        return true;
    }
    if !dragging {
        return false;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    if resp.dragged()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let n = map.norm(q);
        if let Err(e) = app.run("develop.set", json!({"values": {"relight.x": n.x.clamp(-0.5, 1.5), "relight.y": n.y.clamp(-0.5, 1.5)}})) {
            app.toast(ui.ctx(), e);
        }
    }
    if resp.drag_stopped() || !resp.dragged() {
        ui.ctx().data_mut(|m| m.remove::<bool>(dragging_id));
        let _ = app.run("develop.endInteraction", json!({}));
    }
    true
}
