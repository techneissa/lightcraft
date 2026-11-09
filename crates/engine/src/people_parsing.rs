//! Fork feature (`fork/people-parsing.md`): per-person part masks (skin, hair, eyes, lips, teeth, clothes) and face
//! landmarks for a photo, made by `lightcraft-parsing` with the model set the user picked.
//!
//! [`PeopleParsing`] keeps the models folder (on the Mac the one shared with the other ArtCraft apps), the chosen set
//! (`<config>/parsing-model.json`), the downloads, and the parses. A photo is parsed once per model version on a worker
//! thread holding a row in the activity stack (✕ stops it); the result is filed on disk by the pixels it was made from
//! and the set's version (`~/Library/Caches/LightCraft/people-parsing`), and the newest few stay in memory. A caller
//! that needs the masks now (an export) waits for the run already going instead of starting a second one.
//!
//! Never crashes: no model, a damaged model or cache file, a failed or cancelled run and a panic inside the run all
//! end as a message.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_parsing::{FaceHint, ModelSet, Parsing, RgbImage, cache};

use crate::activity::{Activity, Cancel};

mod download;
pub use download::Download;

/// The newest parses kept in memory.
const IN_MEMORY: usize = 4;
/// What the parse files on disk may take together (the oldest go first).
const CACHE_BYTES: u64 = 512 << 20;
/// How long the models stay loaded after their last use.
const KEEP_LOADED: Duration = Duration::from_secs(60);
/// The long edge of the picture a photo is parsed at.
pub const PARSE_EDGE: usize = 2048;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How a request for a photo's parse ended.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// The parse; `cached`: it came from memory or disk, no model ran.
    Ready {
        parsing: Arc<Parsing>,
        cached: bool,
    },
    /// A run is going (the caller did not wait).
    Running,
    Failed(String),
}

/// What runs a parse: the model set's parser normally; tests put a fake one in.
pub type Runner = Arc<dyn Fn(ModelSet, &Path, &RgbImage, &[FaceHint], &dyn Fn() -> bool) -> Result<Parsing, String> + Send + Sync>;

/// A stand-in for the network in tests: gets the set and the models folder, says how the download ended.
#[cfg(any(test, feature = "test-hooks"))]
pub type TestFetch = Arc<dyn Fn(ModelSet, &Path) -> Download + Send + Sync>;

enum Job {
    Running(Arc<AtomicBool>),
    /// The result and when it finished (a count, oldest lowest).
    Done(Result<Arc<Parsing>, String>, u64),
}

/// Finished runs kept for whoever waits on them (their parses are also cached).
const KEEP_FINISHED: usize = 16;

/// The runs, and how many callers wait on each.
#[derive(Default)]
struct Jobs {
    map: HashMap<String, Job>,
    waiters: HashMap<String, usize>,
    finished: u64,
}

impl Jobs {
    /// File a finished run, then let the oldest finished runs nobody waits on go, down to [`KEEP_FINISHED`].
    fn finish(&mut self, key: String, result: Result<Arc<Parsing>, String>) {
        self.finished += 1;
        self.map.insert(key, Job::Done(result, self.finished));
        while self.map.values().filter(|j| matches!(j, Job::Done(..))).count() > KEEP_FINISHED {
            let oldest = self
                .map
                .iter()
                .filter(|(k, _)| self.waiters.get(*k).copied().unwrap_or(0) == 0)
                .filter_map(|(k, j)| match j {
                    Job::Done(_, n) => Some((*n, k.clone())),
                    Job::Running(_) => None,
                })
                .min();
            let Some((_, k)) = oldest else { break };
            self.map.remove(&k);
        }
    }

