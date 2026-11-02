//! Fork feature (`fork/lama-fill.md`): the LaMa fill behind Remove and Content-Aware spots whose fill is LaMa.
//!
//! [`LamaFill`] is the pipeline's [`LearnedFill`]: it finds the model (`big-lama.pt`, shared with PhotoCraft in
//! `ArtCraft/models/lama`), runs it with a row in the activity stack that ✕ stops, and keeps what it made (by the pixels it
//! was given) so a slider drag, a second render at the same size and an export ask again for free. Drafts (renders that
//! follow a drag) take a cached fill or none, so a drag never starts a run.
//!
//! Never crashes: no model, a damaged model, a failed or cancelled run, and a panic inside the run all give `None`
//! (the spot shows the Classic fill) and a message ([`LamaFill::message`]) the Remove panel shows.
//!
//! One run at a time (the model is large); the model stays loaded for a minute after its last use.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_pipeline::learned_fill::LearnedFill;

use crate::activity::{Activity, Cancel};

mod download;
pub use download::{DOWNLOAD_BYTES, DOWNLOAD_URL, Download};

/// The model's file name.
pub const MODEL_FILE: &str = "big-lama.pt";

/// How long the model stays loaded after its last use.
const KEEP_LOADED: Duration = Duration::from_secs(60);
/// What the fills kept are allowed to take together, bytes (the newest stay).
const CACHE_BYTES: usize = 256 << 20;

/// Where the model is kept: a non-empty `LIGHTCRAFT_LAMA_DIR`; else on the Mac the folder shared with PhotoCraft,
/// `~/Library/Application Support/ArtCraft/models/lama`; else `<config>/models/lama`.
pub fn default_lama_dir() -> Option<PathBuf> {
    lama_dir(std::env::var_os("LIGHTCRAFT_LAMA_DIR"), std::env::var_os("HOME"), crate::config::config_dir())
}

fn lama_dir(var: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>, config: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(v) = var.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    if cfg!(target_os = "macos")
        && let Some(home) = home.filter(|h| !h.is_empty())
    {
        return Some(PathBuf::from(home).join("Library").join("Application Support").join("ArtCraft").join("models").join("lama"));
    }
    config.map(|c| c.join("models").join("lama"))
}

struct Loaded {
    #[cfg(not(target_arch = "wasm32"))]
    model: Arc<lightcraft_lama::Lama>,
    /// The file's size and time when it was read: a file replaced meanwhile is read again.
    stamp: (u64, Option<std::time::SystemTime>),
    last_used: Instant,
}

#[derive(Default)]
struct Cache {
    /// Oldest first.
    entries: VecDeque<(u64, Arc<Vec<f32>>)>,
    bytes: usize,
}

struct Inner {
    activity: Activity,
    dir: Mutex<Option<PathBuf>>,
    model: Mutex<Option<Loaded>>,
    /// One run at a time.
    run: Mutex<()>,
    cache: Mutex<Cache>,
    message: Mutex<Option<String>>,
    download: Mutex<Download>,
    watcher: AtomicBool,
}

/// The LaMa fill of a session. Cheap to clone; every clone is the same fill.
#[derive(Clone)]
pub struct LamaFill {
    inner: Arc<Inner>,
}

impl LamaFill {
    /// A fill with no model folder (so nothing installed): the session's own activity stack shows its rows.
    pub fn new(activity: Activity) -> Self {
        LamaFill {
            inner: Arc::new(Inner {
                activity,
                dir: Mutex::new(None),
                model: Mutex::new(None),
                run: Mutex::new(()),
                cache: Mutex::new(Cache::default()),
                message: Mutex::new(None),
                download: Mutex::new(Download::Idle),
                watcher: AtomicBool::new(false),
            }),
        }
    }

    /// Look for the model in `dir` (`None`: nowhere).
    pub fn set_dir(&self, dir: Option<PathBuf>) {
        *self.inner.dir.lock().unwrap_or_else(PoisonError::into_inner) = dir;
        *self.inner.model.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The folder the model lives in (or goes to when downloaded).
    pub fn dir(&self) -> Option<PathBuf> {
        self.inner.dir.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The model file, whether or not it is there.
    pub fn model_path(&self) -> Option<PathBuf> {
        self.dir().map(|d| d.join(MODEL_FILE))
    }

    /// Whether the model file is in place (it is checked when read).
    pub fn installed(&self) -> bool {
        self.model_path().is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0))
    }

