//! The line under a Subject, Sky or Background part that the classical heuristic draws (fork feature, issue #37): it
//! says the mask is rough and, without SAM 3, how to get it. A click opens the SAM 3 download. Also the line under an
//! AI part that selects nothing: still being found on this photo, or not found on it (issue #399).

use egui;
use lightcraft_develop::MaskShape;

use crate::LightcraftApp;
use crate::theme::Tokens;

/// The line for `shape`, or nothing when its selection is not rough or the build has no SAM 3 at all.
pub fn line(app: &mut LightcraftApp, ui: &mut egui::Ui, shape: &MaskShape) {
    if !lightcraft_engine::segment::Segmenter::AVAILABLE || !app.session.is_rough_ai_mask(shape) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let seg = &app.session.segmenter;
    if seg.has_model() {
        // a mask from before SAM 3 was installed keeps its look; adding it again uses the model
        ui.label(egui::RichText::new(crate::i18n::tr("Rough mask. Add it again to use SAM 3.")).color(t.text_dim).size(11.0));
    } else {
        let text = egui::RichText::new(crate::i18n::tr("Rough mask. Click to install SAM 3.")).color(t.text_dim).size(11.0);
        let resp = ui.add(egui::Button::new(text).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand);
        crate::widgets::register(ui, "button:maskRough", resp.rect);
        if resp.clicked() {
            app.offer_sam_download(None);
        }
    }
}

/// The line for a part that is waiting to be found on the photo, or whose search selected nothing.
pub fn unfound(app: &LightcraftApp, ui: &mut egui::Ui, shape: &MaskShape) {
    let text = match (shape.is_waiting(), app.session.adaptive_busy()) {
        (true, true) => "Finding AI masks",
        (true, false) => "Not found yet. Use Update AI Masks in the Photo menu.",
        (false, _) => "Not found on this photo.",
    };
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(crate::i18n::tr(text)).color(t.text_dim).size(11.0));
}
