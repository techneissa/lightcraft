//! "Wait until it stops changing": work that follows a value being typed (the Rename Photos preview plans files
//! on disk) starts once the value has rested, not on every keystroke. The first time a slot is asked about, and
//! again after it was not asked about for a while (its window was closed and opened again), it is immediate.

use std::time::Duration;

/// How long Rename Photos' template rests before its preview is planned again.
pub(crate) const PREVIEW_REST: f64 = 0.25;

/// How long a slot may go unasked before the next question counts as a first one (seconds).
const IDLE: f64 = 1.0;

/// Whether `key` may be acted on as of `now` (egui time): it is the first question about `slot`, or the slot was
/// idle, or `key` has been the same for `delay` seconds. Remembered in `ctx` under `slot`; while it has not rested,
/// a repaint is asked for at the moment it will have.
pub(crate) fn rested(ctx: &egui::Context, slot: &str, key: u64, now: f64, delay: f64) -> bool {
    let id = egui::Id::unique(("debounce", slot));
    let (since, fresh) = match ctx.data(|d| d.get_temp::<(u64, f64, f64)>(id)) {
        Some((k, since, last)) if now - last <= IDLE => (if k == key { since } else { now }, false),
        _ => (now, true),
    };
    ctx.data_mut(|d| d.insert_temp(id, (key, since, now)));
    let left = since + delay - now;
    if fresh || left <= 0.0 {
        return true;
    }
    ctx.request_repaint_after(Duration::from_secs_f64(left));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Given a value that changes on every keystroke, when it is asked about, then it has not rested until
    /// it stayed the same for the delay; the very first question, and the first after a pause, are immediate.
    #[test]
    fn a_value_rests_when_it_stops_changing() {
        let ctx = egui::Context::default();
        assert!(rested(&ctx, "t", 1, 0.00, 0.25), "the first question is immediate");
        assert!(!rested(&ctx, "t", 2, 0.10, 0.25), "changed: the clock starts again");
        assert!(!rested(&ctx, "t", 3, 0.20, 0.25));
        assert!(!rested(&ctx, "t", 3, 0.40, 0.25), "0.20 s since it last changed");
        assert!(rested(&ctx, "t", 3, 0.46, 0.25), "0.26 s");
        assert!(rested(&ctx, "t", 3, 0.80, 0.25), "stays rested");
        assert!(!rested(&ctx, "t", 4, 0.90, 0.25), "a new change waits again");
        assert!(rested(&ctx, "other", 3, 0.90, 0.25), "each slot has its own clock");
        assert!(rested(&ctx, "t", 5, 5.0, 0.25), "after a pause (the window was closed) the first question is immediate");
    }
}
