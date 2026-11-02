//! A mask's or a part's name in its row (fork feature, issue #37): a name too long for the row first takes a slightly
//! smaller font (as the slider labels do), down to [`SMALLEST`], and only then ends with "…" (the row shows the full
//! name on hover).

use std::sync::Arc;

use egui::{Color32, FontId, Galley, Painter};

/// The smallest font a long name shrinks to before it is cut.
pub const SMALLEST: f32 = 10.5;

/// `text` on one line within `max`: at `font` when it fits, else at the largest size down to [`SMALLEST`] (steps of
/// half a point) that fits, else at that size with an ellipsis. `Galley::elided` says it was cut.
pub fn fit(painter: &Painter, text: &str, font: FontId, color: Color32, max: f32) -> Arc<Galley> {
    let max = max.max(1.0);
    let floor = SMALLEST.min(font.size);
    let mut size = font.size;
    loop {
        let g = crate::widgets::one_line(painter, text, FontId { size, ..font.clone() }, color, max);
        if !g.elided || size <= floor {
            return g;
        }
        size = (size - 0.5).max(floor);
    }
}

/// The smallest font a list of submask names shares before one is cut.
pub const LIST_FLOOR: f32 = 11.0;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_name_keeps_its_font_and_a_long_one_shrinks_then_ends_with_an_ellipsis() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_pass(egui::RawInput::default(), |ctx| {
            let p = Painter::new(ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING);
            let font = FontId::proportional(13.0);
            let size_of = |g: &Galley| g.job.sections.first().map_or(0.0, |s| s.format.font_id.size);
            let short = fit(&p, "Sky Mask 1", font.clone(), Color32::WHITE, 400.0);
            assert!(!short.elided && (size_of(&short) - 13.0).abs() < 1e-3);
            // a name that fits only a little smaller
            let name = "Background Mask 12";
            let wide = p.layout_no_wrap(name.into(), font.clone(), Color32::WHITE).size().x;
            let snug = fit(&p, name, font.clone(), Color32::WHITE, wide - 6.0);
            assert!(!snug.elided && size_of(&snug) < 13.0 && size_of(&snug) >= SMALLEST, "shrunk to {}", size_of(&snug));
            // far too long: the smallest size, cut
            let long = fit(&p, "A very long mask name that the row cannot hold at any size", font, Color32::WHITE, 90.0);
            assert!(long.elided && (size_of(&long) - SMALLEST).abs() < 1e-3);
            assert!(long.size().x <= 90.5);
        });
        out.textures_delta.clear();
    }
}
