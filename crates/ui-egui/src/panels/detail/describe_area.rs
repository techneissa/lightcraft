//! Issue #281: zoomed in, a Describe mask looks only in the part of the photo the loupe shows. The loupe notes that
//! part each frame the Masking panel is open ([`note`]); the Describe field sends it as the mask's `region`
//! ([`region_for`]); a selected mask's region is outlined on the photo ([`outline`]).

use lightcraft_catalog::PhotoId;

use super::geometry::CanvasMap;
use super::{Color32, LightcraftApp, Point, Pos2, Rect, Stroke};

/// The part of the photo the loupe shows when zoomed in, normalized, on the photo as shot (`Mask::on_photo`): where
/// the model looks, whatever mask the selection joins (an old mask is moved onto the photo before the model runs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DescribeArea {
    pub photo: PhotoId,
    pub region: [f64; 4],
}

/// Points along each edge of the view the area is found from: a lens warp bends its edges.
const EDGE_SAMPLES: usize = 16;

/// Note what the loupe shows of photo `photo` (`map`: the photo on screen): an area when the photo is cut by the
/// canvas (zoomed in), else none (Fit, or zoomed out: the whole photo).
pub(super) fn note(app: &mut LightcraftApp, photo: PhotoId, map: &CanvasMap) {
    let canvas = app.canvas_rect.unwrap_or(map.rect);
    let shown = map.rect.intersect(canvas);
    let zoomed = !canvas.expand(1.0).contains_rect(map.rect) && shown.is_positive();
    app.describe_area = zoomed.then(|| area(&map.for_mask(true), shown)).flatten().map(|region| DescribeArea { photo, region });
}

/// The box around screen rectangle `r` in `map`'s coordinates, cleaned as a Describe region (`None`: the whole
/// photo).
fn area(map: &CanvasMap, r: Rect) -> Option<[f64; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in 0..=EDGE_SAMPLES {
        let t = i as f32 / EDGE_SAMPLES as f32;
        let (x, y) = (r.left() + t * r.width(), r.top() + t * r.height());
        for q in [(x, r.top()), (x, r.bottom()), (r.left(), y), (r.right(), y)] {
            let n = map.norm(Pos2::new(q.0, q.1));
            if n.x.is_finite() && n.y.is_finite() {
                (x0, y0, x1, y1) = (x0.min(n.x), y0.min(n.y), x1.max(n.x), y1.max(n.y));
            }
        }
    }
    (x0 <= x1 && y0 <= y1).then_some([x0, y0, x1, y1]).and_then(lightcraft_develop::prompt_region)
}

/// The region a Describe started now looks in; `None` for the whole photo.
pub(crate) fn region_for(app: &LightcraftApp) -> Option<[f64; 4]> {
    let id = app.session.active()?;
    app.describe_area.filter(|a| a.photo == id).map(|a| a.region)
}

/// Outline region `r` (normalized, `map`'s coordinates) on the photo: a thin dashed box, drawn through any warp.
pub(super) fn outline(p: &egui::Painter, map: &CanvasMap, r: [f64; 4]) {
    let corners = [Point::new(r[0], r[1]), Point::new(r[2], r[1]), Point::new(r[2], r[3]), Point::new(r[0], r[3])];
    let mut pts: Vec<Pos2> = Vec::with_capacity(4 * EDGE_SAMPLES + 1);
    for (i, a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % 4];
        for k in 0..EDGE_SAMPLES {
            let t = k as f64 / EDGE_SAMPLES as f64;
            pts.push(map.screen(Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y))));
        }
    }
    if let Some(first) = pts.first().copied() {
        pts.push(first);
    }
    let dark = Stroke::new(2.5, Color32::from_black_alpha(110));
    let light = Stroke::new(1.0, Color32::from_white_alpha(220));
    p.add(egui::Shape::line(pts.clone(), dark));
    p.extend(egui::Shape::dashed_line(&pts, light, 6.0, 4.0));
}
