//! Fork feature (`fork/parse-ahead.md`): keeps the engine's people parsing ahead of time going
//! (`parsing.ahead.pump`) and its switch in Settings ▸ Performance.
//!
//! The pace is the face scan's ([`crate::panels::faces::background_pace`]), held back further: nothing new while a
//! slider is dragged, or for [`QUIET_SECS`] after the user last dragged, typed or scrolled (edits come in bursts), also
//! in a window without the keyboard. The engine also waits while the loupe waits for its render and while an export
//! runs. Outside the Edit view the user culls: the engine parses nothing near the open photo then.

use serde_json::json;

use crate::LightcraftApp;
use crate::panels::settings::{check, hint, row};
use crate::theme::Tokens;

/// How long after the user's last drag, typing or scroll parsing ahead stays paused.
const QUIET_SECS: f64 = 1.5;
/// How often the pump is asked while there is work (it also takes finished photos in).
const BUSY_SECS: f64 = 0.2;
/// How often a pump with nothing to do is asked again (a frame that comes sooner does not ask).
const IDLE_SECS: f64 = 1.0;
/// How long after the pointer last moved the face scan's pace turns from light to normal (`pace_for`).
const STILL_SECS: f64 = 3.0;

fn next_id() -> egui::Id {
    egui::Id::unique("parse-ahead-next-pump")
}

/// The pace for parsing ahead now.
fn pace(app: &LightcraftApp, ctx: &egui::Context, now: f64) -> &'static str {
    held_back(app.ui.dragging_control.is_some(), now - app.caches.last_input, crate::panels::faces::background_pace(app, ctx, now))
}

/// The pace while a slider is (or is not) dragged, `worked` seconds after the user last dragged, typed or scrolled,
/// with the face scan's pace `base`. (A drag in a window without the keyboard pauses too: the loupe redraws for it.)
fn held_back(dragging: bool, worked: f64, base: &'static str) -> &'static str {
    if dragging || worked < QUIET_SECS { "pause" } else { base }
}

/// When to draw a frame again, from the pump's answer, so the work goes on in a window that draws none: soon while
/// there is work the pace allows, now and then while it is held back; when the user will count as idle while only
/// photos for an idle user wait (`idle_in`: how long until then, `None` when the pace will not change by itself);
/// else not.
fn repaint_in(state: &str, left: u64, queued: u64, idle_in: Option<f64>) -> Option<f64> {
    if matches!(state, "working" | "loading") {
        return Some(BUSY_SECS);
    }
    if state == "paused" || left > 0 {
        return Some(IDLE_SECS);
    }
    if queued > 0 {
        return idle_in.map(|s| s.max(BUSY_SECS));
    }
    None
}

/// How long until the face scan's pace turns normal by itself: the pointer still for [`STILL_SECS`] in a window with
/// the keyboard (a minimized window stays light; one without the keyboard is normal already).
fn idle_in(app: &LightcraftApp, ctx: &egui::Context, now: f64) -> Option<f64> {
    let (focused, minimized) = ctx.input(|i| (i.raw.focused, i.viewport().minimized.unwrap_or(false)));
    let still = now - app.caches.last_move;
    (focused && !minimized && still < STILL_SECS).then_some(STILL_SECS - still + 0.05)
}

/// Called every frame; asks the engine at most every [`BUSY_SECS`] while it has work and every [`IDLE_SECS`] when not.
pub fn pump(app: &mut LightcraftApp, ctx: &egui::Context, now: f64) {
    let pace = pace(app, ctx, now);
    let due = ctx.data(|d| d.get_temp::<f64>(next_id())).unwrap_or(0.0);
    // (a pause is passed on at once: it stops a model run going)
    if now < due && pace != "pause" {
        return;
    }
    // (outside the Edit view the user culls: nothing near the open photo is parsed, issue #200)
    let culling = !(app.ui.view == crate::state::ViewMode::Detail && crate::full_decode::editing(app.ui.right));
    let p = json!({"pace": pace, "backward": app.look_ahead.backward(), "culling": culling, "enabled": app.ui.settings.parse_ahead});
    let again = match app.session.execute("parsing.ahead.pump", &p) {
        Ok(v) => {
            repaint_in(v["state"].as_str().unwrap_or(""), v["left"].as_u64().unwrap_or(0), v["queued"].as_u64().unwrap_or(0), idle_in(app, ctx, now))
        }
        Err(e) => {
            log::warn!("parse ahead: {e}");
            None
        }
    };
    ctx.data_mut(|d| d.insert_temp(next_id(), now + again.unwrap_or(IDLE_SECS).min(IDLE_SECS)));
    // (a quiet window draws no frames: ask for one, so the work goes on, and photos left for when the user is idle
    // start once they are)
    if let Some(secs) = again {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(secs));
    }
}

/// The switch in Settings ▸ Performance.
pub fn setting(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(crate::i18n::tr("People Parts")).font(t.semibold(12.0)).color(t.text));
    row(ui, t, crate::i18n::tr("Masks"), |ui| {
        check(ui, "settings.parseAhead", &mut app.ui.settings.parse_ahead, "Find people's parts ahead of time");
    });
    hint(ui, t, crate::i18n::tr("Paused on battery below 40 % and in Low Power Mode."));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_pauses_the_work_also_in_a_window_without_the_keyboard() {
        // (the face scan calls a window without the keyboard "normal")
        assert_eq!(held_back(false, 0.5, "normal"), "pause");
        assert_eq!(held_back(true, 60.0, "normal"), "pause");
        assert_eq!(held_back(false, QUIET_SECS, "normal"), "normal");
        assert_eq!(held_back(false, 60.0, "light"), "light");
    }

    #[test]
    fn frames_are_asked_for_only_while_there_is_work_the_pace_allows() {
        assert_eq!(repaint_in("working", 3, 3, None), Some(BUSY_SECS));
        assert_eq!(repaint_in("loading", 0, 0, None), Some(BUSY_SECS));
        // held back (an export, a drag): now and then
        assert_eq!(repaint_in("paused", 3, 3, None), Some(IDLE_SECS));
        // only photos for an idle user wait: no frames, unless the pace will turn idle by itself
        assert_eq!(repaint_in("done", 0, 40, None), None);
        assert_eq!(repaint_in("done", 0, 40, Some(2.0)), Some(2.0));
        assert_eq!(repaint_in("done", 0, 0, Some(2.0)), None);
        assert_eq!(repaint_in("off", 0, 0, None), None);
    }
}
