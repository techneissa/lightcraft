//! Finding an AI mask part again on another photo (fork, `fork/adaptive-masks.md`): what the model is asked for a
//! Subject, Sky, Background, Object or Describe part, and sending it to the worker, waiting or not. The queue that
//! asks is `crate::adaptive`.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{MaskShape, SegMask};

use super::{MAX_CLICKS, clicks_of};
use crate::Session;

/// What the model is asked (opaque outside the worker's module).
pub(crate) struct Request {
    #[cfg(feature = "sam")]
    kind: super::worker::Kind,
}

/// How an AI part is found on a photo.
pub(crate) enum Ask {
    /// By the model (`invert`: Background, the subject turned around).
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    Model { request: Request, invert: bool },
    /// The classical way: no model for it on this computer, or the photo's own matte for it (Subject, Sky and
    /// Background only, as when such a part is added).
    Classical,
    /// It can't be found on this computer: why.
    Missing(String),
}

/// Replies of the model requests sent with [`Session::adaptive_submit`].
pub(crate) struct Replies {
    #[cfg(feature = "sam")]
    channel: (std::sync::mpsc::Sender<super::worker::Outcome>, std::sync::mpsc::Receiver<super::worker::Outcome>),
}

// (derivable only without the `sam` feature: the channel has no Default)
#[allow(clippy::derivable_impls)]
impl Default for Replies {
    fn default() -> Self {
        Replies {
            #[cfg(feature = "sam")]
            channel: std::sync::mpsc::channel(),
        }
    }
}

impl Replies {
    /// The replies that came in: each request's id and what it found.
    pub(crate) fn drain(&self) -> Vec<(u64, Result<Option<SegMask>, String>)> {
        #[cfg(feature = "sam")]
        {
            self.channel.1.try_iter().map(|o| (o.id, if o.superseded { Err("the selection was stopped".to_string()) } else { o.result })).collect()
        }
        #[cfg(not(feature = "sam"))]
        Vec::new()
    }
}

impl Session {
    /// How `shape` (an AI part other than Select People) is found on photo `id`.
    pub(crate) fn adaptive_ask(&self, id: PhotoId, shape: &MaskShape) -> Ask {
        match shape {
            MaskShape::Subject { .. } | MaskShape::Sky { .. } | MaskShape::Background { .. } => {
                let Some(c) = crate::cmd::concepts::of(shape) else { return Ask::Classical };
                if !crate::cmd::concepts::wanted(self, id, shape) {
                    return Ask::Classical;
                }
                #[cfg(feature = "sam")]
                {
                    match self.concept_kind(&c) {
                        Ok((kind, _)) => Ask::Model { request: Request { kind }, invert: c.invert },
                        Err(e) => Ask::Missing(e),
                    }
                }
                #[cfg(not(feature = "sam"))]
                {
                    let _ = c;
                    Ask::Classical
                }
            }
            MaskShape::Object { hint, exclude, .. } => {
                if hint.is_empty() || hint.len() + exclude.len() > MAX_CLICKS {
                    return Ask::Missing("the object has no clicks to find it by".into());
                }
                if let Err(e) = self.segmenter.model_dir() {
                    return Ask::Missing(e);
                }
                #[cfg(feature = "sam")]
                {
                    let kind = super::worker::Kind::Clicks(Self::model_clicks(&clicks_of(hint, exclude)));
                    Ask::Model { request: Request { kind }, invert: false }
                }
                #[cfg(not(feature = "sam"))]
                {
                    let _ = clicks_of(hint, exclude);
                    Ask::Missing("AI masks are not available in this build".into())
                }
            }
            MaskShape::Prompt { text, .. } => {
                let text = text.trim();
                if text.is_empty() {
                    return Ask::Missing("the description is empty".into());
                }
                if let Err(e) = self.segmenter.model_dir() {
                    return Ask::Missing(e);
                }
                #[cfg(feature = "sam")]
                {
                    Ask::Model { request: Request { kind: super::worker::Kind::Text(text.to_string()) }, invert: false }
                }
                #[cfg(not(feature = "sam"))]
                Ask::Missing("AI masks are not available in this build".into())
            }
            _ => Ask::Missing("not an AI mask part".into()),
        }
    }

    /// Ask the model and wait for its answer (CLI, MCP, tests).
    pub(crate) fn adaptive_wait(&mut self, id: PhotoId, request: Request) -> Result<Option<SegMask>, String> {
        #[cfg(feature = "sam")]
        {
            self.segment_wait(id, request.kind, None)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, request);
            Err("AI masks are not available in this build".into())
        }
    }

    /// Send a request to the model's worker; its answer comes to `replies` under the returned id. The flag stops it.
    pub(crate) fn adaptive_submit(
        &mut self,
        id: PhotoId,
        request: Request,
        replies: &Replies,
    ) -> Result<(u64, std::sync::Arc<std::sync::atomic::AtomicBool>), String> {
        #[cfg(feature = "sam")]
        {
            let job = self.segment_job(id, request.kind, super::Tag::Wait, replies.channel.0.clone(), None)?;
            let out = (job.id, job.cancel.clone());
            self.segmenter.worker.submit(job)?;
            Ok(out)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, request, replies);
            Err("AI masks are not available in this build".into())
        }
    }
}
