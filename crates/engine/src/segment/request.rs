//! A "describe" request that something other than an AI mask waits for (Find People): the same worker, model and
//! encoded-photo cache as Describe masks, but the caller keeps the reply channel and waits for it on its own
//! thread, checking a cancel flag, so the session thread never blocks on the model.

use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::SegMask;

use crate::Session;

/// A request in flight.
pub(crate) struct TextRequest {
    #[cfg(feature = "sam")]
    rx: std::sync::mpsc::Receiver<super::worker::Outcome>,
}

impl TextRequest {
    /// Wait for the mask (`Ok(None)`: nothing matched), up to [`super::WAIT`], giving up when `cancel` is set.
    pub(crate) fn wait(self, cancel: &AtomicBool) -> Result<Option<SegMask>, String> {
        #[cfg(feature = "sam")]
        {
            use std::sync::mpsc::RecvTimeoutError;
            let start = std::time::Instant::now();
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                match self.rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(o) => {
                        if let Some(n) = o.notice {
                            log::warn!("{n}");
                        }
                        return o.result;
                    }
                    Err(RecvTimeoutError::Timeout) if start.elapsed() < super::WAIT => {}
                    Err(RecvTimeoutError::Timeout) => return Err("the AI model took too long; try again".into()),
                    Err(RecvTimeoutError::Disconnected) => return Err("the AI mask worker stopped".into()),
                }
            }
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = cancel.load(Ordering::SeqCst);
            Err("AI masks are not available in this build".into())
        }
    }
}

impl Session {
    /// Queue "segment everything `text` describes" on photo `id` and return at once. Fails when the model is not
    /// installed ([`crate::segment::Segmenter::require_model`] says how to get it).
    pub(crate) fn segment_text_request(&mut self, id: PhotoId, text: &str) -> Result<TextRequest, String> {
        self.segmenter.model_dir()?;
        #[cfg(feature = "sam")]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let job = self.segment_job(id, super::worker::Kind::Text(text.trim().to_string()), super::Tag::Wait, tx, None)?;
            self.segmenter.worker.submit(job)?;
            Ok(TextRequest { rx })
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, text);
            Err("AI masks are not available in this build".into())
        }
    }
}
