//! Fork (issues #353 and #407, `fork/mask-models-kept.md`): the AI mask models stay loaded while the Masking panel is
//! open in the front window and the open photo has an AI mask that uses them. Since #304 the worker lets its models go
//! a minute after its last request, and on Core ML loading them again and planning each part on the GPU at its first
//! run costs a mask about 13 s on a test Mac (a mask made right after another: 4 s). So the models such masks use are
//! kept, and the minute starts when the hold ends. A model no mask on the photo uses is not kept (issue #407: it held
//! gigabytes through sessions of brush masks), and an AI mask's stop button lets its models go at once.

use std::time::Duration;

use web_time::Instant;

use super::{Job, Kind, Shared, State, Tag};
use crate::segment::MaskModels;

/// Which of the worker's models the session keeps, and since when (the panel opened or closed, or the photo's AI
/// masks changed).
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    /// SAM 3 and BiRefNet (`matting` is never set here: BiRefNet Matting is held by `HairEdges`).
    pub models: MaskModels,
    pub since: Instant,
}

impl Default for Hold {
    fn default() -> Self {
        Hold { models: MaskModels::NONE, since: Instant::now() }
    }
}

/// The part of `models` the worker holds (SAM 3 and BiRefNet).
pub(super) fn worker_part(models: MaskModels) -> MaskModels {
    MaskModels { matting: false, ..models }
}

/// Which of SAM 3 (or a remote worker) and BiRefNet `state` holds.
pub(super) fn loaded(state: &State) -> MaskModels {
    MaskModels { sam3: state.model.is_some() || state.remote.is_some(), birefnet: state.birefnet.is_some(), matting: false }
}

/// When the loaded models the hold does not keep go: `idle` after the later of the last request and the hold's last
/// change; `None` when every loaded model is held (or none is loaded).
pub(super) fn unload_at(last: Instant, hold: Hold, idle: Duration, loaded: MaskModels) -> Option<Instant> {
    let unheld = (loaded.sam3 && !hold.models.sam3) || (loaded.birefnet && !hold.models.birefnet);
    unheld.then(|| last.max(hold.since) + idle)
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

    /// The cause of #353: a mask made more than a minute after the last one found every model unloaded while the user
    /// was still masking.
    #[test]
    fn held_models_are_never_due_and_the_idle_time_starts_when_the_hold_ends() {
        let idle = Duration::from_secs(60);
        let last = Instant::now();
        let later = |s: u64| last + Duration::from_secs(s);
        // nothing holds them: a minute after the last request
        assert_eq!(unload_at(last, Hold { models: MaskModels::NONE, since: last }, idle, BOTH), Some(later(60)));
        // the open photo's AI masks use both: never, however long ago the last request was
        assert_eq!(unload_at(last, Hold { models: BOTH, since: later(1) }, idle, BOTH), None);
        // let go ten minutes later: a minute after that, not after the last request
        assert_eq!(unload_at(last, Hold { models: MaskModels::NONE, since: later(600) }, idle, BOTH), Some(later(660)));
        // a request after the hold ended counts from that request
        assert_eq!(unload_at(later(700), Hold { models: MaskModels::NONE, since: later(600) }, idle, BOTH), Some(later(760)));
    }

    /// Issue #407: a Sky mask (SAM 3 only) does not keep a BiRefNet a Subject mask on another photo loaded.
    #[test]
    fn a_model_no_mask_uses_goes_while_the_others_are_held() {
        let idle = Duration::from_secs(60);
        let last = Instant::now();
        let sky = MaskModels { sam3: true, ..MaskModels::NONE };
        assert_eq!(unload_at(last, Hold { models: sky, since: last }, idle, BOTH), Some(last + idle));
        assert_eq!(unload_at(last, Hold { models: sky, since: last }, idle, sky), None);
    }
}