    #[cfg(test)]
    fn finished_kept(&self) -> usize {
        self.map.values().filter(|j| matches!(j, Job::Done(..))).count()
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct Loaded {
    set: ModelSet,
    parser: Arc<lightcraft_parsing::Parser>,
    last_used: Instant,
}

#[derive(Default)]
struct Paths {
    models: Option<PathBuf>,
    cache: Option<PathBuf>,
    choice: Option<PathBuf>,
}

struct Inner {
    activity: Activity,
    paths: Mutex<Paths>,
    set: Mutex<Option<ModelSet>>,
    jobs: Mutex<Jobs>,
    done: Condvar,
    memory: Mutex<VecDeque<(String, Arc<Parsing>)>>,
    downloads: Mutex<HashMap<ModelSet, Download>>,
    download_flags: Mutex<Vec<Arc<AtomicBool>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    test_fetch: Mutex<Option<TestFetch>>,
    runner: Mutex<Option<Runner>>,
    /// One model run at a time: runs share one loaded model and its GPU device, whose buffers are safe with one graph
    /// in flight only.
    one_run: Mutex<()>,
    #[cfg(not(target_arch = "wasm32"))]
    loaded: Mutex<Option<Loaded>>,
    watcher: AtomicBool,
    runs: std::sync::atomic::AtomicUsize,
    message: Mutex<Option<String>>,
    /// BiRefNet-matting edges for Entire Person and Background (`fork/hair-edges.md`), when the session has them.
    hair_edges: Mutex<Option<crate::hair_edges::HairEdges>>,
}

/// The people parsing of a session. Cheap to clone; every clone is the same.
#[derive(Clone)]
pub struct PeopleParsing {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PeopleParsing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeopleParsing").field("models", &self.models_dir()).finish()
    }
}

impl PeopleParsing {
    /// No models folder, no cache, no saved choice (so nothing is installed): rows go to `activity`.
    pub fn new(activity: Activity) -> Self {
        Self {
            inner: Arc::new(Inner {
                activity,
                paths: Mutex::new(Paths::default()),
                set: Mutex::new(None),
                jobs: Mutex::new(Jobs::default()),
                done: Condvar::new(),
                memory: Mutex::new(VecDeque::new()),
                downloads: Mutex::new(HashMap::new()),
                download_flags: Mutex::new(Vec::new()),
                #[cfg(any(test, feature = "test-hooks"))]
                test_fetch: Mutex::new(None),
                runner: Mutex::new(None),
                one_run: Mutex::new(()),
                #[cfg(not(target_arch = "wasm32"))]
                loaded: Mutex::new(None),
                watcher: AtomicBool::new(false),
                runs: std::sync::atomic::AtomicUsize::new(0),
                message: Mutex::new(None),
                hair_edges: Mutex::new(None),
            }),
        }
    }

    /// Draw Entire Person and Background edges with `edges` when its People edge is BiRefNet Matting.
    pub fn with_hair_edges(self, edges: crate::hair_edges::HairEdges) -> Self {
        *lock(&self.inner.hair_edges) = Some(edges);
        self
    }

    /// The hair edges to draw parses with now, and their edge model, if one other than Current is chosen for people
    /// (and here: BiRefNet Matting installed, Apple Vision on a Mac that runs it).
    fn matting(&self) -> Option<(crate::hair_edges::HairEdges, crate::hair_edges::Edge)> {
        use crate::hair_edges::{Edge, EdgeKind};
        let edges = lock(&self.inner.hair_edges).clone()?;
        let edge = edges.edge(EdgeKind::People);
        (edge != Edge::Current).then_some((edges, edge))
    }

    /// Use these folders: `models` (each model in its own folder under it), `cache` (parse files), `choice` (the file
    /// that keeps the chosen set). `None`: none.
    pub fn set_paths(&self, models: Option<PathBuf>, cache: Option<PathBuf>, choice: Option<PathBuf>) {
        let saved = choice.as_deref().and_then(read_choice);
        *lock(&self.inner.paths) = Paths { models, cache, choice };
        *lock(&self.inner.set) = saved;
        #[cfg(not(target_arch = "wasm32"))]
        {
            *lock(&self.inner.loaded) = None;
        }
    }

    pub fn models_dir(&self) -> Option<PathBuf> {
        lock(&self.inner.paths).models.clone()
    }

    pub fn cache_dir(&self) -> Option<PathBuf> {
        lock(&self.inner.paths).cache.clone()
    }

    /// Whether every file of `set` is in the models folder.
    pub fn installed(&self, set: ModelSet) -> bool {
        self.models_dir().is_some_and(|d| set.installed_in(&d))
    }

