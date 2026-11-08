//! Fork feature (`fork/object-remove.md`): the Remove tool on the photo for spots in an object's shape. A shaped spot
//! is drawn as its outline (not as circles); in Object mode a click selects the object under it (⇧ adds to the
//! selected object spot, ⌥ leaves a part out) and a loose stroke snaps to the object it covers. The panel side is
//! `panels/right/remove_object.rs`.

use std::sync::Arc;

use egui::{Color32, Painter, Stroke};
use lightcraft_develop::Spot;
use lightcraft_geom::Point;
use lightcraft_pipeline::find::shape::{Mask, components, trace_outline};
use serde_json::json;

use super::LightcraftApp;
use super::geometry::CanvasMap;

/// Most separate pieces of one outline drawn.
const MAX_PIECES: usize = 8;

/// The outline of a shaped spot (rings in spot coordinates), worked out once per shape and kept in egui's memory.
fn rings(ctx: &egui::Context, spot: &Spot) -> Arc<Vec<Vec<Point>>> {
    let Some(area) = &spot.area else { return Arc::default() };
    let key = egui::Id::unique(("lc-spot-outline", &area.seg.data, area.seg.side, area.seg.rect.map(|r| r.map(f64::to_bits))));
    if let Some(r) = ctx.data(|d| d.get_temp::<Arc<Vec<Vec<Point>>>>(key)) {
        return r;
    }
    let out = Arc::new(trace(area));
    ctx.data_mut(|d| d.insert_temp(key, out.clone()));
    out
}

fn trace(area: &lightcraft_develop::SpotArea) -> Vec<Vec<Point>> {
    let (Some(r), Some(l)) = (area.bounds(), area.seg.logits()) else { return Vec::new() };
    let side = area.seg.side as usize;
    let m = Mask { w: side, h: side, bits: l.iter().map(|v| *v > 0.0).collect() };
    let to_norm = |p: &(f64, f64)| Point::new(r[0] + p.0 / side as f64 * (r[2] - r[0]), r[1] + p.1 / side as f64 * (r[3] - r[1]));
    components(&m, MAX_PIECES).iter().map(|c| trace_outline(&c.mask).iter().map(to_norm).collect()).collect()
}

/// Draw `spot`'s outline when it is a shaped spot; false for a stroke spot (its circles are drawn as before).
pub(super) fn outline(ctx: &egui::Context, p: &Painter, map: &CanvasMap, spot: &Spot, sel: bool) -> bool {
    if spot.area.is_none() {
        return false;
    }
    let col = Color32::from_white_alpha(if sel { 230 } else { 150 });
    for ring in rings(ctx, spot).iter() {
        let mut pts: Vec<egui::Pos2> = ring.iter().map(|q| map.screen(*q)).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        p.add(egui::Shape::line(pts, Stroke::new(if sel { 1.5 } else { 1.0 }, col)));
    }
    true
}

/// In Object mode, the end of a click or stroke on the photo (`points`, spot coordinates): the object under it is
/// selected and removed. False when not in Object mode (the brush makes a spot as before).
pub(super) fn commit(app: &mut LightcraftApp, ctx: &egui::Context, points: &[Point], click: bool) -> bool {
    if !crate::panels::right::remove_object::object_mode(app) {
        return false;
    }
    let Some(&at) = points.first() else { return true };
    let (add, subtract) = ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
    let (cmd, params) = if click || points.len() == 1 {
        ("spot.object", json!({"x": at.x, "y": at.y, "add": add, "subtract": subtract}))
    } else {
        let pts: Vec<[f64; 2]> = points.iter().map(|q| [q.x, q.y]).collect();
        let brush = json!({"points": pts, "size": app.ui.remove_size, "feather": app.ui.remove_feather, "opacity": app.ui.remove_opacity});
        ("spot.objectStroke", brush)
    };
    let params = crate::panels::right::remove_object::object_params(app, params);
    if let Err(e) = app.run_typed(cmd, params) {
        app.ai_error(ctx, e, None);
    }
    ctx.request_repaint();
    true
}