    /// What a render of a photo with LaMa spots adds to its cache key: 0 for every other photo; else it says whether
    /// the model is there, so installing it redraws the spots the Classic fill stood in for.
    pub fn cache_salt(&self, settings: &lightcraft_develop::DevelopSettings) -> u64 {
        if settings.spots.iter().any(lightcraft_pipeline::spots_learned::is_learned) { 0x4c41_4d41 ^ u64::from(self.installed()) } else { 0 }
    }

    /// The last thing worth telling the user about the fill (no model, a failed or cancelled run), if still true.
    pub fn message(&self) -> Option<String> {
        self.inner.message.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn say(&self, text: impl Into<String>) {
        let text = text.into();
        log::warn!("LaMa: {text}");
        *self.inner.message.lock().unwrap_or_else(PoisonError::into_inner) = Some(text);
    }

    fn clear_message(&self) {
        *self.inner.message.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Where the model download stands.
    pub fn download_state(&self) -> Download {
        self.inner.download.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn cached(&self, key: u64) -> Option<Arc<Vec<f32>>> {
        self.inner.cache.lock().unwrap_or_else(PoisonError::into_inner).entries.iter().find(|e| e.0 == key).map(|e| e.1.clone())
    }

    fn keep(&self, key: u64, filled: Arc<Vec<f32>>) {
        let mut c = self.inner.cache.lock().unwrap_or_else(PoisonError::into_inner);
        c.entries.retain(|e| e.0 != key);
        c.bytes += filled.len() * 4;
        c.entries.push_back((key, filled));
        while c.bytes > CACHE_BYTES && c.entries.len() > 1 {
            if let Some((_, old)) = c.entries.pop_front() {
                c.bytes = c.bytes.saturating_sub(old.len() * 4);
            }
        }
    }

    /// Fills ready to give back at once (for tests and `lama.status`).
    pub fn cached_fills(&self) -> usize {
        self.inner.cache.lock().unwrap_or_else(PoisonError::into_inner).entries.len()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn model(&self) -> Result<Arc<lightcraft_lama::Lama>, String> {
        let path = self.model_path().ok_or("LaMa has no model folder")?;
        let meta = std::fs::metadata(&path).map_err(|_| format!("LaMa is not installed ({MODEL_FILE} is not in {})", path.parent().map_or(String::new(), |p| p.display().to_string())))?;
        let stamp = (meta.len(), meta.modified().ok());
        let mut slot = self.inner.model.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(l) = slot.as_mut().filter(|l| l.stamp == stamp) {
            l.last_used = Instant::now();
            return Ok(l.model.clone());
        }
        let model = lightcraft_lama::Lama::load(&path, &lightcraft_lama::best_device()).map_err(|e| format!("could not read the LaMa model: {e}"))?;
        let model = Arc::new(model);
        *slot = Some(Loaded { model: model.clone(), stamp, last_used: Instant::now() });
        drop(slot);
        self.watch();
        Ok(model)
    }

    /// Drop the loaded model once it has been idle for [`KEEP_LOADED`] (one watcher thread at a time).
    #[cfg(not(target_arch = "wasm32"))]
    fn watch(&self) {
        if self.inner.watcher.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let spawned = std::thread::Builder::new().name("lama-idle".into()).spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let Some(inner) = weak.upgrade() else { return };
                let mut slot = inner.model.lock().unwrap_or_else(PoisonError::into_inner);
                let idle = slot.as_ref().is_none_or(|l| l.last_used.elapsed() >= KEEP_LOADED);
                if idle {
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

    #[cfg(not(target_arch = "wasm32"))]
    fn run_model(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, cancel: &dyn Fn() -> bool) -> Result<Vec<f32>, String> {
        let model = self.model()?;
        let params = lightcraft_lama::region::Params::default();
        model.fill_region(rgb, hole, w, h, &params, cancel).map_err(|e| match e {
            lightcraft_lama::Error::Cancelled => "cancelled".to_string(),
            other => other.to_string(),
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn run_model(&self, _: &[f32], _: &[f32], _: usize, _: usize, _: &dyn Fn() -> bool) -> Result<Vec<f32>, String> {
        Err("the web build has no LaMa fill".into())
    }

    /// Fill `hole` of `rgb`, from the cache or with a run (see [`LearnedFill::fill`]). The row `kind` is `lama`.
    fn make(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, key: u64) -> Option<Arc<Vec<f32>>> {
        if !self.installed() {
            let place = self.dir().map_or_else(|| "its model folder".to_string(), |d| d.display().to_string());
            self.say(format!("LaMa is not installed ({MODEL_FILE} is not in {place}): spots show the Classic fill"));
            return None;
        }
        let _one_at_a_time = self.inner.run.lock().unwrap_or_else(PoisonError::into_inner);
        // another render may have made it while this one waited
        if let Some(hit) = self.cached(key) {
            return Some(hit);
        }
        let flag = Arc::new(AtomicBool::new(false));
        let _row = self.inner.activity.start("lama", "Filling with LaMa", Cancel::Flag(flag.clone()));
        let cancelled = || flag.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down();
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_model(rgb, hole, w, h, &cancelled)));
        match ran {
            Ok(Ok(out)) if out.len() == rgb.len() && out.iter().all(|v| v.is_finite()) => {
                self.clear_message();
                let out = Arc::new(out);
                self.keep(key, out.clone());
                Some(out)
            }
            Ok(Ok(_)) => {
                self.say("LaMa gave a result that does not fit the picture: the spot shows the Classic fill");
                None
            }
            Ok(Err(e)) if e == "cancelled" => {
                self.say("LaMa fill cancelled: the spot shows the Classic fill");
                None
            }
            Ok(Err(e)) => {
                self.say(format!("{e}: the spot shows the Classic fill"));
                None
            }
            Err(_) => {
                self.say("LaMa stopped unexpectedly: the spot shows the Classic fill");
                None
            }
        }
    }
}

/// What a fill is stored under: the picture and hole it was made from.
fn key_of(rgb: &[f32], hole: &[f32], w: usize, h: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write_usize(w);
    hasher.write_usize(h);
    for v in rgb {
        hasher.write_u32(v.to_bits());
    }
    for v in hole {
        hasher.write_u32(v.to_bits());
    }
    hasher.finish()
}

impl LearnedFill for LamaFill {
    fn fill(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, draft: bool) -> Option<Vec<f32>> {
        if w.checked_mul(h).is_none_or(|n| n == 0 || rgb.len() != n * 3 || hole.len() != n) {
            return None;
        }
        let key = key_of(rgb, hole, w, h);
        if let Some(hit) = self.cached(key) {
            return Some(hit.as_ref().clone());
        }
        // a drag's drafts start nothing: the Classic fill shows until the full render has the real one
        if draft {
            return None;
        }
        self.make(rgb, hole, w, h, key).map(|a| a.as_ref().clone())
    }
}

impl std::fmt::Debug for LamaFill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LamaFill").field("dir", &self.dir()).finish()
    }
}

/// The model's path inside a folder.
pub fn model_in(dir: &Path) -> PathBuf {
    dir.join(MODEL_FILE)
}

impl crate::Session {
    /// Look for the LaMa model in the shared default folder ([`default_lama_dir`]) and use the fill for every render of
    /// this process. Hosts (the app, the CLI, MCP) ask for it; a bare session has no model, so tests stay hermetic.
    pub fn with_default_lama_model(self) -> Self {
        self.with_lama_dir(default_lama_dir())
    }

    /// [`Session::with_default_lama_model`] with the model folder `dir` (`None`: no model).
    pub fn with_lama_dir(self, dir: Option<PathBuf>) -> Self {
        self.lama.set_dir(dir);
        let provider: Arc<dyn LearnedFill> = Arc::new(self.lama.clone());
        lightcraft_pipeline::learned_fill::install(Some(provider));
        self
    }
}
