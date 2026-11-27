//! Fork (issues #353, #407 and #452, `fork/mask-models-kept.md`): the worker lets SAM 3 and BiRefNet go `idle` after
//! the last request, whatever panel is open. On Core ML the two models hold gigabytes of GPU memory between runs (a test
//! Mac rested at 7.3 GB with the Masking panel open and a Subject mask made, 1.6 GB without them), and loading them again
//! costs the next mask about 13 s, so that is what a pause longer than `idle` buys. An AI mask's stop button lets its
//! models go at once.

use std::time::Duration;

use web_time::Instant;

use super::{Job, Kind, Shared, State, Tag};
use crate::segment::MaskModels;

/// Which of SAM 3 (or a remote worker) and BiRefNet `state` holds.
pub(super) fn loaded(state: &State) -> MaskModels {
    MaskModels { sam3: state.model.is_some() || state.remote.is_some(), birefnet: state.birefnet.is_some(), matting: false }
}

/// When the loaded models go: `idle` after the last request; `None` when none is loaded.
pub(super) fn unload_at(last: Instant, idle: Duration, loaded: MaskModels) -> Option<Instant> {
    loaded.any().then(|| last + idle)
}

/// Let `models`' SAM 3 (with its encoded photo and warm-up) and BiRefNet go.
pub(super) fn let_go(state: &mut State, shared: &Shared, models: MaskModels) {
    let had = loaded(state);
    if models.sam3 && had.sam3 {
        shared.warm.pause();
        (state.model, state.warm, state.cache, state.remote) = (None, None, None, None);
        log::info!("SAM 3 unloaded");
    }
    if models.birefnet && had.birefnet {
        state.birefnet = None;
        log::info!("BiRefNet unloaded");
    }
    state.publish(shared);
}

/// The message that makes the worker let `models` go after the requests queued before it.
pub(super) fn release_job(models: MaskModels) -> Job {
    let (reply, _) = std::sync::mpsc::channel();
    Job {
        id: 0,
        cancel: Default::default(),
        dir: Default::default(),
        key: 0,
        render: None,
        missing: None,
        kind: Kind::Release(models),
        tag: Tag::Wait,
        reply,
        remote: None,
        refine: None,
        plain: None,
        detail: Vec::new(),
        runtime: Default::default(),
        activity: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: MaskModels = MaskModels { sam3: true, birefnet: true, matting: false };

    /// Issue #452: the models go `idle` after the last request however long the Masking panel stays open, and the next
    /// request loads them again and starts a new idle time. The worker's loop, with the clock passed in and the loaded
    /// models a plain value.
    #[test]
    fn models_go_after_the_idle_time_and_the_next_request_loads_them_again() {
        let idle = Duration::from_secs(60);
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        // what the worker does at `now`: lets the models go when they are due
        let tick = |last: Instant, loaded: &mut MaskModels, now: Instant| {
            if unload_at(last, idle, *loaded).is_some_and(|d| now >= d) {
                *loaded = MaskModels::NONE;
            }
        };
        let mut loaded = BOTH;
        let last = t0; // the last request ended at t0
        // (a photo with AI masks, the panel open: the models are not due before the idle time)
        tick(last, &mut loaded, at(59));
        assert_eq!(loaded, BOTH);
        tick(last, &mut loaded, at(61));
        assert_eq!(loaded, MaskModels::NONE, "released with no request, though the panel is open");
        assert_eq!(unload_at(last, idle, loaded), None, "nothing loaded: the worker sleeps until a request");
        // the next request, long after: loads them, and they stay for another idle time counted from its end
        loaded = BOTH;
        let last = at(500);
        assert!(unload_at(last, idle, loaded).is_some_and(|d| d > at(500)));
        tick(last, &mut loaded, at(559));
        assert_eq!(loaded, BOTH);
        tick(last, &mut loaded, at(561));
        assert_eq!(loaded, MaskModels::NONE);
    }
}