    /// The set that parses: the saved choice; else the first installed set; else SegFormer.
    pub fn model_set(&self) -> ModelSet {
        if let Some(s) = *lock(&self.inner.set) {
            return s;
        }
        ModelSet::ALL.into_iter().find(|s| self.installed(*s)).unwrap_or(ModelSet::Segformer)
    }

    /// Choose the set that parses from now on (kept in the choice file when there is one).
    pub fn set_model_set(&self, set: ModelSet) -> Result<(), String> {
        *lock(&self.inner.set) = Some(set);
        let Some(path) = lock(&self.inner.paths).choice.clone() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("could not keep the choice: {e}"))?;
        }
        std::fs::write(&path, serde_json::json!({"set": set.id()}).to_string()).map_err(|e| format!("could not keep the choice: {e}"))
    }

    /// The last thing worth telling the user (a failed run), if any.
    pub fn message(&self) -> Option<String> {
        lock(&self.inner.message).clone()
    }

    fn say(&self, text: impl Into<String>) {
        let text = text.into();
        log::warn!("people parsing: {text}");
        *lock(&self.inner.message) = Some(text);
    }

    /// How many times a model ran (for tests and status).
    pub fn runs(&self) -> usize {
        self.inner.runs.load(Ordering::Relaxed)
    }

    /// Whether a run is going.
    pub fn busy(&self) -> bool {
        lock(&self.inner.jobs).map.values().any(|j| matches!(j, Job::Running(_)))
    }

    /// Stop every run going.
    pub fn cancel_runs(&self) {
        for j in lock(&self.inner.jobs).map.values() {
            if let Job::Running(flag) = j {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn set_runner(&self, runner: Option<Runner>) {
        *lock(&self.inner.runner) = runner;
    }

    fn remembered(&self, key: &str) -> Option<Arc<Parsing>> {
        lock(&self.inner.memory).iter().find(|e| e.0 == key).map(|e| e.1.clone())
    }

    fn remember(&self, key: &str, p: Arc<Parsing>) {
        let mut m = lock(&self.inner.memory);
        m.retain(|e| e.0 != key);
        m.push_back((key.to_string(), p));
        while m.len() > IN_MEMORY {
            m.pop_front();
        }
    }

    /// The parse filed on disk under `key`; a damaged file is removed (the photo is parsed again).
    fn filed(&self, key: &str) -> Option<Arc<Parsing>> {
        let dir = self.cache_dir()?;
        match cache::load(&dir, key) {
            Ok(p) => p.map(Arc::new),
            Err(e) => {
                log::warn!("people parsing: {e}; parsing again");
                let _ = std::fs::remove_file(cache::path(&dir, key));
                None
            }
        }
    }

    /// The parse of `img` (with the faces `hints`) by the chosen set: from memory or disk when it was made before;
    /// else a run on a worker thread. With `wait`, the call returns when the run (this one, or one already going for
    /// the same picture) has ended; without, it returns [`Outcome::Running`].
    pub fn parse(&self, img: RgbImage, hints: Vec<FaceHint>, wait: bool) -> Outcome {
        let set = self.model_set();
        let matting = self.matting();
        // (a parse with matting edges is its own version, so switching the edge never reads the other's file)
        let version = match matting.as_ref().and_then(|(_, e)| crate::hair_edges::vision::version(*e)) {
            Some(edge) => format!("{}+{edge}", set.version()),
            None => set.version(),
        };
        let key = cache::key(&img, &hints, &version);
        if let Some(p) = self.remembered(&key).or_else(|| self.filed(&key)) {
            self.remember(&key, p.clone());
            return Outcome::Ready { parsing: p, cached: true };
        }
        let has_runner = lock(&self.inner.runner).is_some();
        let Some(models) = self.models_dir().filter(|d| has_runner || set.installed_in(d)) else {
            return Outcome::Failed(format!(
                "the {} people parsing models are not downloaded yet ({} MB; parsing.download)",
                set.label(),
                set.size().div_ceil(1_000_000)
            ));
        };
        {
            let mut jobs = lock(&self.inner.jobs);
            match jobs.map.get(&key) {
                Some(Job::Running(_)) if !wait => return Outcome::Running,
                Some(Job::Running(_)) => {}
                _ => {
                    let flag = Arc::new(AtomicBool::new(false));
                    jobs.map.insert(key.clone(), Job::Running(flag.clone()));
                    drop(jobs);
                    if let Err(e) = self.spawn(key.clone(), set, models, (img, hints), matting, flag) {
                        lock(&self.inner.jobs).finish(key.clone(), Err(e.clone()));
                        self.inner.done.notify_all();
                        return Outcome::Failed(e);
                    }
                    if !wait {
                        return Outcome::Running;
                    }
                }
            }
        }
        self.wait_for(&key)
    }

    /// Wait until the run for `key` has ended. Its result stays filed while this waits; a run no longer filed (it
    /// finished long ago) gives its cached parse.
    pub(crate) fn wait_for(&self, key: &str) -> Outcome {
        let mut jobs = lock(&self.inner.jobs);
        *jobs.waiters.entry(key.to_string()).or_default() += 1;
        let outcome = loop {
            match jobs.map.get(key) {
                Some(Job::Running(_)) => {
                    jobs = self.inner.done.wait_timeout(jobs, Duration::from_millis(500)).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
                }
                Some(Job::Done(Ok(p), _)) => break Some(Outcome::Ready { parsing: p.clone(), cached: false }),
                Some(Job::Done(Err(e), _)) => break Some(Outcome::Failed(e.clone())),
                None => break None,
            }
        };
        if let Some(n) = jobs.waiters.get_mut(key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                jobs.waiters.remove(key);
            }
        }
        drop(jobs);
        outcome.unwrap_or_else(|| match self.remembered(key).or_else(|| self.filed(key)) {
            Some(parsing) => Outcome::Ready { parsing, cached: true },
            None => Outcome::Failed("the run was dropped".into()),
        })
    }

    #[cfg(test)]
    pub(crate) fn finished_kept(&self) -> usize {
        lock(&self.inner.jobs).finished_kept()
    }

    fn spawn(
        &self,
        key: String,
        set: ModelSet,
        models: PathBuf,
        (img, hints): (RgbImage, Vec<FaceHint>),
        matting: Option<(crate::hair_edges::HairEdges, crate::hair_edges::Edge)>,
        flag: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let row = self.inner.activity.start("parsing", "Finding people parts", Cancel::Flag(flag.clone()));
        row.detail(set.label());
        let this = self.clone();
        std::thread::Builder::new()
            .name("people-parsing".into())
            .spawn(move || {
                // the row goes when the thread ends, however it ends
                let _row = row;
                let cancelled = || flag.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down();
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let p = this.run(set, &models, &img, &hints, &cancelled)?;
                    match &matting {
                        Some((edges, crate::hair_edges::Edge::Vision)) => crate::hair_edges::vision::people_edged(edges, p, &img, &flag),
                        Some((edges, edge)) => hair_edged(edges, *edge, p, &img, &flag),
                        None => Ok(p),
                    }
                }));
                let result = match ran {
                    Ok(Ok(p)) => {
                        let p = Arc::new(p);
                        if let Some(dir) = this.cache_dir() {
                            if let Err(e) = cache::store(&dir, &key, &p) {
                                log::warn!("people parsing: {e}");
                            }
                            cache::prune(&dir, CACHE_BYTES);
                        }
                        this.remember(&key, p.clone());
                        *lock(&this.inner.message) = None;
                        Ok(p)
                    }
                    Ok(Err(e)) => {
                        this.say(&e);
                        Err(e)
                    }
                    Err(_) => {
                        this.say("people parsing stopped unexpectedly");
                        Err("people parsing stopped unexpectedly".into())
                    }
                };
                lock(&this.inner.jobs).finish(key, result);
                this.inner.done.notify_all();
            })
            .map(|_| ())
            .map_err(|e| format!("could not start people parsing: {e}"))
    }

    fn run(&self, set: ModelSet, models: &Path, img: &RgbImage, hints: &[FaceHint], cancelled: &dyn Fn() -> bool) -> Result<Parsing, String> {
        let _one_at_a_time = lock(&self.inner.one_run);
        // (one model on the GPU at a time, hair edges and LaMa included: `crate::gpu_run`)
        let _gpu = crate::gpu_run::turn();
        if cancelled() {
            return Err("cancelled".into());
        }
        self.inner.runs.fetch_add(1, Ordering::Relaxed);
        let runner = lock(&self.inner.runner).clone();
        if let Some(r) = runner {
            return r(set, models, img, hints, cancelled);
        }
        self.run_models(set, models, img, hints, cancelled)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run_models(&self, set: ModelSet, models: &Path, img: &RgbImage, hints: &[FaceHint], cancelled: &dyn Fn() -> bool) -> Result<Parsing, String> {
        let parser = {
            let mut slot = lock(&self.inner.loaded);
            match slot.as_mut().filter(|l| l.set == set) {
                Some(l) => {
                    l.last_used = Instant::now();
                    l.parser.clone()
                }
                None => {
                    *slot = None;
                    let p = Arc::new(lightcraft_parsing::Parser::load(set, models).map_err(|e| e.to_string())?);
                    *slot = Some(Loaded { set, parser: p.clone(), last_used: Instant::now() });
                    p
                }
            }
        };
        self.watch();
        parser.parse(img, hints, &lightcraft_parsing::Options::default(), cancelled).map_err(|e| e.to_string())
    }

    #[cfg(target_arch = "wasm32")]
    fn run_models(&self, _: ModelSet, _: &Path, _: &RgbImage, _: &[FaceHint], _: &dyn Fn() -> bool) -> Result<Parsing, String> {
        Err("the web build has no people parsing".into())
    }

    /// Drop the loaded models once they have been idle for [`KEEP_LOADED`] (one watcher thread at a time).
    #[cfg(not(target_arch = "wasm32"))]
    fn watch(&self) {
        if self.inner.watcher.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let spawned = std::thread::Builder::new().name("parsing-idle".into()).spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(10));
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
}

