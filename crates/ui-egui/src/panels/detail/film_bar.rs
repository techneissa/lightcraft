//! Filmstrip cell layout with the badge bar at one height (fork/filmstrip-badge-bar.md).

use egui::{Rect, pos2, vec2};

/// Height of the badge bar (points).
pub(super) const BAR_H: f32 = 15.0;
/// Corner radius of the badge bar (points).
pub(super) const BAR_ROUNDING: f32 = 3.0;
/// Gap between the photo and the bar.
const GAP: f32 = 2.0;

/// Where a cell draws its photo and its badge bar.
pub(super) struct CellLayout {
    /// The photo, fitted and centred in the picture area above the bar.
    pub image: Rect,
    /// The badge bar: along the cell's bottom edge, inset from the cell's left and right edges by the picture
    /// area's own margin, so the bars of neighbouring cells are as far apart as their photos (`BAR_ROUNDING`).
    pub bar: Rect,
}

/// Lay out one cell. `cell` is the whole cell, `area` its picture area, `tex` the thumbnail's pixel size. With
/// badges on, the bar is the same rectangle in every cell and the photo is fitted into the space above it, so
/// photos of different shapes show their bars at one height and the bar never covers a photo.
pub(super) fn layout(cell: Rect, area: Rect, tex: [usize; 2], badges: bool) -> CellLayout {
    let bar = Rect::from_min_max(pos2(area.left().max(cell.left()), cell.bottom() - BAR_H), pos2(area.right().min(cell.right()), cell.bottom()));
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
            assert_eq!((l.bar.left(), l.bar.right(), l.bar.bottom()), (area.left(), area.right(), cell.bottom()));
            bars.push(l.bar);
        }
        assert!(bars.windows(2).all(|w| w[0] == w[1]));
    }

    /// Neighbouring cells' bars never touch: their gap is the picture area's margin on both sides.
    #[test]
    fn neighbouring_bars_keep_a_gap() {
        let area_of = |cell: Rect| Rect::from_min_max(cell.min + vec2(10.0, 22.0), cell.max - vec2(10.0, 8.0));
        let (a, b) = (Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 96.0)), Rect::from_min_size(pos2(120.0, 0.0), vec2(120.0, 96.0)));
        let (la, lb) = (layout(a, area_of(a), [1600, 900], true), layout(b, area_of(b), [1600, 900], true));
        assert_eq!(lb.bar.left() - la.bar.right(), 20.0);
        assert!(la.bar.left() > a.left() && lb.bar.right() < b.right());
        // the bar still sits along the cell's bottom edge and spans the width of a landscape photo
        assert_eq!((la.bar.bottom(), la.bar.width()), (a.bottom(), la.image.width()));
    }

    #[test]
    fn badges_off_keep_the_photo_area() {
        let cell = Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 96.0));
        let area = Rect::from_min_max(cell.min + vec2(10.0, 22.0), cell.max - vec2(10.0, 8.0));
        assert_eq!(layout(cell, area, [1000, 1000], false).image.center(), area.center());
    }
}
