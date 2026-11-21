//! Fork (issue #353, `fork/mask-models-kept.md`): the AI mask models stay loaded while the Masking panel is open in the
//! front window. Since #304 the worker lets its models go a minute after its last request, and on Core ML loading them
//! again and planning each part on the GPU at its first run costs a mask about 13 s on a test Mac (a mask made right
//! after another: 4 s). So a user making masks with a pause between them waited for the whole load every time; while
//! the panel is open the models are kept, and the minute starts when it closes.

use std::time::Duration;

use web_time::Instant;

/// Whether the session keeps the models, and since when (the panel opened or closed).
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub on: bool,
    pub since: Instant,
}

impl Default for Hold {
    fn default() -> Self {
        Hold { on: false, since: Instant::now() }
    }
}

/// When the loaded models go: `idle` after the later of the last request and the end of the hold; never while held.
pub(super) fn unload_at(last: Instant, hold: Hold, idle: Duration) -> Option<Instant> {
    (!hold.on).then(|| last.max(hold.since) + idle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cause of #353: a mask made more than a minute after the last one found every model unloaded while the user
    /// was still masking.
    #[test]
    fn held_models_are_never_due_and_the_idle_time_starts_when_the_hold_ends() {
        let idle = Duration::from_secs(60);
        let last = Instant::now();
        let later = |s: u64| last + Duration::from_secs(s);
        // nothing holds them: a minute after the last request
        assert_eq!(unload_at(last, Hold { on: false, since: last }, idle), Some(later(60)));
        // the Masking panel is open: never, however long ago the last request was
        assert_eq!(unload_at(last, Hold { on: true, since: later(1) }, idle), None);
        // closed ten minutes later: a minute after it closed, not after the last request
        assert_eq!(unload_at(last, Hold { on: false, since: later(600) }, idle), Some(later(660)));
        // a request after the panel closed counts from that request
        assert_eq!(unload_at(later(700), Hold { on: false, since: later(600) }, idle), Some(later(760)));
    }
}
