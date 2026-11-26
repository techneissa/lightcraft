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
        if self.inner.release.swap(false, Ordering::SeqCst) {
            // (a mask's stop button asked for it during the run: fork, issue #407)
            *lock(&self.inner.loaded) = None;
            log::info!("BiRefNet Matting unloaded");
            return out;
        }
        match &out {
            // (a model that failed, or panicked mid-run, is not kept)
            Err(_) => *lock(&self.inner.loaded) = None,
            Ok(_) => self.idle().touch(),
        }
        out
    }

    /// Keep the model loaded while `on` (the Masking panel is open in the front window, fork issue #353): it is not let
    /// go until this is turned off, and its idle time counts from then.
    pub fn keep_loaded(&self, on: bool) {
        let mut held = lock(&self.inner.held);
        if on != held.is_some() {
            *held = on.then(|| self.idle().using());
        }
    }

    /// Let the model go now, held or not (fork, issue #407: an AI mask's stop button); a run going lets it go when it
    /// ends.
    pub fn release(&self) {
        match self.inner.loaded.try_lock() {
            Ok(mut slot) => {
                if slot.take().is_some() {
                    log::info!("BiRefNet Matting unloaded");
                }
            }
            Err(std::sync::TryLockError::Poisoned(p)) => *p.into_inner() = None,
            Err(std::sync::TryLockError::WouldBlock) => self.inner.release.store(true, Ordering::SeqCst),
        }
    }

    /// Whether the Masking panel's hold keeps the model (tests).
    #[cfg(test)]
    pub(crate) fn is_held(&self) -> bool {
        lock(&self.inner.held).is_some()
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
    #[test]
    fn the_pinned_size_is_the_variants() {
        assert_eq!(super::MODEL_BYTES, lightcraft_birefnet::Variant::Matting.size());
        assert_eq!(super::super::MODEL_DIR, lightcraft_birefnet::Variant::Matting.dir_name());
    }
}
