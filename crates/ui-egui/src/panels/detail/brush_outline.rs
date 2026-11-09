//! Fork feature (`fork/brush-area.md`): a brushed Remove, Heal or Clone spot on the photo is drawn as ONE outline,
//! the contour of the area it covers (the union of its discs), not a circle for every point of the stroke; while a
//! stroke is painted, it is one translucent filled shape. A shaped spot (`object_remove`, a lasso) keeps its own
//! outline.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke};
use lightcraft_develop::Spot;
use lightcraft_geom::Point;
use lightcraft_pipeline::spot_lasso;

use super::geometry::CanvasMap;

/// Most separate pieces of one outline drawn.
const MAX_PIECES: usize = 8;
/// Grid side the stroke being painted is measured on (every frame, so coarser than a spot's outline).
const PAINT_SIDE: usize = 300;

/// The photo's size ratio on screen as `(fx, fy)`: normalized x and y times these are long-edge units (the longer is 1).
/// `None` when the map is degenerate.
fn scale(map: &CanvasMap) -> Option<(f64, f64)> {
    let o = map.screen(Point::new(0.0, 0.0));
    let (sx, sy) = (f64::from(map.screen(Point::new(1.0, 0.0)).distance(o)), f64::from(map.screen(Point::new(0.0, 1.0)).distance(o)));
    let m = sx.max(sy);
    (sx.is_finite() && sy.is_finite() && sx > 0.0 && sy > 0.0).then_some((sx / m, sy / m))
}

fn to_long(points: &[Point], (fx, fy): (f64, f64)) -> Vec<Point> {
    points.iter().map(|p| Point::new(p.x * fx, p.y * fy)).collect()
}

/// The outline of a stroke spot (rings in spot coordinates), worked out once per stroke and kept in egui's memory
/// (`slot`: the spot's place in the list, so a changed spot replaces its entry).
fn rings(ctx: &egui::Context, map: &CanvasMap, slot: usize, spot: &Spot) -> Arc<Vec<Vec<Point>>> {
    let Some(k) = scale(map) else { return Arc::default() };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spot.size.to_bits().hash(&mut h);
    (k.0.to_bits(), k.1.to_bits()).hash(&mut h);
    for p in &spot.points {
        (p.x.to_bits(), p.y.to_bits()).hash(&mut h);
    }
    let (id, stamp) = (egui::Id::unique(("lc-brush-outline", slot)), h.finish());
    if let Some((s, r)) = ctx.data(|d| d.get_temp::<(u64, Arc<Vec<Vec<Point>>>)>(id))
        && s == stamp
    {
        return r;
    }
    let rings: Vec<Vec<Point>> = spot_lasso::outline(&to_long(&spot.points, k), spot.size, false, MAX_PIECES)
        .into_iter()
        .map(|ring| ring.into_iter().map(|p| Point::new(p.x / k.0, p.y / k.1)).collect())
        .collect();
    let out = Arc::new(rings);
    ctx.data_mut(|d| d.insert_temp(id, (stamp, out.clone())));
    out
}

fn path(map: &CanvasMap, ring: &[Point], offset: Point) -> Vec<Pos2> {
    let mut pts: Vec<Pos2> = ring.iter().map(|q| map.screen(Point::new(q.x + offset.x, q.y + offset.y))).collect();
    if let Some(first) = pts.first().copied() {
        pts.push(first);
    }
    pts
}

/// Draw the outline of stroke spot number `slot`, moved by `offset` (the source copy), in `col`. False for a shaped
/// spot (its own outline is drawn by `object_remove`).
pub(super) fn outline(ctx: &egui::Context, p: &Painter, map: &CanvasMap, slot: usize, spot: &Spot, offset: Point, stroke: Stroke) -> bool {
    if spot.area.is_some() {
        return false;
    }
    for ring in rings(ctx, map, slot, spot).iter() {
        p.add(Shape::line(path(map, ring, offset), stroke));
    }
    true
}

/// The stroke being painted: its area as one translucent filled mesh and one outline. A stroke that has come back
/// near its start fills its inside, as it will once it is let go.
pub(super) fn painting(p: &Painter, map: &CanvasMap, points: &[Point], radius: f64, fill: Color32, outline: Stroke) {
    let Some(k) = scale(map) else { return };
    let pts = to_long(points, k);
    let Some(c) = spot_lasso::cover(&pts, radius, spot_lasso::is_loop(&pts, radius), PAINT_SIDE) else { return };
    let screen = |x: f64, y: f64| {
        let l = c.at(x, y);
        map.screen(Point::new(l.x / k.0, l.y / k.1))
    };
    // one mesh: every row's runs of covered cells as rectangles (no cell is drawn twice, so the translucent fill never doubles up)
    let mut mesh = Mesh::default();
    for y in 0..c.mask.h {
        let mut x = 0;
        while x < c.mask.w {
            if !c.mask.get(x as i64, y as i64) {
                x += 1;
                continue;
            }
            let start = x;
            while x < c.mask.w && c.mask.get(x as i64, y as i64) {
                x += 1;
            }
            let (a, b) = (screen(start as f64, y as f64), screen(x as f64, y as f64 + 1.0));
            mesh.add_colored_rect(Rect::from_two_pos(a, b), fill);
        }
    }
    p.add(Shape::mesh(mesh));
    for ring in spot_lasso::rings(&c, MAX_PIECES) {
        let mut pts: Vec<Pos2> = ring.iter().map(|q| map.screen(Point::new(q.x / k.0, q.y / k.1))).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        p.add(Shape::line(pts, outline));
    }
}
