//! What the shortcut editor draws over the window each frame: the Assign Shortcut box and the hint
//! that follows the first step of a chord.

use egui::{Align2, RichText};

use crate::LightcraftApp;
use crate::theme::Tokens;

use super::{capture, chord, context, runtime};

/// Draw the box and the chord hint (one call per frame, after the dialogs).
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    context::publish_keymap(ctx, &app.ui.settings.keymap);
    damaged_notice(app, ctx);
    capture::show(app, ctx);
    hint(app, ctx);
}

/// Once, when the keymap holds a condition that can't be read: say it is ignored.
fn damaged_notice(app: &mut LightcraftApp, ctx: &egui::Context) {
    if app.keybinds.damage_noticed {
        return;
    }
    app.keybinds.damage_noticed = true;
    let n = super::model::conditions::damaged(&app.ui.settings.keymap).len();
    if n > 0 {
        app.toast_for(ctx, crate::i18n::tr_format!("{} shortcut condition(s) in your settings could not be read and are ignored", n), 5.0);
    }
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
    let next = runtime::continuations(app, ctx, &p);
    let area = egui::Area::new(egui::Id::unique("lightcraft-chord-hint"))
        .order(egui::Order::Tooltip)
        .anchor(Align2::CENTER_BOTTOM, [0.0, -64.0])
        .interactable(false);
    area.show(ctx, |ui| {
        let frame = egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(14, 10));
        let out = frame.show(ui, |ui| {
            ui.set_min_width(200.0);
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
            // the key that started the chord, as a keycap, then each key that finishes it beside its command
            keycap(ui, &t, &lead, t.semibold(16.0));
            for (step, id) in next.iter().take(10) {
                let key = chord::spell(&[*step]).map(|s| chord::text(&s, mac)).unwrap_or_default();
                let label = crate::shortcuts::find_bindable(id).map_or(*id, |b| crate::i18n::tr(b.label));
                ui.horizontal(|ui| {
                    keycap(ui, &t, &key, t.semibold(13.0));
                    ui.label(RichText::new(label).color(t.text_label));
                });
            }
            if next.len() > 10 {
                ui.label(RichText::new(crate::i18n::tr_format!("and {} more", next.len() - 10)).size(11.0).color(t.text_dim));
            }
            ui.horizontal(|ui| {
                keycap(ui, &t, "Esc", t.font(11.0));
                ui.label(RichText::new(crate::i18n::tr("Cancel")).size(11.0).color(t.text_dim));
            });
        });
        crate::widgets::register(ui, "chordHint", out.response.rect);
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
}

/// `text` drawn as a key on a keyboard: a rounded box with the key's name in it.
fn keycap(ui: &mut egui::Ui, t: &Tokens, text: &str, font: egui::FontId) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, t.text);
    let size = galley.size() + egui::vec2(14.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect(rect, 4.0, t.button, egui::Stroke::new(1.0, t.button_border), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, t.text);
}
