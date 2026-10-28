//! What the shortcut editor draws over the window each frame: the Assign Shortcut box and the hint
//! that follows the first step of a chord.

use egui::{Align2, RichText};

use crate::LightcraftApp;
use crate::theme::Tokens;

use super::{capture, chord, context, runtime};

/// Draw the box and the chord hint (one call per frame, after the dialogs).
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    context::publish_keymap(ctx, &app.ui.settings.keymap);
    capture::show(app, ctx);
    hint(app, ctx);
}

/// A small card above the bottom bar while a chord waits for its next key: what was pressed and
/// the keys that finish a chord, each with its command. It goes away when the chord ends, times
/// out, or a dialog or prompt takes the screen.
fn hint(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(p) = app.keybinds.pending else { return };
    let now = ctx.input(|i| i.time);
    let blocked = app.ui.dialog.is_some()
        || app.ui.palette.is_some()
        || app.keybinds.capture.is_some()
        || crate::panels::notices::blocks_input(app)
        || crate::panels::library_problem::blocks_dialog_input(app);
    if blocked || now - p.since > runtime::TIMEOUT {
        app.keybinds.pending = None;
        return;
    }
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let t = Tokens::get(ctx);
    let lead = chord::spell(&[p.first]).map(|s| chord::text(&s, mac)).unwrap_or_default();
    let next = runtime::continuations(app, &p);
    let area = egui::Area::new(egui::Id::unique("lightcraft-chord-hint"))
        .order(egui::Order::Tooltip)
        .anchor(Align2::CENTER_BOTTOM, [0.0, -64.0])
        .interactable(false);
    area.show(ctx, |ui| {
        let frame = egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(14, 10));
        let out = frame.show(ui, |ui| {
            ui.set_min_width(260.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(RichText::new(crate::i18n::tr_format!("{} was pressed. Waiting for the next key…", lead)).font(t.semibold(13.0)).color(t.text));
            for (step, id) in next.iter().take(10) {
                let key = chord::spell(&[*step]).map(|s| chord::text(&s, mac)).unwrap_or_default();
                let label = crate::shortcuts::find_bindable(id).map_or(*id, |b| crate::i18n::tr(b.label));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(key).font(t.semibold(13.0)).color(t.accent));
                    ui.label(RichText::new(label).color(t.text_label));
                });
            }
            if next.len() > 10 {
                ui.label(RichText::new(crate::i18n::tr_format!("and {} more", next.len() - 10)).size(11.0).color(t.text_dim));
            }
            ui.label(RichText::new(crate::i18n::tr("Esc cancels")).size(11.0).color(t.text_dim));
        });
        crate::widgets::register(ui, "chordHint", out.response.rect);
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
}
