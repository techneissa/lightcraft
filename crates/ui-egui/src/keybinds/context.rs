//! "Assign Shortcut…" on a right-click: on an icon or button that runs a command, and on a menu
//! item. The click only leaves a request in egui's memory ([`request`]); the Assign Shortcut box
//! opens from it on the next frame ([`take_request`]), so a widget needs no access to the app.

use egui::{Id, Response};

use crate::shortcuts;

/// Automation id of an icon or text button (`icon:sidebar`, `button:auto`) → the command it runs.
/// Only widgets built by `widgets::icon_button` and `widgets::text_button` take part; a button that
/// has a context menu of its own (Appearance) is left out. A test checks every command exists.
pub const WIDGET_COMMANDS: &[(&str, &str)] = &[
    ("icon:sidebar", "view.leftPanel"),
    ("icon:back", "view.back"),
    ("icon:help", "app.shortcuts"),
    ("icon:share", "dialog.export"),
    ("icon:import", "file.addPhotos"),
    ("icon:bell", "panel.activity"),
    ("icon:photoGrid", "view.photoGrid"),
    ("icon:squareGrid", "view.squareGrid"),
    ("icon:detail", "view.detail"),
    ("icon:compare", "view.compare"),
    ("icon:survey", "view.survey"),
    ("icon:people", "view.people"),
    ("icon:autoAdvance", "view.autoAdvance"),
    ("icon:beforeAfter", "view.beforeAfter"),
    ("icon:beforeAfterSplit", "view.beforeAfterSplit"),
    ("icon:faceBoxes", "view.faceBoxes"),
    ("icon:filmstrip", "view.filmstrip"),
    ("icon:presets", "panel.presets"),
    ("icon:edit", "panel.edit"),
    ("icon:crop", "panel.crop"),
    ("icon:remove", "panel.remove"),
    ("icon:masking", "panel.masking"),
    ("icon:versions", "panel.versions"),
    ("icon:activity", "panel.activity"),
    ("icon:keywords", "panel.keywords"),
    ("icon:info", "panel.info"),
    ("icon:profileBrowser", "panel.profiles"),
    ("icon:maskOverlay", "view.maskOverlay"),
    ("icon:maskOverlayAll", "view.maskOverlayAll"),
    ("icon:maskDelete", "mask.delete"),
    ("button:auto", "develop.auto"),
    ("button:bw", "develop.treatment"),
    ("button:hdr", "develop.hdr"),
    ("button:reset", "develop.reset"),
    ("button:versionCreate", "version.create"),
];

/// The command the widget registered as `widget` runs, if it is in [`WIDGET_COMMANDS`].
pub fn command_for_widget(widget: &str) -> Option<&'static str> {
    WIDGET_COMMANDS.iter().find(|(w, _)| *w == widget).map(|(_, c)| *c)
}

fn request_id() -> Id {
    Id::unique("lightcraft-assign-shortcut-request")
}

/// Ask for the Assign Shortcut box for `command` (it opens on the next frame).
pub fn request(ctx: &egui::Context, command: &str) {
    ctx.data_mut(|d| d.insert_temp(request_id(), command.to_string()));
    ctx.request_repaint();
}

/// The command a right-click asked for since the last frame.
pub fn take_request(ctx: &egui::Context) -> Option<String> {
    ctx.data_mut(|d| d.remove_temp::<String>(request_id()))
}

/// The "Assign Shortcut…" context menu on `response` when `widget` runs a command.
pub fn attach(response: &Response, widget: &str) {
    if let Some(command) = command_for_widget(widget) {
        let _ = crate::widgets::context_menu(response, |ui| assign_item(ui, command));
    }
}

/// The "Assign Shortcut…" row of a context menu for `command`, with its current shortcut at the right.
pub fn assign_item(ui: &mut egui::Ui, command: &str) {
    let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
    let mut button = egui::Button::new(crate::i18n::tr("Assign Shortcut…"));
    let keymap = crate::keybinds::model::effective(&app_keymap(ui.ctx()), command, 0).map(|sc| super::chord::text(sc, mac));
    if let Some(sc) = keymap {
        button = button.shortcut_text(sc);
    }
    let r = ui.add(button);
    crate::widgets::register(ui, "menu:assignShortcut", r.rect);
    if r.clicked() {
        request(ui.ctx(), command);
        ui.close();
    }
}

/// A right-click on a menu item asks for the box directly (the item's menu closes first).
pub fn on_menu_item(ui: &egui::Ui, response: &Response, command: &str) {
    if response.secondary_clicked() && shortcuts::find_bindable(command).is_some() {
        request(ui.ctx(), command);
        ui.close();
    }
}

/// The keymap as the last frame left it, for labelling widgets that have no access to the app.
fn app_keymap(ctx: &egui::Context) -> shortcuts::Keymap {
    ctx.data(|d| d.get_temp::<shortcuts::Keymap>(Id::unique("lightcraft-keymap-snapshot"))).unwrap_or_default()
}

/// Remember the keymap for [`assign_item`]; called once per frame from the overlay pass.
pub fn publish_keymap(ctx: &egui::Context, keymap: &shortcuts::Keymap) {
    ctx.data_mut(|d| d.insert_temp(Id::unique("lightcraft-keymap-snapshot"), keymap.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_widget_command_exists_and_can_have_a_shortcut() {
        for (widget, command) in WIDGET_COMMANDS {
            assert!(shortcuts::find_bindable(command).is_some(), "{widget} runs {command}, which is not a command");
        }
        let mut ids: Vec<_> = WIDGET_COMMANDS.iter().map(|(w, _)| *w).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), WIDGET_COMMANDS.len(), "a widget id is listed twice");
    }
}
