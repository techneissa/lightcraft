//! The leader key: pressing the first step of a chord arms it, the next key finishes it. Esc, a key
//! that completes no chord, or [`TIMEOUT`] seconds without a key cancel it.

use egui::{Key, Modifiers};

use crate::LightcraftApp;
use crate::shortcuts::matches_ex;

use super::chord::Step;
use super::model;

/// Seconds the leader key waits for the next key.
pub const TIMEOUT: f64 = 4.0;

/// A chord whose first step was pressed.
#[derive(Clone, Copy, Debug)]
pub struct Pending {
    pub first: Step,
    /// `egui` time of the press.
    pub since: f64,
}

/// What the key pass does after [`poll`].
#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    /// Not a chord key: the normal shortcuts go on.
    Pass,
    /// The keys were used up (a leader pressed, a chord cancelled): nothing else fires this frame.
    Swallow,
    /// A chord finished: run this command.
    Fire(&'static str),
}

/// Whether a key (not a lone modifier) was freshly pressed this frame: the repeats of a key still
/// held down (the leader, usually) don't count.
fn any_key(i: &egui::InputState) -> bool {
    i.events.iter().any(|e| matches!(e, egui::Event::Key { key, pressed: true, repeat: false, .. } if !crate::shortcuts::is_modifier(*key)))
        || i.events.iter().any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)))
}

/// Remove this frame's key presses, so nothing else sees the ones the chord took.
pub fn consume_keys(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            !matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_))
        });
    });
}

/// Advance the chord state with this frame's keys (call once per frame, before the single-step
/// shortcuts, while nothing else owns the keyboard).
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) -> Poll {
    let now = ctx.input(|i| i.time);
    let chords = model::chords(&app.ui.settings.keymap);
    if let Some(p) = app.keybinds.pending {
        if now - p.since > TIMEOUT {
            app.keybinds.pending = None;
        } else {
            return follow(app, ctx, p, &chords, now);
        }
    }
    let leader =
        ctx.input(|i| chords.iter().find(|(_, [first, _])| matches_ex(i, first.0, first.1, true, Modifiers::NONE)).map(|(_, [first, _])| *first));
    let Some(first) = leader else { return Poll::Pass };
    consume_keys(ctx);
    app.keybinds.pending = Some(Pending { first, since: now });
    ctx.request_repaint_after(std::time::Duration::try_from_secs_f64(TIMEOUT + 0.05).unwrap_or_default());
    Poll::Swallow
}

/// The frame after a leader: the next key finishes the chord, or cancels it.
fn follow(app: &mut LightcraftApp, ctx: &egui::Context, p: Pending, chords: &[(&'static str, [Step; 2])], now: f64) -> Poll {
    if !ctx.input(any_key) {
        ctx.request_repaint_after(std::time::Duration::try_from_secs_f64((TIMEOUT - (now - p.since)).max(0.0) + 0.05).unwrap_or_default());
        return Poll::Swallow;
    }
    let (esc, hit) = ctx.input(|i| {
        let next = || chords.iter().filter(|(_, [first, _])| *first == p.first);
        // the key as pressed first; failing that, with the modifiers of the leader still held taken
        // off, so ⌘K then C works with ⌘ kept down
        let hit = next()
            .find(|(_, [_, second])| matches_ex(i, second.0, second.1, true, Modifiers::NONE))
            .or_else(|| next().find(|(_, [_, second])| matches_ex(i, second.0, second.1, true, p.first.0)))
            .map(|(id, _)| *id);
        (matches_ex(i, Modifiers::NONE, Key::Escape, true, Modifiers::NONE), hit)
    });
    consume_keys(ctx);
    app.keybinds.pending = None;
    match hit {
        Some(id) => Poll::Fire(id),
        None => {
            if !esc {
                let mac = ctx.os() == egui::os::OperatingSystem::Mac;
                let lead = super::chord::spell(&[p.first]).map(|s| super::chord::text(&s, mac)).unwrap_or_default();
                app.toast(ctx, crate::i18n::tr_format!("{} was pressed, but no shortcut continues with that key", lead));
            }
            Poll::Swallow
        }
    }
}

/// The chords that continue the pending leader: the next step and its command.
pub fn continuations(app: &LightcraftApp, p: &Pending) -> Vec<(Step, &'static str)> {
    let mut v: Vec<(Step, &'static str)> = model::chords(&app.ui.settings.keymap)
        .into_iter()
        .filter(|(_, [first, _])| *first == p.first)
        .map(|(id, [_, second])| (second, id))
        .collect();
    v.sort_by_key(|(_, id)| *id);
    v
}
