//! Editing keyboard shortcuts from anywhere, VS Code style (fork feature `feat/shortcut-editing`).
//!
//! - [`model`]: two shortcuts per command (slot 1 is the primary one that menus and the Command
//!   Palette show, slot 2 also works), each one step or a chord, with conflicts and reset, stored in
//!   the existing keymap ([`crate::shortcuts::Keymap`]) so an old `ui.json` loads unchanged.
//! - [`chord`]: the shortcut strings (`Cmd+K C`): parse, spell, show.
//! - [`runtime`]: the leader key: the first step of a chord arms it, the next key finishes it (or
//!   Esc, a wrong key or the timeout cancels), and [`overlay`] shows what is pending.
//! - [`capture`]: the "Assign Shortcut" box (press the keys, Enter saves), opened from the Keyboard
//!   Shortcuts window, a right-click ([`context`]) or the `app.assignShortcut` command.
//! - [`window`]: the Keyboard Shortcuts window itself.

pub mod capture;
pub mod chord;
pub mod context;
pub mod model;
pub mod overlay;
pub mod runtime;
pub mod window;

/// What the shortcut editor keeps between frames (a field of [`crate::LightcraftApp`]).
#[derive(Default)]
pub struct State {
    /// The first step of a chord was pressed and the next key is awaited.
    pub pending: Option<runtime::Pending>,
    /// The Assign Shortcut box, while it is open.
    pub capture: Option<capture::Capture>,
}

/// `app.assignShortcut {id, slot?}`: open the Assign Shortcut box for a command (`slot` 1 or 2; the
/// first free one by default). The keys are then pressed (`ui.key`) and Enter saves.
pub fn command(app: &mut crate::LightcraftApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let id = p.get("id").and_then(serde_json::Value::as_str).ok_or("missing id")?;
    let slot = match p.get("slot") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .and_then(|n| n.checked_sub(1))
                .filter(|n| *n < model::SLOTS)
                .ok_or("slot must be 1 or 2")?,
        ),
    };
    capture::open(app, id, slot)?;
    Ok(serde_json::Value::Null)
}
