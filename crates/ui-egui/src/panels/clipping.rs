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

/// What the loupe shows in place of the photo while ⌥ is held during a slider drag (Lightroom's
/// Alt-drag preview), or while `view.clippingPreview` holds one for an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preview {
    /// Black image, only the pixels clipped at white (Exposure, Whites, Highlights).
    Highlights,
    /// White image, only the pixels crushed to black (Blacks, Shadows).
    Shadows,
    /// The black-and-white mask that limits sharpening (Sharpening Masking).
    SharpenMask,
    /// The photo in greyscale (Sharpening Amount, Radius, Detail).
    Greyscale,
}

impl Preview {
    /// Stable name (control channel).
    pub fn name(self) -> &'static str {
        match self {
            Preview::Highlights => "highlights",
            Preview::Shadows => "shadows",
            Preview::SharpenMask => "sharpenMask",
            Preview::Greyscale => "greyscale",
        }
    }

    pub fn parse(name: &str) -> Option<Preview> {
        [Preview::Highlights, Preview::Shadows, Preview::SharpenMask, Preview::Greyscale].into_iter().find(|p| p.name() == name)
    }

    /// The preview a slider's drag asks for.
    pub fn for_control(id: &str) -> Option<Preview> {
        match id {
            "light.exposure" | "light.whites" | "light.highlights" => Some(Preview::Highlights),
            "light.blacks" | "light.shadows" => Some(Preview::Shadows),
            "detail.sharpenMasking" => Some(Preview::SharpenMask),
            "detail.sharpenAmount" | "detail.sharpenRadius" | "detail.sharpenDetail" => Some(Preview::Greyscale),
            _ => None,
        }
    }

    /// The render's overlay that draws it.
    pub fn overlay(self) -> lightcraft_pipeline::Overlay {
        use lightcraft_pipeline::Overlay;
        match self {
            Preview::Highlights => Overlay::ClipHighlights,
            Preview::Shadows => Overlay::ClipShadows,
            Preview::SharpenMask => Overlay::SharpenMask,
            Preview::Greyscale => Overlay::Greyscale,
        }
    }
}

/// A slider drag's last frame: which control, whether ⌥ was down, and the frame it was seen on.
#[derive(Clone, Copy)]
struct DragNote(&'static str, bool, u64);

/// A slider reports each frame it is being dragged (by the thumb or by scrubbing its value).
pub fn note_drag(ctx: &egui::Context, control: &'static str, alt: bool) {
    let note = DragNote(control, alt, ctx.cumulative_frame_nr());
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("clipping-drag-note"), note));
}

/// Settle [`crate::state::UiState::alt_preview`] for this frame: the control being dragged, with
/// ⌥ down on a drag seen this frame or the last. Anything else (the drag or ⌥ ended, the panel
/// closed) clears it, so a preview can never stay up.
pub fn sync_alt_preview(app: &mut crate::LightcraftApp, ctx: &egui::Context) {
    let note = ctx.data(|d| d.get_temp::<DragNote>(egui::Id::unique("clipping-drag-note")));
    let dragging = app.ui.dragging_control.as_deref();
    app.ui.alt_preview = match note {
        Some(DragNote(control, true, at)) if dragging == Some(control) && ctx.cumulative_frame_nr().saturating_sub(at) <= 1 => {
            Preview::for_control(control)
        }
        _ => None,
    };
}

/// The preview on show: the one a command holds, else the Alt-drag's.
pub fn preview(app: &crate::LightcraftApp) -> Option<Preview> {
    app.ui.preview_forced.or(app.ui.alt_preview)
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
    fn each_slider_asks_for_its_preview() {
        for id in ["light.exposure", "light.whites", "light.highlights"] {
            assert_eq!(Preview::for_control(id), Some(Preview::Highlights), "{id}");
        }
        for id in ["light.blacks", "light.shadows"] {
            assert_eq!(Preview::for_control(id), Some(Preview::Shadows), "{id}");
        }
        assert_eq!(Preview::for_control("detail.sharpenMasking"), Some(Preview::SharpenMask));
        for id in ["detail.sharpenAmount", "detail.sharpenRadius", "detail.sharpenDetail"] {
            assert_eq!(Preview::for_control(id), Some(Preview::Greyscale), "{id}");
        }
        for id in ["light.contrast", "color.vibrance", "detail.noiseLuminance", ""] {
            assert_eq!(Preview::for_control(id), None, "{id}");
        }
        for p in [Preview::Highlights, Preview::Shadows, Preview::SharpenMask, Preview::Greyscale] {
            assert_eq!(Preview::parse(p.name()), Some(p));
        }
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
