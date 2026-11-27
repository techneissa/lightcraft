//! The eyedropper magnifier on the photo (issue #459): every on-canvas eyedropper (white balance, Color Range, Point
//! Color, Recolor's two, Recolor's) shows [`crate::widgets::magnifier`] while the pointer is over the photo. It reads the pixels of
//! the loupe's own render, the ones drawn on screen, at the pointer.

use egui::{Pos2, Rect, Response, vec2};
use lightcraft_catalog::PhotoId;

use super::fit_texture_rect;
use crate::LightcraftApp;
use crate::widgets::magnifier::{self, Patch};

/// The magnifier's reading this frame: the patch of pixels and where the pointer is.
pub struct Magnified {
    pub patch: Patch,
    pub at: Pos2,
}

/// An on-canvas tool that picks a colour or a neutral from the photo.
pub(crate) fn armed(tool: &str) -> bool {
    matches!(tool, "wbPicker" | "colorRange" | "pointColor")
        || crate::panels::mask_recolor::pick_target(tool).is_some()
        || crate::panels::edit::color_swap_pick_target(tool).is_some()
}

/// Read the pixels under the pointer into `app.eyedropper` (cleared when no eyedropper is armed or the pointer is not on
/// the photo). `views` are the photo's views, each as (the photo's rectangle, the pane it is shown in): one, or
/// the two of a side by side view, which show the same frame.
pub(super) fn read(app: &mut LightcraftApp, resp: &Response, id: PhotoId, views: &[(Rect, Rect)]) {
    app.eyedropper = None;
    if !armed(&app.ui.tool) || app.space_hand.is_some() || !(resp.hovered() || resp.dragged()) {
        return;
    }
    let Some(q) = resp.hover_pos().or(resp.interact_pointer_pos()) else { return };
    let Some(&(img, _)) = views.iter().find(|(img, pane)| pane.contains(q) && img.contains(q)) else { return };
    // the render on screen, read back from the GPU when it is drawn from there
    let Some(main) = app.renderer.main_pixels(id) else { return };
    let shown = fit_texture_rect(img, main.size);
    if !shown.contains(q) {
        return;
    }
    let Some(mut patch) = magnifier::sample(&main.pixels, vec2((q.x - shown.min.x) / shown.width(), (q.y - shown.min.y) / shown.height())) else {
        return;
    };
    if !main.current {
        // a stand-in for another look (still loading): the magnifier shows it, but its value would be the wrong colour
        patch.set_srgb(None);
    } else if let Some(display) = app.renderer.display() {
        // a monitor profile: what is on screen is the display's device values, turned back into sRGB for the value
        patch.set_srgb(patch.shown_rgb().and_then(|rgb| display.profile.device_pixel_to_srgb(rgb)));
    }
    app.eyedropper = Some(Magnified { patch, at: q });
}

/// Draw the magnifier read by [`read`] (after the tools, so its cursor is the one that counts).
pub(super) fn paint(app: &LightcraftApp, ctx: &egui::Context) {
    if let Some(m) = &app.eyedropper {
        magnifier::paint(ctx, m.at, &m.patch);
    }
}

/// `ui.inspect` → `eyedropper`: the colour under an armed eyedropper's pointer.
pub(crate) fn to_json(app: &LightcraftApp) -> serde_json::Value {
    use serde_json::json;
    app.eyedropper.as_ref().map_or(serde_json::Value::Null, |m| {
        json!({
            "tool": app.ui.tool,
            "pixel": m.patch.pixel,
            "hex": m.patch.hex(),
            "name": m.patch.name(),
            "rgb": m.patch.rgb(),
        })
    })
}
