//! Fork (issue #333, `fork/relight.md`): the Underwater group of the Relight section, for the whole photo: how much water
//! there is between the camera and each thing (Water Depth), with its Clarity (adjustable once there is some) and its
//! Tint (once any of the look is on), then Surface Shafts and Caustics. Each is a control by id: `relight.water`,
//! `.waterClarity`, `.waterTint`, `.waterShafts`, `.waterCaustics`.

use lightcraft_develop::DevelopSettings;

use super::super::{Icon, LightcraftApp, control};

/// The group: a flyout row "Underwater" that opens the water's controls.
pub(super) fn water(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let open = app.ui.flyout_open("relightWater");
    if crate::widgets::flyout_row(ui, "relightWater", "Underwater", Icon::Sun, open).clicked() {
        app.ui.toggle_flyout("relightWater");
    }
    if !open {
        return;
    }
    control(app, ui, d, "relight.water", true);
    // (always shown, greyed out while they have nothing to act on, so no row appears above the one being dragged:
    // issue #470; the tint is the hue of the caustics and the shafts too)
    control(app, ui, d, "relight.waterClarity", d.relight.water.amount > 0.0);
    control(app, ui, d, "relight.waterTint", d.relight.water.is_on());
    control(app, ui, d, "relight.waterShafts", true);
    control(app, ui, d, "relight.waterCaustics", true);
}
