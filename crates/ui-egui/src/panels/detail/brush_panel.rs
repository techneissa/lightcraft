//! The brush panel a right-click on the photo opens while a brush is in use (Photoshop's brush
//! picker): the Masking brush's Size, Feather and Flow, or the Remove tool's Size, Feather and
//! Opacity, at the pointer, so the brush changes without a trip to the side panel.

use super::LightcraftApp;
use crate::state::RightPanel;

/// Whether a brush is in use: `Some(false)` the Masking brush, `Some(true)` the Remove tool's.
pub(super) fn active(app: &LightcraftApp) -> Option<bool> {
    match app.ui.right {
        RightPanel::Masking if app.ui.tool == "brush" => Some(false),
        RightPanel::Remove => Some(true),
        _ => None,
    }
}

/// The panel's sliders; they scale as the side panel's do (Masking Size 1-100, Remove Size 1-250).
pub(super) fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, remove: bool) {
    ui.set_min_width(220.0);
    let (scale, max, name) = if remove { (1000.0, 250.0, "Opacity") } else { (400.0, 100.0, "Flow") };
    let (size, feather, third) = if remove {
        (&mut app.ui.remove_size, &mut app.ui.remove_feather, &mut app.ui.remove_opacity)
    } else {
        (&mut app.ui.brush_size, &mut app.ui.brush_feather, &mut app.ui.brush_flow)
    };
    let mut shown = (*size * scale).clamp(1.0, max);
    if ui.add(egui::Slider::new(&mut shown, 1.0..=max).text(crate::i18n::tr("Size")).integer()).changed() {
        *size = shown / scale;
    }
    ui.add(egui::Slider::new(feather, 0.0..=100.0).text(crate::i18n::tr("Feather")).integer());
    ui.add(egui::Slider::new(third, if remove { 0.0 } else { 1.0 }..=100.0).text(crate::i18n::tr(name)).integer());
}
