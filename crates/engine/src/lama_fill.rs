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
//!
//! On a Mac with LaMa's Core ML version in the model's folder, fills run on the Neural Engine (issue #268, `coreml`).

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_pipeline::learned_fill::{FillRun, LearnedFill};

use crate::activity::{Activity, Cancel};

#[cfg(not(target_arch = "wasm32"))]
mod coreml;
mod download;
pub use download::{DOWNLOAD_BYTES, DOWNLOAD_URL, Download};

/// The model's file name.
pub const MODEL_FILE: &str = "big-lama.pt";

/// How long the model stays loaded after its last use.
const KEEP_LOADED: Duration = Duration::from_secs(60);
/// How long [`LamaFill::term`] trusts what it read from the model's file.
const TERM_EVERY: Duration = Duration::from_secs(2);
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
    if let Some(shared) = crate::shared_models::shared_models_dir(home) {
        return Some(shared.join("lama"));
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
    /// [`LamaFill::term`], read from the file at most every [`TERM_EVERY`].
    term: Mutex<(Option<Instant>, u64)>,
    /// The engine choice and the Core ML model (`coreml`).
    #[cfg(not(target_arch = "wasm32"))]
    coreml: Mutex<coreml::CoreMl>,
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
                term: Mutex::new((None, 0)),
                #[cfg(not(target_arch = "wasm32"))]
                coreml: Mutex::new(coreml::CoreMl::default()),
            }),
        }
    }

    /// Look for the model in `dir` (`None`: nowhere).
    pub fn set_dir(&self, dir: Option<PathBuf>) {
        *self.inner.dir.lock().unwrap_or_else(PoisonError::into_inner) = dir;
        self.inner.term.lock().unwrap_or_else(PoisonError::into_inner).0 = None;
        *self.inner.model.lock().unwrap_or_else(PoisonError::into_inner) = None;
        #[cfg(not(target_arch = "wasm32"))]
        {
            coreml::reset(&mut self.inner.coreml.lock().unwrap_or_else(PoisonError::into_inner));
            self.warm();
        }
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

    /// What a search that uses LaMa (Wrinkles' face search, `fork/retouch-wrinkles.md`) adds to its key: 0 while the model
    /// is not there, else a number made of its file's size and time, so installing it (or replacing it) searches the photos
    /// again. The file is looked at every [`TERM_EVERY`] at most (keys are made on every render).
    pub fn term(&self) -> u64 {
        let mut t = self.inner.term.lock().unwrap_or_else(PoisonError::into_inner);
        if t.0.is_none_or(|at| at.elapsed() >= TERM_EVERY) {
            let meta = self.model_path().and_then(|p| std::fs::metadata(p).ok()).filter(|m| m.is_file() && m.len() > 0);
            let stamp = meta.map_or(0, |m| {
                let secs = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                (m.len() ^ secs.rotate_left(23)) | 1
            });
            *t = (Some(Instant::now()), stamp);
        }
        t.1
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
        let meta = std::fs::metadata(&path).map_err(|_| {
            format!("LaMa is not installed ({MODEL_FILE} is not in {})", path.parent().map_or(String::new(), |p| p.display().to_string()))
        })?;
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
    fn run_model(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, params: Crop, cancel: &dyn Fn() -> bool) -> Result<Vec<f32>, String> {
        let params = lightcraft_lama::region::Params { work: params.work, context: params.context, grow: params.grow };
        // (the crop goes to Core ML or candle: `coreml`)
        lightcraft_lama::region::fill(rgb, hole, w, h, &params, &|img, m, ww, wh| self.fill_crop(img, m, ww, wh, cancel)).map_err(|e| match e {
            lightcraft_lama::Error::Cancelled => "cancelled".to_string(),
            other => other.to_string(),
        })
    }

    /// The web build has no Core ML: LaMa's engine choice is kept and does nothing.
    #[cfg(target_arch = "wasm32")]
    pub fn set_runtime(&self, _runtime: lightcraft_coreml::Runtime) {}

    #[cfg(target_arch = "wasm32")]
    pub fn runtime(&self) -> lightcraft_coreml::Runtime {
        lightcraft_coreml::Runtime::Candle
    }

    #[cfg(target_arch = "wasm32")]
    pub fn backend(&self) -> Option<&'static str> {
        None
    }

    #[cfg(target_arch = "wasm32")]
    pub fn coreml_installed(&self) -> bool {
        false
    }

    #[cfg(target_arch = "wasm32")]
    fn run_model(&self, _: &[f32], _: &[f32], _: usize, _: usize, _: Crop, _: &dyn Fn() -> bool) -> Result<Vec<f32>, String> {
        Err("the web build has no LaMa fill".into())
    }

    /// Fill `hole` of `rgb`, from the cache or with a run (see [`LearnedFill::fill`]). The row `kind` is `lama`, one per
    /// render (`run` holds it until the render's last spot): its ✕ sets the render's stop flag, so the spots that follow
    /// are not started either.
    fn make(&self, run: &FillRun, job: Job<'_>, use_: &Use) -> Option<Arc<Vec<f32>>> {
        let Job { rgb, hole, w, h, key } = job;
        if run.stopped() {
            return None;
        }
        if !self.installed() {
            let place = self.dir().map_or_else(|| "its model folder".to_string(), |d| d.display().to_string());
            self.say(format!("LaMa is not installed ({MODEL_FILE} is not in {place}): {}", use_.without));
            return None;
        }
        let _one_at_a_time = self.inner.run.lock().unwrap_or_else(PoisonError::into_inner);
        // (one model on the GPU at a time: `crate::gpu_run`)
        let _gpu = crate::gpu_run::turn();
        // another render may have made it while this one waited
        if let Some(hit) = self.cached(key) {
            return Some(hit);
        }
        if run.stopped() {
            return None;
        }
        if !run.holds() {
            run.hold(Box::new(self.inner.activity.start("lama", "Filling with LaMa", Cancel::Flag(run.stop_flag()))));
        }
        let cancelled = || run.stopped() || lightcraft_gpu::shutting_down();
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_model(rgb, hole, w, h, use_.params, &cancelled)));
        match ran {
            Ok(Ok(out)) if out.len() == rgb.len() && out.iter().all(|v| v.is_finite()) => {
                self.clear_message();
                let out = Arc::new(out);
                self.keep(key, out.clone());
                Some(out)
            }
            Ok(Ok(_)) => {
                self.say(format!("LaMa gave a result that does not fit the picture: {}", use_.without));
                None
            }
            Ok(Err(e)) if e == "cancelled" => {
                self.say(format!("LaMa fill cancelled: {}", use_.without));
                None
            }
            Ok(Err(e)) => {
                self.say(format!("{e}: {}", use_.without));
                None
            }
            Err(_) => {
                self.say(format!("LaMa stopped unexpectedly: {}", use_.without));
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

/// How much of the picture the model sees: the longest side it works at, the context around the hole (a fraction of the
/// hole's longer side) and how far the hole grows (`lightcraft_lama::region::Params`).
#[derive(Clone, Copy)]
struct Crop {
    work: usize,
    context: f32,
    grow: usize,
}

/// The picture, the hole and what the fill is cached under.
struct Job<'a> {
    rgb: &'a [f32],
    hole: &'a [f32],
    w: usize,
    h: usize,
    key: u64,
}

/// What a fill is for: how the model is run, and what is said when there is none.
struct Use {
    params: Crop,
    without: &'static str,
}

impl Use {
    fn spots() -> Use {
        Use { params: Crop { work: 512, context: 0.5, grow: 2 }, without: "the spot shows the Classic fill" }
    }

    /// Wrinkles (`fork/retouch-wrinkles.md`): a face crop of at most 768 px, with little context around the hole.
    fn wrinkles() -> Use {
        Use { params: Crop { work: 768, context: 0.15, grow: 2 }, without: "Wrinkles use the built-in heal" }
    }
}

impl LamaFill {
    /// Fill `hole` (one value per pixel) of the linear `w × h` picture `rgb` for Wrinkles' face search: the same run as
    /// a spot's, at the face crop's resolution, with the same cache, one activity row held by `run` and ✕ on it. `None`:
    /// not installed, stopped or failed (the search then heals without a model).
    pub fn fill_wrinkles(&self, run: &FillRun, rgb: &[f32], hole: &[f32], w: usize, h: usize) -> Option<Arc<Vec<f32>>> {
        if w.checked_mul(h).and_then(|n| n.checked_mul(3)).is_none_or(|n3| w == 0 || h == 0 || rgb.len() != n3 || hole.len() != n3 / 3) {
            return None;
        }
        // (another key than a spot's fill of the same pixels: the model runs at another size)
        let key = key_of(rgb, hole, w, h) ^ 0x5752_494e_4b4c_4553;
        if let Some(hit) = self.cached(key) {
            return Some(hit);
        }
        self.make(run, Job { rgb, hole, w, h, key }, &Use::wrinkles())
    }
}

impl LearnedFill for LamaFill {
    fn fill(&self, run: &FillRun, rgb: &[f32], hole: &[f32], w: usize, h: usize, draft: bool) -> Option<Arc<Vec<f32>>> {
        if w.checked_mul(h).and_then(|n| n.checked_mul(3)).is_none_or(|n3| w == 0 || h == 0 || rgb.len() != n3 || hole.len() != n3 / 3) {
            return None;
        }
        let key = key_of(rgb, hole, w, h);
        if let Some(hit) = self.cached(key) {
            return Some(hit);
        }
        // a drag's drafts start nothing: the Classic fill shows until the full render has the real one
        if draft {
            return None;
        }
        self.make(run, Job { rgb, hole, w, h, key }, &Use::spots())
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
        // Repair Glare's FLUX model and results (`fork/glare-repair.md`): set up by the same hosts, for the same reason
        #[cfg(not(target_arch = "wasm32"))]
        self.glare.set_dirs(crate::glare_repair::default_flux_dir(), crate::config::cache_dir().map(|d| d.join("glare-repair")));
        // the engine chosen in Settings, before the folder starts the Core ML load (`coreml`)
        self.model_runtimes.open(crate::model_runtime::default_file());
        self.lama.set_runtime(self.model_runtimes.get(crate::model_runtime::LAMA));
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
