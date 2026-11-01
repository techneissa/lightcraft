//! Settings ▸ Top Bar: the bar's items in the order of the bar, each with a check box, its icon,
//! its name, and on the right where the menu bar still has it.
//!
//! - A click on a row (or its box) shows or hides the item.
//! - Reset to Default shows every item again.
//!
//! Changes apply at once, as everywhere in Settings, and look like the Photo Menu tab's list.

use egui::{Rect, Sense, Stroke, pos2, vec2};

use super::{ITEMS, reach, reset, set_shown, shown};
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::panels::settings::{heading, hint};
use crate::theme::Tokens;
use crate::widgets::register;

const ROW_H: f32 = 26.0;

/// The Top Bar tab.
pub fn tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let commands = crate::panels::photo_menu::menu_commands(app);
    heading(ui, t, "Top Bar");
    hint(ui, t, "The buttons the top bar shows. A hidden button's command stays in the menu bar. The search field and the menus stay.");
    ui.add_space(4.0);

    let frame = egui::Frame::new().fill(t.inset).stroke(Stroke::new(1.0, t.divider)).corner_radius(6.0).inner_margin(4.0);
    let mut toggled: Option<(&str, bool)> = None;
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width() - 10.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        for item in ITEMS {
            let on = shown(app, item.id);
            let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            register(ui, format!("topBarRow:{}", item.id), rect);
            register(ui, format!("check:topBar-{}", item.id), checkbox_rect(rect));
            crate::widgets::announce_selected(&r, crate::i18n::tr(item.label), on);
            let p = ui.painter();
            if r.hovered() {
                p.rect_filled(rect, 4.0, t.hover);
            }
            let b = checkbox_rect(rect);
            p.rect(
                b,
                3.0,
                if on { t.accent } else { t.field },
                Stroke::new(1.0, if on { t.accent } else { t.field_border }),
                egui::StrokeKind::Inside,
            );
            if on {
                paint(p, b.shrink(1.5), Icon::Check, t.canvas);
            }
            let icon_colour = if on { t.icon } else { t.text_disabled };
            paint(p, Rect::from_center_size(pos2(rect.left() + 42.0, rect.center().y), vec2(16.0, 16.0)), item.icon, icon_colour);
            let label_colour = if on { t.text } else { t.text_dim };
            let g = p.layout_no_wrap(crate::i18n::tr(item.label).to_string(), t.font(12.5), label_colour);
            p.galley(pos2(rect.left() + 58.0, rect.center().y - g.size().y / 2.0), g, label_colour);
            let note = reach(&commands, item);
            let g = p.layout_no_wrap(note, t.font(10.5), t.text_dim);
            p.galley(pos2(rect.right() - 8.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, t.text_dim);
            if r.clicked() {
                toggled = Some((item.id, !on));
            }
        }
    });
    if let Some((id, on)) = toggled {
        set_shown(app, id, on);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let custom = !app.ui.settings.top_bar_hidden.is_empty();
        let r = ui.add_enabled(custom, egui::Button::new(crate::i18n::tr("Reset to Default")));
        register(ui, "button:topBarReset", r.rect);
        if r.clicked() {
            reset(app);
        }
    });
}

/// The check box at the left end of a row.
fn checkbox_rect(row: Rect) -> Rect {
    Rect::from_center_size(pos2(row.left() + 16.0, row.center().y), vec2(14.0, 14.0))
}
