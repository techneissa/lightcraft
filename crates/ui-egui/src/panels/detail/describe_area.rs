//! Issue #281: zoomed in, a Describe mask looks only in the part of the photo the loupe shows. The loupe notes that
//! part each frame the Masking panel is open ([`note`]); the Describe field sends it as the mask's `region`
//! ([`region_for`]). The region is not outlined on the photo (techneissa: the box always showing was in the way).

use lightcraft_catalog::PhotoId;

use super::geometry::CanvasMap;
use super::{LightcraftApp, Pos2, Rect};

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
