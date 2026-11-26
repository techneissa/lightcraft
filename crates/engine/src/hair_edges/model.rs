//! The BiRefNet-matting model behind [`HairEdges`]: loading it (GPU, else the CPU), running it, unloading it when idle,
//! and its download.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use lightcraft_birefnet::{BiRefNet, Variant};

use super::{Attempt, HairEdges, Inner, Loaded, MODEL_BYTES, Matte, lock, on_devices};
use crate::segment::DownloadStatus;

/// What the user reads when the file is there but cannot be used.
fn damaged(e: impl std::fmt::Display) -> String {
    format!("{e}. If the file is damaged, delete model.safetensors from the birefnet-matting folder and download it again.")
}

impl HairEdges {
    pub(super) fn run_model(&self, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Matte, String> {
        // (a model measurement waits while this runs: issue #267)
        let _job = crate::model_units::shared().user_job();
        let stop = || cancel.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down();
        let dir = self.model_dir().ok_or("no folder is set for the BiRefNet Matting model")?;
        let file = dir.join(lightcraft_birefnet::WEIGHTS_FILE);
        // a short or overlong file (a download cut off, another model) is refused before it is read
        let len = std::fs::metadata(&file).map_err(|e| format!("BiRefNet Matting model file not found in {} ({e})", dir.display()))?.len();
        if len != MODEL_BYTES {
            return Err(damaged(format!("the BiRefNet Matting model file is {len} bytes, not {MODEL_BYTES}")));
        }
        let attempt = |cpu: bool| -> Attempt {
            if stop() {
                return Attempt::Fail("cancelled".into());
            }
            let runtime = self.runtime();
            let mut slot = lock(&self.inner.loaded);
            if slot.as_ref().is_none_or(|l| l.model.device().is_cpu() != cpu || l.runtime != runtime) {
                *slot = None;
                let t = web_time::Instant::now();
                // (on the GPU through the chosen engine, Core ML when its package is there: issue #264; the CPU is
                // candle's)
                let model = match cpu {
                    true => BiRefNet::load_cpu(&dir, Variant::Matting),
                    false => {
                        // (the units measured fastest on this Mac, issue #267)
                        let plan = crate::model_units::probes::plan(crate::model_units::probes::birefnet(&dir, true), runtime);
                        BiRefNet::load_in_order(&dir, Variant::Matting, plan.runtime(runtime), plan.coreml, lightcraft_birefnet::best_device())
                    }
                };
                let model = match model {
                    Ok(m) => m,
                    // (the GPU could not take it: the CPU next)
                    Err(lightcraft_birefnet::Error::Candle(e)) if !cpu => return Attempt::Gpu(e.to_string()),
                    Err(e @ lightcraft_birefnet::Error::CoreMl(_)) => return Attempt::Fail(e.to_string()),
                    Err(e) => return Attempt::Fail(damaged(e)),
                };
                log::info!(
                    "BiRefNet Matting loaded in {:?} ({}, {})",
                    t.elapsed(),
                    model.backend(),
                    if model.device().is_cpu() { "CPU" } else { "GPU" }
                );
                *slot = Some(Loaded { model, runtime });
                // (a model loaded again has made no mask's matte yet: issue #407)
                self.inner.for_masks.store(false, Ordering::SeqCst);
            }
            let Some(l) = slot.as_mut() else { return Attempt::Fail("the BiRefNet Matting model did not load".into()) };
            l.model.set_cancel(Some(cancel.clone()));
            let out = l.model.matte(rgb, w, h);
            l.model.set_cancel(None);
            // (Automatic hands a failed Core ML run to candle: the engine is read after the run)
            *lock(&self.inner.backend) = Some(l.model.backend());
            match out {
                Ok(m) => Attempt::Done(Matte { side: m.side, logits: m.logits }),
                Err(lightcraft_birefnet::Error::Cancelled) => Attempt::Fail("cancelled".into()),
                Err(lightcraft_birefnet::Error::Candle(e)) if !cpu => Attempt::Gpu(e.to_string()),
                Err(e @ lightcraft_birefnet::Error::CoreMl(_)) => Attempt::Fail(e.to_string()),
                Err(e) => Attempt::Fail(damaged(e)),
            }
        };
        let out = on_devices(&self.inner.gpu_refused, attempt);
        match &out {
            // (a model that failed, or panicked mid-run, is not kept)
            Err(_) => *lock(&self.inner.loaded) = None,
            Ok(_) => self.idle().touch(),
        }
        out
    }

    /// Keep the model loaded while `on` (the Masking panel is open in the front window, fork issue #353): it is not let
    /// go until this is turned off, and its idle time counts from then. Only a model a Subject or Background mask has
    /// used is kept (issue #407): one a people parse loaded goes after the idle time, as outside Masking.
    pub fn keep_loaded(&self, on: bool) {
        // (`holding` changes and is read under the `held` lock, so a matte made as the panel closes leaves no hold)
        let mut held = lock(&self.inner.held);
        self.inner.holding.store(on, Ordering::SeqCst);
        if !on {
            *held = None;
        } else if held.is_none() && self.inner.for_masks.load(Ordering::SeqCst) {
            *held = Some(self.idle().using());
        }
    }

    /// A Subject or Background mask's matte was made (issue #407): from now on the Masking panel's hold keeps the model.
    pub(super) fn used_for_masks(&self) {
        self.inner.for_masks.store(true, Ordering::SeqCst);
        let mut held = lock(&self.inner.held);
        if self.inner.holding.load(Ordering::SeqCst) && held.is_none() {
            *held = Some(self.idle().using());
        }
    }

    /// Whether the model is in memory (a run holding it counts as loaded).
    pub fn model_loaded(&self) -> bool {
        match self.inner.loaded.try_lock() {
            Ok(slot) => slot.is_some(),
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().is_some(),
            Err(std::sync::TryLockError::WouldBlock) => true,
        }
    }

    /// The shared idle rule's handle (`crate::idle`): the model is let go after a while without use. A run holds the
    /// `loaded` lock, so a model found locked is in use.
    fn idle(&self) -> &crate::idle::Tracker {
        self.inner.idle.get_or_init(|| {
            let weak: Weak<Inner> = Arc::downgrade(&self.inner);
            crate::idle::register("BiRefNet Matting", move || {
                use crate::idle::Unload;
                let Some(inner) = weak.upgrade() else { return Unload::Nothing };
                let old = match inner.loaded.try_lock() {
                    Ok(mut slot) => slot.take(),
                    Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().take(),
                    Err(std::sync::TryLockError::WouldBlock) => return Unload::Busy,
                };
                inner.for_masks.store(false, Ordering::SeqCst);
                if old.is_some() { Unload::Freed } else { Unload::Nothing }
            })
        })
    }

    /// The download's state.
    pub fn download_status(&self) -> DownloadStatus {
        let s = self.inner.download.status();
        DownloadStatus {
            running: s.running,
            done: s.done,
            total: s.total,
            file: s.file,
            error: s.error,
            cancelled: s.cancelled,
            finished: s.finished,
        }
    }

    /// Start downloading the model in the background (activity row, resume, size and SHA-256 checked). `Ok(false)`:
    /// it is installed or downloading already.
    pub fn start_download(&self, activity: &crate::activity::Activity) -> Result<bool, String> {
        let dir = self.model_dir().ok_or("no folder is set for the BiRefNet Matting model")?;
        if self.installed() {
            return Ok(false);
        }
        let env = std::env::var(lightcraft_birefnet::fetch::MIRRORS_ENV).ok();
        let mirrors = lightcraft_birefnet::fetch::mirrors(Variant::Matting, env.as_deref());
        self.start_download_from(activity, dir, mirrors)
    }

    /// [`HairEdges::start_download`] from `mirrors` (tests point it at a local server).
    pub(crate) fn start_download_from(
        &self,
        activity: &crate::activity::Activity,
        dir: std::path::PathBuf,
        mirrors: Vec<String>,
    ) -> Result<bool, String> {
        self.inner.download.start(
            Variant::Matting.files(),
            mirrors,
            dir,
            lightcraft_birefnet::fetch::Options::default(),
            activity,
            "BiRefNet Matting",
        )
    }

    /// Stop a running download (its partial file stays, to resume). False when none runs.
    pub fn cancel_download(&self) -> bool {
        self.inner.download.cancel()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::super::{Edge, EdgeKind, HairEdges, Matte, lock};

    #[test]
    fn the_pinned_size_is_the_variants() {
        assert_eq!(super::MODEL_BYTES, lightcraft_birefnet::Variant::Matting.size());
        assert_eq!(super::super::MODEL_DIR, lightcraft_birefnet::Variant::Matting.dir_name());
    }

    /// Issue #407: a people parse ahead of time loaded the model during a session of brush masks, and the Masking
    /// panel's hold kept it (about 3 GB) until the panel closed. Only a mask's matte makes the hold keep it.
    #[test]
    fn the_masking_hold_keeps_the_model_only_once_a_mask_used_it() {
        let edges = HairEdges::new(crate::activity::Activity::default());
        edges.set_runner(Some(Arc::new(|_, _, _, _| Ok(Matte { side: 2, logits: vec![1.0; 4] }))));
        let held = || lock(&edges.inner.held).is_some();
        let cancel = Arc::new(AtomicBool::new(false));
        let matte = |kind, shade: u8| edges.matte_for(Edge::Matting, kind, &[shade; 12], 2, 2, &cancel).unwrap();
        edges.keep_loaded(true);
        matte(EdgeKind::People, 1);
        assert!(!held(), "a people parse is not a mask");
        matte(EdgeKind::Subject, 2);
        assert!(held(), "a Subject mask's matte is kept while masking");
        edges.keep_loaded(false);
        assert!(!held());
        // (back in Masking with the model a mask used still loaded: kept again, as before #407)
        edges.keep_loaded(true);
        assert!(held());
        edges.keep_loaded(false);
    }
}
