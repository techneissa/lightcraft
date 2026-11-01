//! Find People / Find Wires on the photo: every candidate's outline, the selected ones bold; a click picks one.

use super::geometry::CanvasMap;
use super::{Color32, LightcraftApp, Pos2, Rect, Stroke, json, register, vec2};
use lightcraft_pipeline::find::Kind;

/// Draw the candidates of the active photo and handle a click on one. `true` when the click was taken (so the
/// Remove tool does not also start a spot there).
pub(super) fn show(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, map: &CanvasMap) -> bool {
    let view = app.session.finds.view(app.session.active());
    if view.entries.is_empty() {
        return false;
    }
    let p = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    let long = map.rect.width().max(map.rect.height());
    let hover = resp.hover_pos().map(|q| map.norm(q));
    for e in view.entries.iter() {
        let c = &e.candidate;
        let base = if c.kind == Kind::Person { Color32::from_rgb(90, 200, 255) } else { Color32::from_rgb(255, 196, 64) };
        let hot = hover.is_some_and(|h| c.hit(h.x, h.y, 6.0 / f64::from(long.max(1.0))));
        let pts: Vec<Pos2> = c.outline.iter().map(|q| map.screen(lightcraft_geom::Point::new(q.0, q.1))).collect();
        if pts.len() < 2 {
            continue;
        }
        let mut line = pts.clone();
        if c.closed {
            line.push(pts[0]);
        }
        if e.selected {
            p.add(egui::Shape::line(line.clone(), Stroke::new(8.0, base.gamma_multiply(0.35))));
            p.add(egui::Shape::line(line, Stroke::new(3.0, Color32::WHITE)));
        } else {
            p.add(egui::Shape::line(line.clone(), Stroke::new(4.0, Color32::from_black_alpha(110))));
            p.add(egui::Shape::line(line, Stroke::new(if hot { 2.5 } else { 1.5 }, base)));
        }
        let (a, b) = (map.screen(lightcraft_geom::Point::new(c.bbox[0], c.bbox[1])), map.screen(lightcraft_geom::Point::new(c.bbox[2], c.bbox[3])));
        register(ui, format!("findCandidate:{}", e.id), Rect::from_two_pos(a, b).expand2(vec2(2.0, 2.0)));
    }
    // a click on one: select it (⌘ or ⇧ keeps the others selected)
    if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let n = map.norm(q);
        let tol = 8.0 / f64::from(long.max(1.0));
        let mut hits: Vec<_> = view.entries.iter().filter(|e| e.candidate.hit(n.x, n.y, tol)).collect();
        hits.sort_by(|a, b| {
            (a.candidate.kind == Kind::Person, a.candidate.area)
                .partial_cmp(&(b.candidate.kind == Kind::Person, b.candidate.area))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(e) = hits.first() {
            let toggle = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            let _ = app.run("spot.selectCandidate", json!({"id": e.id, "toggle": toggle}));
            return true;
        }
    }
    false
}
