//! Shortcuts that work while a dialog, the Command Palette or a text box owns the keyboard. They
//! are the ones whose condition needs it: it reads `textFocus`, `dialogOpen` or `paletteOpen` and
//! would not hold without it (`textFocus` itself, not `!textFocus || view == detail`).

use crate::LightcraftApp;
use crate::shortcuts::{self, matches};

use super::model::{self, conditions};
use super::when::Context;
use super::when::keys::{DIALOG_OPEN, PALETTE_OPEN, TEXT_FOCUS};

const BLOCKERS: [usize; 3] = [TEXT_FOCUS, DIALOG_OPEN, PALETTE_OPEN];

/// The commands whose one-step shortcut was pressed this frame and whose condition asks for a
/// blocked keyboard.
pub fn firing(app: &LightcraftApp, ctx: &egui::Context) -> Vec<&'static str> {
    let keymap = &app.ui.settings.keymap;
    if !keymap.keys().any(|k| k.ends_with(conditions::SUFFIX)) {
        return Vec::new();
    }
    let cx = Context::of_app(app, ctx);
    let mut calm = cx;
    BLOCKERS.iter().for_each(|k| calm.set_flag(*k, false));
    let mut out = Vec::new();
    ctx.input(|i| {
        for b in shortcuts::bindable() {
            let Some(when) = conditions::of(keymap, b.id) else { continue };
            if !BLOCKERS.iter().any(|k| when.mentions(*k)) || !when.eval(&cx) || when.eval(&calm) {
                continue;
            }
            let hit = (0..model::SLOTS).filter_map(|slot| model::effective(keymap, b.id, slot)).any(|sc| {
                matches!(shortcuts::parse(sc), Some((m, k)) if !app.native_shortcuts.contains(sc) && !super::chord::is_chord(sc) && matches(i, m, k))
            });
            if hit {
                out.push(b.id);
            }
        }
    });
    out
}
