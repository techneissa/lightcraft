//! The names of a mask's parts in their rows (fork feature, issue #37): the list shares one font size, and a name too
//! long even for the smallest one ends with "…". (A mask's own name wraps in its row: `mask_list`, fork issue #401.)

use egui::{Color32, FontId, Painter};

/// The smallest font a list of submask names shares before one is cut.
pub const LIST_FLOOR: f32 = 10.5;

/// One font size for every row of a list: the largest from `font` down to `floor` (steps of half a point) at which every
/// `(text, room)` fits its room, else `floor`. Rows share it, so a long name does not make one row smaller than the rest.
pub fn common_size(painter: &Painter, rows: &[(&str, f32)], font: &FontId, floor: f32) -> f32 {
    let floor = floor.min(font.size);
    let mut size = font.size;
    let too_wide = |size: f32| {
        rows.iter().any(|(text, room)| painter.layout_no_wrap((*text).to_string(), FontId { size, ..font.clone() }, Color32::WHITE).size().x > *room)
    };
    while size > floor && too_wide(size) {
        size = (size - 0.5).max(floor);
    }
    size
}