/// `p` with its Entire Person edges drawn by BiRefNet Matting from `img` (the picture it was parsed from).
fn hair_edged(
    edges: &crate::hair_edges::HairEdges,
    edge: crate::hair_edges::Edge,
    mut p: Parsing,
    img: &RgbImage,
    cancel: &Arc<AtomicBool>,
) -> Result<Parsing, String> {
    if p.people.is_empty() {
        return Ok(p);
    }
    let kind = crate::hair_edges::EdgeKind::People;
    let matte = edges.matte_for(edge, kind, &img.data, img.width, img.height, cancel).map_err(|e| format!("hair edges: {e}"))?;
    crate::hair_edges::people::refine(&mut p, &matte);
    Ok(p)
}

fn read_choice(path: &Path) -> Option<ModelSet> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    ModelSet::from_id(v.get("set")?.as_str()?)
}

/// Where the models are: a non-empty `LIGHTCRAFT_PARSING_MODELS`; else on the Mac the folder shared with the other
/// ArtCraft apps; else `<config>/models`.
pub fn default_models_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("LIGHTCRAFT_PARSING_MODELS").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    crate::shared_models::default_shared_models_dir().or_else(|| crate::config::config_dir().map(|c| c.join("models")))
}

impl crate::Session {
    /// Use the default models folder, cache folder and choice file for people parsing. Hosts (the app, MCP) ask for
    /// it; a bare session has none, so tests stay hermetic.
    pub fn with_default_parsing_models(self) -> Self {
        self.parsing.set_paths(
            default_models_dir(),
            crate::config::cache_dir().map(|d| d.join("people-parsing")),
            crate::config::config_dir().map(|d| d.join("parsing-model.json")),
        );
        // (the hair edges share the models folder; `fork/hair-edges.md`)
        let models = std::env::var_os("LIGHTCRAFT_BIREFNET_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(default_models_dir);
        self.hair_edges.set_paths(models, crate::config::config_dir().map(|d| d.join(crate::hair_edges::CHOICE_FILE)));
        self
    }
}
