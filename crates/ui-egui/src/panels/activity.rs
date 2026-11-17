//! What the activity registry shows besides the ring (issues #345, #295): every long-running task is listed by the ring
//! in the top bar (`activity_ring`), so nothing is drawn in a corner. This module words the pieces the ring and the
//! messages share (the count, the time left, what a batch did) and runs ✕ as `activity.cancel`, like any other
//! frontend would.
//!
//! A batch (AI Denoise or an export over many photos) is one task: "37 of 200 · about 6 min left · 2 failed". Its
//! per-photo tasks are not listed, and when it ends its summary shows once as a message ("AI Denoise: 198 done, 2
//! failed: why").

use lightcraft_engine::activity::{BatchSummary, TaskInfo, Unit};
use serde_json::json;

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};

/// A task shows in the ring once it has run this long, so quick ones don't flash.
pub const SHOW_AFTER_MS: u64 = 500;

/// "about 6 min left": the time a batch has left, roughly.
pub fn eta_text(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        tr("less than a minute left").to_string()
    } else if secs < 90 * 60 {
        tr_format!("about {n} min left", n = secs.div_ceil(60))
    } else {
        let mins = secs.div_ceil(60);
        tr_format!("about {h} h {m} min left", h = mins / 60, m = mins % 60)
    }
}

/// What a batch did, for the message when it ends: "AI Denoise: 198 done, 2 failed: IMG_2.CR3: …".
pub fn summary_text(s: &BatchSummary) -> String {
    let mut m = format!("{}: {}", tr(&s.label), tr_format!("{n} done", n = s.done.saturating_sub(s.failed)));
    if s.failed > 0 {
        m += &tr_format!(", {n} failed", n = s.failed);
        if let Some(why) = s.reasons.first() {
            m += &format!(": {why}");
        }
    }
    if s.cancelled {
        m += &format!(" · {}", tr("cancelled"));
    }
    m
}

/// "3 of 25", "12 of 340 MB", "40 %"; empty while the amount of work isn't known.
pub fn count_text(t: &TaskInfo) -> String {
    if t.total == 0 {
        return String::new();
    }
    match t.unit {
        Unit::Count => tr_format!("{done} of {total}", done = t.done, total = t.total),
        Unit::Bytes => tr_format!("{done} of {total} MB", done = megabytes(t.done), total = megabytes(t.total)),
        Unit::Percent => tr_format!("{pct} %", pct = t.done.saturating_mul(100) / t.total),
    }
}

fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb < 10.0 { format!("{mb:.1}") } else { format!("{:.0}", mb.floor()) }
}

/// Called every frame: say what a batch that ended did, and look again soon while anything runs (a young task becomes
/// due for the ring, counts move).
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    announce_finished(app, ctx);
    if !app.session.activity.list().is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Say once what each batch of several photos that ended did ("AI Denoise: 198 done, 2 failed: why"); a failure
/// stays longer. A single photo's says nothing here: its own messages and the photo show how it went.
fn announce_finished(app: &mut LightcraftApp, ctx: &egui::Context) {
    let finished = app.session.activity.finished();
    let Some(last) = finished.last().map(|s| s.seq) else { return };
    if last <= app.activity_seen_seq {
        return;
    }
    let seen = app.activity_seen_seq;
    app.activity_seen_seq = last;
    let new: Vec<&BatchSummary> = finished.iter().filter(|s| s.seq > seen && !s.reported && s.done >= 2).collect();
    // several at once (rare): the one with failures, else the last
    if let Some(s) = new.iter().find(|s| s.failed > 0).or(new.last()) {
        let text = summary_text(s);
        if s.failed > 0 {
            app.toast_for(ctx, text, 15.0);
        } else {
            app.toast(ctx, text);
        }
    }
}

/// ✕ on task `id`'s row. The task may have just finished or stopped being cancellable: nothing to tell the user then,
/// so the command goes to the engine directly (`LightcraftApp::run` would put its error in the status bar).
pub(crate) fn cancel(app: &mut LightcraftApp, id: u64) {
    let _ = app.session.execute("activity.cancel", &json!({"id": id}));
}
