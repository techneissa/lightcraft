//! Fork feature (`fork/object-remove.md`): Object clicks that a Remove spot waits for. The same worker, model and
//! encoded-photo cache as Object masks, but the caller keeps the reply channel and checks it when the app polls
//! ([`crate::Session::segment_poll`]), so the session thread never blocks on the model.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::SegMask;

use super::Click;
use crate::Session;

/// Clicks on their way to the model.
pub(crate) struct ClicksRequest {
    /// Set to stop the request (the worker gives up on it).
    pub(crate) cancel: Arc<AtomicBool>,
    #[cfg(feature = "sam")]
    rx: std::sync::mpsc::Receiver<super::worker::Outcome>,
}

impl ClicksRequest {
    /// The selection once the model has answered (`Ok(None)`: nothing was selected); `None` while it works.
    pub(crate) fn try_take(&self) -> Option<Result<Option<SegMask>, String>> {
        #[cfg(feature = "sam")]
        {
            use std::sync::mpsc::TryRecvError;
            match self.rx.try_recv() {
                Ok(o) => {
                    if let Some(n) = o.notice {
                        log::warn!("{n}");
                    }
                    Some(o.result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("the AI mask worker stopped".into())),
            }
        }
        #[cfg(not(feature = "sam"))]
        {
            Some(Err("AI masks are not available in this build".into()))
        }
    }
}

impl Session {
    /// Queue Object `clicks` (photo coordinates) on photo `id` and return at once. Fails when the model is not
    /// installed ([`crate::segment::Segmenter::require_model`] says how to get it).
    pub(crate) fn segment_clicks_request(&mut self, id: PhotoId, clicks: &[Click]) -> Result<ClicksRequest, String> {
        self.segmenter.model_dir()?;
        if !clicks.iter().any(|c| c.include) {
            return Err("click the object to include it first".into());
        }
        #[cfg(feature = "sam")]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let job = self.segment_job(id, super::worker::Kind::Clicks(Self::model_clicks(clicks)), super::Tag::Wait, tx, None)?;
            let cancel = job.cancel.clone();
            self.segmenter.worker.submit(job)?;
            Ok(ClicksRequest { cancel, rx })
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = id;
            Err("AI masks are not available in this build".into())
        }
    }
}
