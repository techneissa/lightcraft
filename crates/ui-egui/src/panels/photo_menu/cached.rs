//! The menu-bar commands a customised photo menu shows, kept while the menu stays open: building
//! them ([`super::menu_commands`]) walks the whole menu bar and asks every command whether it is
//! enabled, too much to do on each frame the menu is drawn. They are built when the menu opens and
//! again when the keymap, the selection or the library changes (an edit, an undo), and at least
//! every [`REFRESH`] seconds for the rest of what enablement reads.

use std::sync::Arc;

use crate::LightcraftApp;
use crate::menus::menubar::MenuNode;

/// Most seconds an open menu shows the same enablement.
const REFRESH: f64 = 1.0;

/// What the list was built for.
#[derive(Clone, PartialEq)]
struct Key {
    keymap: crate::shortcuts::Keymap,
    selection: lightcraft_engine::Selection,
    undo: usize,
    redo: usize,
    language: crate::i18n::Locale,
}

#[derive(Clone)]
struct Cached {
    key: Key,
    /// The frame it was last drawn on and the time it was built.
    frame: u64,
    built: f64,
    commands: Arc<Vec<(MenuNode, String)>>,
}

/// [`super::menu_commands`] for the open photo menu, built again only when needed (see the module
/// docs).
pub(super) fn commands(app: &LightcraftApp, ctx: &egui::Context) -> Arc<Vec<(MenuNode, String)>> {
    let id = egui::Id::unique("photo-menu-commands");
    let (frame, now) = (ctx.cumulative_frame_nr(), ctx.input(|i| i.time));
    let s = &app.session;
    let key = Key {
        keymap: app.ui.settings.keymap.clone(),
        selection: s.selection.clone(),
        undo: s.undo.len(),
        redo: s.redo.len(),
        language: crate::i18n::language(),
    };
    let kept = ctx.data(|d| d.get_temp::<Cached>(id)).filter(|c| {
        // drawn on the last frame too: the menu stayed open
        frame.saturating_sub(c.frame) <= 1 && c.key == key && now - c.built < REFRESH
    });
    let (commands, built) = match kept {
        Some(c) => (c.commands, c.built),
        None => (Arc::new(super::menu_commands(app)), now),
    };
    ctx.data_mut(|d| d.insert_temp(id, Cached { key, frame, built, commands: commands.clone() }));
    commands
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::LightcraftApp;

    /// One open menu: the commands are built once over its frames, again when the selection
    /// changes, and again when the menu is opened anew.
    #[test]
    fn the_open_photo_menu_builds_its_commands_once() {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
        let ctx = egui::Context::default();
        let frame = |ctx: &egui::Context, app: &LightcraftApp| {
            let mut out = None;
            ctx.run_ui(egui::RawInput::default(), |ui| out = Some(super::commands(app, ui.ctx()))).textures_delta.clear();
            out.unwrap()
        };
        let first = frame(&ctx, &app);
        let second = frame(&ctx, &app);
        assert!(Arc::ptr_eq(&first, &second), "the same list while the menu stays open");
        let ids: Vec<u64> = app.session.catalog.photos().map(|p| p.id.0).take(2).collect();
        app.session.execute("library.select", &serde_json::json!({"ids": ids})).unwrap();
        let third = frame(&ctx, &app);
        assert!(!Arc::ptr_eq(&second, &third), "a new selection can enable other commands");
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let reopened = frame(&ctx, &app);
        assert!(!Arc::ptr_eq(&third, &reopened), "a menu opened again is built again");
    }
}
