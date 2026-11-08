//! The BiRefNet-matting model behind [`HairEdges`]: loading it (GPU, else the CPU), running it, unloading it when idle,
//! and its download.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use lightcraft_birefnet::{BiRefNet, Variant};

use super::{HairEdges, Inner, KEEP_LOADED, Loaded, MODEL_BYTES, Matte, lock};
use crate::segment::DownloadStatus;

/// What the user reads when the file is there but cannot be used.
fn damaged(e: impl std::fmt::Display) -> String {
    format!("{e}. If the file is damaged, delete model.safetensors from the birefnet-matting folder and download it again.")
}

impl HairEdges {
    pub(super) fn run_model(&self, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Matte, String> {
        let stop = || cancel.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down();
        let dir = self.model_dir().ok_or("no folder is set for the BiRefNet Matting model")?;
        let file = dir.join(lightcraft_birefnet::WEIGHTS_FILE);
        // a short or overlong file (a download cut off, another model) is refused before it is read
        let len = std::fs::metadata(&file).map_err(|e| format!("BiRefNet Matting model file not found in {} ({e})", dir.display()))?.len();
        if len != MODEL_BYTES {
            return Err(damaged(format!("the BiRefNet Matting model file is {len} bytes, not {MODEL_BYTES}")));
        }
        let mut slot = lock(&self.inner.loaded);
        for cpu in [false, true] {
            if stop() {
                return Err("cancelled".into());
            }
            if slot.as_ref().is_none_or(|l| l.model.device().is_cpu() != cpu) {
                *slot = None;
                let t = web_time::Instant::now();
                let model = if cpu { BiRefNet::load_cpu(&dir, Variant::Matting) } else { BiRefNet::load(&dir, Variant::Matting) };
                let model = match model {
                    Ok(m) => m,
                    // (the GPU could not take it: the CPU next)
                    Err(lightcraft_birefnet::Error::Candle(e)) if !cpu => {
                        log::warn!("BiRefNet Matting did not load on the GPU ({e}); trying the CPU");
                        continue;
                    }
                    Err(e) => return Err(damaged(e)),
                };
                log::info!("BiRefNet Matting loaded in {:?} ({})", t.elapsed(), if model.device().is_cpu() { "CPU" } else { "GPU" });
                *slot = Some(Loaded { model, last_used: std::time::Instant::now() });
            }
            let Some(l) = slot.as_mut() else { continue };
            l.last_used = std::time::Instant::now();
            l.model.set_cancel(Some(cancel.clone()));
            match l.model.matte(rgb, w, h) {
                Ok(m) => {
                    l.model.set_cancel(None);
                    drop(slot);
                    self.watch();
                    return Ok(Matte { side: m.side, logits: m.logits });
                }
                Err(lightcraft_birefnet::Error::Cancelled) => return Err("cancelled".into()),
                Err(lightcraft_birefnet::Error::Candle(e)) if !cpu => {
                    log::warn!("BiRefNet Matting failed on the GPU ({e}); trying the CPU");
                    *slot = None;
                }
                Err(e) => return Err(damaged(e)),
            }
        }
        Err("BiRefNet Matting could not run on this Mac".into())
    }

    /// Drop the model once it has been idle for [`KEEP_LOADED`] (one watcher thread at a time).
    fn watch(&self) {
        if self.inner.watcher.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let spawned = std::thread::Builder::new().name("matting-idle".into()).spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(10));
                let Some(inner) = weak.upgrade() else { return };
                let mut slot = lock(&inner.loaded);
                if slot.as_ref().is_none_or(|l| l.last_used.elapsed() >= KEEP_LOADED) {
                    *slot = None;
                    inner.watcher.store(false, Ordering::Release);
                    return;
                }
            }
        });
        if spawned.is_err() {
            self.inner.watcher.store(false, Ordering::Release);
        }
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
