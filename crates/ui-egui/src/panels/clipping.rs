//! Clipping warnings: the histogram's two corner triangles and the photo overlay they switch.
//!
//! Layout and behaviour only; the triangle is drawn in code (original work, no bitmap).

use egui::{Color32, ColorImage, Painter, Pos2, Rect, Shape, pos2};

/// The shadow overlay's colour where all three channels sit at black.
pub const SHADOW_SOLID: Color32 = Color32::from_rgb(40, 90, 255);
/// The highlight overlay's colour where all three channels sit at white.
pub const HIGHLIGHT_SOLID: Color32 = Color32::from_rgb(255, 50, 50);

/// The colour of a triangle indicator: `None` when no channel clips (the caller draws it dim),
/// white when all three do, otherwise the channel colour, or the mix of the clipping channels
/// (red + green = yellow, green + blue = cyan, red + blue = magenta).
pub fn indicator_color(channels: [bool; 3]) -> Option<Color32> {
    let [r, g, b] = channels;
    // pure channel colours are lifted a little so they read on the dark button
    let on = |v: bool| if v { 255 } else { 40 };
    match (r, g, b) {
        (false, false, false) => None,
        (true, true, true) => Some(Color32::WHITE),
        _ => Some(Color32::from_rgb(on(r), on(g), on(b))),
    }
}

/// What the overlay paints for one pixel of the rendered photo: highlights (red) win over shadows
/// (blue) in the rare pixel that has both. A pixel with all three channels at the limit is solid; one
/// with only some channels tints with those channels' colour and stays a little see-through.
pub fn overlay_pixel(px: [u8; 3], shadows: bool, highlights: bool) -> Color32 {
    let tint = |hit: [bool; 3], solid: Color32| match hit {
        [true, true, true] => solid,
        _ => {
            let [r, g, b] = hit;
            let c = |v: bool| if v { 255u8 } else { 30 };
            Color32::from_rgba_unmultiplied(c(r), c(g), c(b), 200)
        }
    };
    if highlights {
        let hit = px.map(|v| v == 255);
        if hit.iter().any(|&h| h) {
            return tint(hit, HIGHLIGHT_SOLID);
        }
    }
    if shadows {
        let hit = px.map(|v| v == 0);
        if hit.iter().any(|&h| h) {
            return tint(hit, SHADOW_SOLID);
        }
    }
    Color32::TRANSPARENT
}

/// The overlay image for a rendered photo: transparent except where the enabled sides clip.
pub fn overlay_image(src: &ColorImage, shadows: bool, highlights: bool) -> ColorImage {
    let pixels = src.pixels.iter().map(|c| overlay_pixel([c.r(), c.g(), c.b()], shadows, highlights)).collect();
    ColorImage { size: src.size, source_size: src.source_size, pixels }
}

/// The overlay texture for the photo shown, rebuilt only when its pixels or the sides change.
pub struct ClipOverlay {
    /// (texture key, size, shadows, highlights) it was built for
    pub made_for: (u64, [usize; 2], bool, bool),
    pub tex: egui::TextureHandle,
}

/// The sides of the overlay to show: a side is on when its toggle is, or while the pointer rests on
/// its triangle (Lightroom's momentary preview). The hover is only honoured when it was set this
/// frame or the last, so a hidden histogram cannot leave a side stuck on.
pub fn effective_sides(app: &crate::LightcraftApp, frame: u64) -> (bool, bool) {
    let (hover, at) = app.ui.clip_hover;
    let hovered = frame.saturating_sub(at) <= 1;
    (app.ui.clip_shadows || (hovered && hover[0]), app.ui.clip_highlights || (hovered && hover[1]))
}

/// A warning triangle pointing up, centred in `rect`.
pub fn paint_triangle(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let (w, h) = (rect.width().min(rect.height()) * 0.5, rect.height().min(rect.width()) * 0.44);
    let pts: Vec<Pos2> = vec![pos2(c.x, c.y - h), pos2(c.x + w, c.y + h), pos2(c.x - w, c.y + h)];
    p.add(Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indicator_colour_none_single_mixed_all() {
        assert_eq!(indicator_color([false; 3]), None);
        assert_eq!(indicator_color([true; 3]), Some(Color32::WHITE));
        assert_eq!(indicator_color([true, false, false]), Some(Color32::from_rgb(255, 40, 40)));
        assert_eq!(indicator_color([false, true, false]), Some(Color32::from_rgb(40, 255, 40)));
        assert_eq!(indicator_color([false, false, true]), Some(Color32::from_rgb(40, 40, 255)));
        assert_eq!(indicator_color([true, true, false]), Some(Color32::from_rgb(255, 255, 40)), "yellow");
        assert_eq!(indicator_color([false, true, true]), Some(Color32::from_rgb(40, 255, 255)), "cyan");
        assert_eq!(indicator_color([true, false, true]), Some(Color32::from_rgb(255, 40, 255)), "magenta");
    }

    #[test]
    fn overlay_paints_only_the_enabled_side() {
        let src = ColorImage {
            size: [3, 1],
            source_size: egui::vec2(3.0, 1.0),
            pixels: vec![Color32::from_rgb(0, 0, 0), Color32::from_rgb(255, 255, 255), Color32::from_rgb(120, 120, 120)],
        };
        let both = overlay_image(&src, true, true);
        assert_eq!(both.pixels, vec![SHADOW_SOLID, HIGHLIGHT_SOLID, Color32::TRANSPARENT]);
        let shadows = overlay_image(&src, true, false);
        assert_eq!(shadows.pixels, vec![SHADOW_SOLID, Color32::TRANSPARENT, Color32::TRANSPARENT]);
        let highlights = overlay_image(&src, false, true);
        assert_eq!(highlights.pixels, vec![Color32::TRANSPARENT, HIGHLIGHT_SOLID, Color32::TRANSPARENT]);
        assert!(overlay_image(&src, false, false).pixels.iter().all(|c| *c == Color32::TRANSPARENT));
    }

    #[test]
    fn partial_clipping_tints_with_the_channel_and_is_not_solid() {
        let one = overlay_pixel([255, 100, 100], false, true);
        assert_ne!(one, HIGHLIGHT_SOLID);
        let [r, g, b, a] = one.to_srgba_unmultiplied();
        assert_eq!((r, a), (255, 200));
        assert!(g < 100 && b < 100, "only the red channel tints: {one:?}");
        let dark = overlay_pixel([0, 0, 90], true, false);
        let [r, g, b, _] = dark.to_srgba_unmultiplied();
        assert!(r > 200 && g > 200 && b < 100, "red and green clipped: yellow {dark:?}");
    }
}
