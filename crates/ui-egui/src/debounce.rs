//! "Wait until it stops changing": work that follows a value being typed (the Rename Photos
//! preview plans files on disk) starts once the value has rested, not on every keystroke.

use std::time::Duration;

/// How long Rename Photos' template rests before its preview is planned again.
pub(crate) const PREVIEW_REST: f64 = 0.25;

/// Whether `key` has been the same for `delay` seconds as of `now` (egui time). Remembered in
/// `ctx` under `slot`; while it has not rested, a repaint is asked for at the moment it will have.
pub(crate) fn rested(ctx: &egui::Context, slot: &str, key: u64, now: f64, delay: f64) -> bool {
    let id = egui::Id::unique(("debounce", slot));
    let since = match ctx.data(|d| d.get_temp::<(u64, f64)>(id)) {
        Some((k, t)) if k == key => t,
        _ => {
            ctx.data_mut(|d| d.insert_temp(id, (key, now)));
            now
        }
    };
    let left = since + delay - now;
    if left > 0.0 {
        ctx.request_repaint_after(Duration::from_secs_f64(left));
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Given a value that changes on every keystroke, when it is asked about, then it has not rested until
    /// it stayed the same for the delay.
    #[test]
    fn a_value_rests_when_it_stops_changing() {
        let ctx = egui::Context::default();
        assert!(!rested(&ctx, "t", 1, 0.00, 0.25), "first seen");
        assert!(!rested(&ctx, "t", 2, 0.10, 0.25), "changed: the clock starts again");
        assert!(!rested(&ctx, "t", 3, 0.20, 0.25));
        assert!(!rested(&ctx, "t", 3, 0.40, 0.25), "0.20 s since it last changed");
        assert!(rested(&ctx, "t", 3, 0.46, 0.25), "0.26 s");
        assert!(rested(&ctx, "t", 3, 5.0, 0.25), "stays rested");
        assert!(!rested(&ctx, "t", 4, 5.1, 0.25), "a new change waits again");
        assert!(!rested(&ctx, "other", 3, 5.1, 0.25), "each slot has its own clock");
    }
}
