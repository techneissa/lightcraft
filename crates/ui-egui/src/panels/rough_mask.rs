//! The line under a Subject, Sky or Background part that the classical heuristic draws (fork feature, issue #37): it
//! says the mask is rough and, without SAM 3, how to get it. A click opens the SAM 3 download.

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
