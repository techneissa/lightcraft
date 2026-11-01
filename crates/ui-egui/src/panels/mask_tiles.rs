//! The Create New Mask grid's layout (fork feature, issue #26): the tiles fill the side bar's width, in 5, 4 or 2
//! columns and never with one tile alone on the last row. `masking.rs` draws the tiles with it.

/// Space between tiles.
pub const GAP: f32 = 4.0;
/// Room left and right of a tile's label.
pub const PAD: f32 = 1.5;
/// The widest a tile is drawn; a wider side bar leaves room at the right.
const TILE_MAX: f32 = 104.0;
/// The columns tried, most first. Three is left out: ten tiles make rows of 3, 3, 3 and 1.
const COLUMNS: [usize; 3] = [5, 4, 2];
/// The label sizes tried, largest first: a label shrinks a little before a column is dropped.
const SIZES: [f32; 6] = [10.5, 10.0, 9.5, 9.0, 8.5, 8.0];

/// Columns, tile width and label size of the grid of `count` tiles in `width`, `label(size)` being the widest label at
/// a font size. The largest of 5, 4 and 2 columns (one that would leave a single tile on the last row is skipped) whose
/// tiles hold the labels at a size down to 8 pt; the tiles share the width evenly, each at most 104 pt. In a panel
/// too narrow even for that, two tiles whose labels are cut short.
pub fn layout(width: f32, count: usize, label: impl Fn(f32) -> f32) -> (usize, f32, f32) {
    let room = |n: usize| ((width - (n as f32 - 1.0) * GAP) / n as f32).floor();
    for n in COLUMNS {
        if count % n == 1 {
            continue;
        }
        for size in SIZES {
            if room(n) >= (label(size) + 2.0 * PAD).ceil() {
                return (n, room(n).min(TILE_MAX), size);
            }
        }
    }
    (2, room(2).max(24.0), SIZES[SIZES.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    // "Background": 59 pt at 10.5 and, as every label, growing with the size
    fn background(size: f32) -> f32 {
        size * 5.6
    }

    #[test]
    fn no_tile_is_left_alone_on_the_last_row() {
        for width in (100..900).step_by(7) {
            let (n, ..) = layout(width as f32, 10, background);
            assert!(matches!(n, 5 | 4 | 2) && 10 % n != 1, "{width}: {n}");
        }
    }

    #[test]
    fn the_tiles_fill_the_width_and_labels_shrink_a_little_before_a_column_is_dropped() {
        // wide: five columns, the labels at the normal size, the tiles share the width
        let (n, w, size) = layout(474.0, 10, background);
        assert_eq!((n, size), (5, 10.5));
        assert!(474.0 - (n as f32 * w + (n as f32 - 1.0) * GAP) < n as f32, "{w}");
        // narrower: four columns, the labels a little smaller
        let (n, w, size) = layout(236.0, 10, background);
        assert_eq!(n, 4);
        assert!((8.5..10.5).contains(&size) && w >= (background(size) + 2.0 * PAD).ceil(), "{size} {w}");
        // too narrow for four even at the smallest size: two columns
        assert_eq!(layout(160.0, 10, background).0, 2);
        // a very wide panel: tiles stop at 104 pt
        assert_eq!(layout(1600.0, 10, background).1, TILE_MAX);
        // never a negative width
        let (n, w, _) = layout(60.0, 10, background);
        assert_eq!(n, 2);
        assert!(w >= 24.0);
        assert_eq!(layout(f32::NAN, 10, background).0, 2);
    }
}
