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
    /// The part of the photo the model sees (a Describe limited to one, issue #281: rendered zoomed in); `None`, all of it.
    #[cfg(feature = "sam")]
    region: Option<[f64; 4]>,
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
    /// The replies that came in: each request's id, what it found and the finer grids over its edge (fork, issue #240).
    pub(crate) fn drain(&self) -> Vec<(u64, Result<Option<SegMask>, String>, Vec<SegMask>)> {
        #[cfg(feature = "sam")]
        {
            self.channel
                .1
                .try_iter()
                .map(|o| (o.id, if o.superseded { Err("the selection was stopped".to_string()) } else { o.result }, o.detail))
                .collect()
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
                        Ok((kind, _)) => Ask::Model { request: Request { kind, region: None }, invert: c.invert },
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
                    Ask::Model { request: Request { kind, region: None }, invert: false }
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
                    // (a Describe limited to a part of the photo looks there, zoomed in, as when it was made: issue #281)
                    let (kind, region) = super::text_kind(text, shape.region());
                    Ask::Model { request: Request { kind, region }, invert: false }
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
            self.segment_wait(id, request.kind, request.region)
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
            let job = self.segment_job(id, request.kind, super::Tag::Wait, replies.channel.0.clone(), request.region)?;
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

#[cfg(all(test, feature = "sam"))]
mod tests {
    use super::*;

    /// Issue #281: Update AI Masks asks a Describe limited to a part of the photo there, zoomed in, as the Describe
    /// field did (not on the whole photo, clipped afterwards). No model runs: only the request is looked at.
    #[test]
    fn a_describe_with_a_region_is_asked_zoomed_in_there() {
        let mut s = Session::with_demo();
        let dir = std::env::temp_dir().join(format!("lc-test-sam-adaptive-region-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"\x10\x00\x00\x00\x00\x00\x00\x00{}").unwrap();
        std::fs::write(dir.join("vocab.json"), b"{}").unwrap();
        std::fs::write(dir.join("merges.txt"), b"#version: 0.2\n").unwrap();
        s.segmenter.dir = Some(dir.clone());
        let id = s.active().unwrap();
        let area = [0.1, 0.2, 0.3, 0.4];
        let shape = MaskShape::Prompt { text: "eye".into(), seg: None, detail: Vec::new(), edge: 0.0, region: Some(area) };
        let Ask::Model { request, .. } = s.adaptive_ask(id, &shape) else { panic!("asked of the model") };
        assert_eq!(request.region, Some(area));
        assert!(matches!(request.kind, super::super::worker::Kind::Detail { region, .. } if region == area));
        let whole = MaskShape::prompt("eye");
        let Ask::Model { request, .. } = s.adaptive_ask(id, &whole) else { panic!("asked of the model") };
        assert!(request.region.is_none() && matches!(request.kind, super::super::worker::Kind::Text(_)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
