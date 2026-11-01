//! Filmstrip cell layout with the badge bar at one height (fork/filmstrip-badge-bar.md).

use egui::{Rect, pos2, vec2};

/// Height of the badge bar (points).
pub(super) const BAR_H: f32 = 15.0;
/// Gap between the photo and the bar.
const GAP: f32 = 2.0;

/// Where a cell draws its photo and its badge bar.
pub(super) struct CellLayout {
    /// The photo, fitted and centred in the picture area above the bar.
    pub image: Rect,
    /// The badge bar: the cell's full width, along its bottom edge.
    pub bar: Rect,
}

/// Lay out one cell. `cell` is the whole cell, `area` its picture area, `tex` the thumbnail's pixel size. With
/// badges on, the bar is the same rectangle in every cell and the photo is fitted into the space above it, so
/// photos of different shapes show their bars at one height and the bar never covers a photo.
pub(super) fn layout(cell: Rect, area: Rect, tex: [usize; 2], badges: bool) -> CellLayout {
    let bar = Rect::from_min_max(pos2(cell.left(), cell.bottom() - BAR_H), cell.right_bottom());
    let area = if badges { Rect::from_min_max(area.min, pos2(area.right(), area.bottom().min(bar.top() - GAP).max(area.top()))) } else { area };
    let [tw, th] = [tex[0].max(1) as f32, tex[1].max(1) as f32];
    let s = (area.width() / tw).min(area.height() / th);
    CellLayout { image: Rect::from_center_size(area.center(), vec2(tw * s, th * s)), bar }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_share_one_height_and_clear_the_photo() {
        let cell = Rect::from_min_size(pos2(240.0, 4.0), vec2(120.0, 96.0));
        let area = Rect::from_min_max(cell.min + vec2(10.0, 22.0), cell.max - vec2(10.0, 8.0));
        let mut bars = vec![];
        for tex in [[1600, 900], [1000, 1000], [800, 1000]] {
            let l = layout(cell, area, tex, true);
            assert!(l.image.bottom() <= l.bar.top(), "photo {tex:?} overlaps the bar");
            assert!(l.image.top() >= area.top() - 0.01 && l.image.left() >= area.left() - 0.01 && l.image.right() <= area.right() + 0.01);
            assert!((l.image.center().x - area.center().x).abs() < 0.01);
            assert_eq!((l.bar.left(), l.bar.right(), l.bar.bottom()), (cell.left(), cell.right(), cell.bottom()));
            bars.push(l.bar);
        }
        assert!(bars.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn badges_off_keep_the_photo_area() {
        let cell = Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 96.0));
        let area = Rect::from_min_max(cell.min + vec2(10.0, 22.0), cell.max - vec2(10.0, 8.0));
        assert_eq!(layout(cell, area, [1000, 1000], false).image.center(), area.center());
    }
}
