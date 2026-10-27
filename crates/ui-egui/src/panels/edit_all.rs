//! The "Edit All N Selected" switch at the top of the Edit panel. With two or more photos selected it
//! turns Auto Sync (`develop.autoSync`) on or off: on, every change to the photo being edited also
//! changes the other selected photos. Edit > Auto Sync is the same setting.

use serde_json::json;

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// Widget id of the switch, for the control channel (`ui.clickWidget`).
pub const SWITCH_ID: &str = "editAll:switch";

/// Draw the switch for a selection of `n` photos (the caller shows it only for `n > 1`).
pub fn switch(app: &mut LightcraftApp, ui: &mut egui::Ui, n: usize) {
    let t = Tokens::get(ui.ctx());
    let mut on = app.session.auto_sync;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
        let label = crate::i18n::tr_format!("Edit All {n} Selected", n = n);
        let r = ui.add(egui::Checkbox::new(&mut on, label));
        register(ui, SWITCH_ID, r.rect);
        let (hint, color) = if on {
            (crate::i18n::tr("Changes apply to every selected photo"), t.accent)
        } else {
            (crate::i18n::tr("Changes apply only to the photo being edited"), t.text_dim)
        };
        ui.label(egui::RichText::new(hint).color(color).size(11.5));
        if r.changed() {
            let _ = app.run("develop.autoSync", json!({"on": on}));
        }
    });
}
