//! Fork feature (`fork/parse-ahead.md`): keeps the engine's people parsing ahead of time going
//! (`parsing.ahead.pump`) and its switch in Settings ▸ Performance.
//!
//! The pace is the face scan's ([`crate::panels::faces::background_pace`]), held back further: nothing new while a
//! slider is dragged, or for [`QUIET_SECS`] after the user last dragged, typed or scrolled (edits come in bursts). The
//! engine also waits while the loupe waits for its render.

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

fn next_id() -> egui::Id {
    egui::Id::unique("parse-ahead-next-pump")
}

/// The pace for parsing ahead now.
fn pace(app: &LightcraftApp, ctx: &egui::Context, now: f64) -> &'static str {
    let focused = ctx.input(|i| i.raw.focused);
    if app.ui.dragging_control.is_some() || (focused && now - app.caches.last_input < QUIET_SECS) {
        return "pause";
    }
    crate::panels::faces::background_pace(app, ctx, now)
}

/// Called every frame; asks the engine at most every [`BUSY_SECS`] while it has work and every [`IDLE_SECS`] when not.
pub fn pump(app: &mut LightcraftApp, ctx: &egui::Context, now: f64) {
    let pace = pace(app, ctx, now);
    let due = ctx.data(|d| d.get_temp::<f64>(next_id())).unwrap_or(0.0);
    // (a pause is passed on at once: it stops a model run going)
    if now < due && pace != "pause" {
        return;
    }
    let p = json!({"pace": pace, "backward": app.look_ahead.backward(), "enabled": app.ui.settings.parse_ahead});
    let (working, waiting) = match app.session.execute("parsing.ahead.pump", &p) {
        Ok(v) => (matches!(v["state"].as_str(), Some("working" | "paused")), v["active"] == true),
        Err(e) => {
            log::warn!("parse ahead: {e}");
            (false, false)
        }
    };
    let wait = if working { BUSY_SECS } else { IDLE_SECS };
    ctx.data_mut(|d| d.insert_temp(next_id(), now + wait));
    // (a quiet window draws no frames: ask for one, so the work goes on, and photos left for when the user is idle
    // start once they are)
    if working || waiting {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
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
