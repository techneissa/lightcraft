//! Fork feature (`fork/hair-edges.md`, issue #103): hair-accurate edges for people and Subject masks.
//!
//! BiRefNet-matting (the BiRefNet network with weights trained for soft alpha mattes) draws the edge of two kinds of
//! mask, each with its own setting ([`EdgeKind`], [`Edge`]): Entire Person and Background from people parsing, and
//! Subject (with the Background made from it). Current, the default, keeps the masks exactly as they were.
//!
//! [`HairEdges`] keeps the choice (`<config>/mask-edges.json`), the model (loaded once, shared by the Subject worker
//! and people parsing, unloaded when idle), its download (asked first, size and SHA-256) and the newest mattes by the
//! pixels they were made from. A matte is made on the caller's worker thread with a row in the activity stack.
//!
//! Never crashes: a missing, short or damaged model file, a cancelled run and a panic inside the run are messages.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::activity::{Activity, Cancel};

pub mod people;
pub mod vision;

/// The newest mattes kept in memory (4 MB each).
const IN_MEMORY: usize = 4;
/// How long the model stays loaded after its last use.
#[cfg(feature = "sam")]
const KEEP_LOADED: std::time::Duration = std::time::Duration::from_secs(120);
/// The folder of the model under the models folder.
pub const MODEL_DIR: &str = "birefnet-matting";
/// The model file's size, in bytes (pinned with its SHA-256 in `lightcraft-birefnet`'s `Variant::Matting`).
pub const MODEL_BYTES: u64 = 884_878_856;
/// Changes when the way a matte is put into a mask changes, so cached people parses made the old way are redone.
pub const VERSION: &str = "matting1";
/// Where the choice is kept, inside the config folder.
pub const CHOICE_FILE: &str = "mask-edges.json";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What draws a mask's edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Edge {
    /// The edge the mask's own model draws (as before this feature).
    #[default]
    Current,
    /// BiRefNet-matting redraws the edge, so flyaway hair stays in.
    Matting,
    /// Apple Vision's own person and subject masks redraw the edge (`vision`, issue #115).
    Vision,
    // Another edge model plugs in here: a variant with its id, label and `suffix`, its `HairEdges::available` arm and
    // its `HairEdges::make` arm, which returns a `Matte` (logits over the picture stretched to a square). Everything
    // after that (people, Subject, the cache, Settings, `maskEdges.*`) takes any edge.
}

impl Edge {
    pub const ALL: [Edge; 3] = [Edge::Current, Edge::Matting, Edge::Vision];

    pub fn id(self) -> &'static str {
        match self {
            Edge::Current => "current",
            Edge::Matting => "birefnet-matting",
            Edge::Vision => "apple-vision",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Edge::Current => "Current",
            Edge::Matting => "BiRefNet Matting",
            Edge::Vision => "Apple Vision",
        }
    }

    pub fn parse(id: &str) -> Option<Edge> {
        Edge::ALL.into_iter().find(|e| e.id() == id.trim())
    }

    /// What a Subject part whose edge this drew adds to its model id (`SegMask::model`).
    pub fn suffix(self) -> &'static str {
        match self {
            Edge::Current => "",
            Edge::Matting => "+matting",
            Edge::Vision => vision::SUFFIX,
        }
    }
}

/// The kinds of mask whose edge can be chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Entire Person and Background from people parsing.
    People,
    /// Subject, and the Background made from it.
    Subject,
}

impl EdgeKind {
    pub const ALL: [EdgeKind; 2] = [EdgeKind::People, EdgeKind::Subject];

    pub fn id(self) -> &'static str {
        match self {
            EdgeKind::People => "people",
            EdgeKind::Subject => "subject",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::People => "Entire Person / Background",
            EdgeKind::Subject => "Subject",
        }
    }

    pub fn parse(id: &str) -> Option<EdgeKind> {
        EdgeKind::ALL.into_iter().find(|k| k.id() == id.trim())
    }
}

/// A matte: `side × side` logits over the photo stretched to a square (row `y`, column `x` covers
/// `((x + 0.5) / side, (y + 0.5) / side)`).
#[derive(Clone, Debug, PartialEq)]
pub struct Matte {
    pub side: usize,
    pub logits: Vec<f32>,
}

impl Matte {
    /// Whether the sizes add up and every value is a number.
    pub fn is_valid(&self) -> bool {
        self.side > 0 && self.side.checked_mul(self.side) == Some(self.logits.len()) && self.logits.iter().all(|v| v.is_finite())
    }

    /// The coverage (0 to 1) at normalized point `(x, y)`, bilinear over the cells.
    pub fn coverage(&self, x: f32, y: f32) -> f32 {
        let n = self.side;
        if n == 0 || self.logits.len() < n * n {
            return 0.0;
        }
        let fx = (x * n as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let fy = (y * n as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| self.logits.get(y * n + x).copied().unwrap_or(-16.0);
        let l = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty) + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
        1.0 / (1.0 + (-l).exp())
    }
}

/// What makes a matte: the model normally; tests put a fake one in. Gets the 8-bit RGB picture, its size, and whether
/// to stop.
pub type Runner = Arc<dyn Fn(&[u8], usize, usize, &dyn Fn() -> bool) -> Result<Matte, String> + Send + Sync>;

#[derive(Clone, Copy, Default)]
struct Choice {
    people: Edge,
    subject: Edge,
}

#[derive(Default)]
struct Paths {
    models: Option<PathBuf>,
    choice: Option<PathBuf>,
}

#[cfg(feature = "sam")]
struct Loaded {
    model: lightcraft_birefnet::BiRefNet,
    last_used: std::time::Instant,
}

struct Inner {
    activity: Activity,
    paths: Mutex<Paths>,
    choice: Mutex<Choice>,
    mattes: Mutex<VecDeque<(u64, Arc<Matte>)>>,
    /// One run at a time: the model's GPU buffers are safe with one graph in flight only.
    one_run: Mutex<()>,
    runner: Mutex<Option<Runner>>,
    runs: AtomicUsize,
    #[cfg(feature = "sam")]
    loaded: Mutex<Option<Loaded>>,
    #[cfg(feature = "sam")]
    watcher: AtomicBool,
    #[cfg(feature = "sam")]
    download: crate::segment::download::Downloader,
    /// The GPU failed once: run on the CPU from now on ([`on_devices`]).
    gpu_refused: AtomicBool,
    /// The kind whose BiRefNet Matting was asked for with the download: chosen when the download has finished.
    pending: Mutex<Option<EdgeKind>>,
}

/// The hair edges of a session. Cheap to clone; every clone is the same.
#[derive(Clone)]
pub struct HairEdges {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for HairEdges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HairEdges").field("models", &self.models_dir()).finish()
    }
}

impl HairEdges {
    /// No models folder and no saved choice (so the model is not installed and both edges are Current); rows go to
    /// `activity`.
    pub fn new(activity: Activity) -> Self {
        Self {
            inner: Arc::new(Inner {
                activity,
                paths: Mutex::new(Paths::default()),
                choice: Mutex::new(Choice::default()),
                mattes: Mutex::new(VecDeque::new()),
                one_run: Mutex::new(()),
                runner: Mutex::new(None),
                runs: AtomicUsize::new(0),
                #[cfg(feature = "sam")]
                loaded: Mutex::new(None),
                #[cfg(feature = "sam")]
                watcher: AtomicBool::new(false),
                #[cfg(feature = "sam")]
                download: Default::default(),
                gpu_refused: AtomicBool::new(false),
                pending: Mutex::new(None),
            }),
        }
    }

    /// Use `models` (the model is `<models>/birefnet-matting/model.safetensors`) and the choice kept in `choice`.
    pub fn set_paths(&self, models: Option<PathBuf>, choice: Option<PathBuf>) {
        let saved = choice.as_deref().map(read_choice).unwrap_or_default();
        *lock(&self.inner.paths) = Paths { models, choice };
        *lock(&self.inner.choice) = saved;
        self.inner.gpu_refused.store(false, Ordering::Relaxed);
        #[cfg(feature = "sam")]
        {
            *lock(&self.inner.loaded) = None;
        }
    }

    pub fn models_dir(&self) -> Option<PathBuf> {
        lock(&self.inner.paths).models.clone()
    }

    /// `<models>/birefnet-matting`.
    pub fn model_dir(&self) -> Option<PathBuf> {
        self.models_dir().map(|d| d.join(MODEL_DIR))
    }

    /// Whether this build can run the model at all.
    pub const AVAILABLE: bool = cfg!(feature = "sam");

    /// Whether the model file is in place (or a test runner stands in for it).
    pub fn installed(&self) -> bool {
        if lock(&self.inner.runner).is_some() {
            return true;
        }
        // (a short or damaged download is not installed: Settings offers the download again)
        Self::AVAILABLE
            && self
                .model_dir()
                .and_then(|d| std::fs::metadata(d.join("model.safetensors")).ok())
                .is_some_and(|m| m.is_file() && m.len() == MODEL_BYTES)
    }

    /// The edge chosen for `kind` (saved; Current until the user picks).
    pub fn chosen(&self, kind: EdgeKind) -> Edge {
        let c = *lock(&self.inner.choice);
        match kind {
            EdgeKind::People => c.people,
            EdgeKind::Subject => c.subject,
        }
    }

    /// Whether `edge` can draw `kind` masks here (BiRefNet Matting: once downloaded; Apple Vision: on macOS 14 or
    /// later; the same for both kinds).
    pub fn available(&self, edge: Edge, _kind: EdgeKind) -> bool {
        match edge {
            Edge::Current => true,
            Edge::Matting => self.installed(),
            Edge::Vision => vision::available(),
        }
    }

    /// The edge `kind` masks get now: the choice, or Current when the model it needs is not here.
    pub fn edge(&self, kind: EdgeKind) -> Edge {
        let e = self.chosen(kind);
        if self.available(e, kind) { e } else { Edge::Current }
    }

    /// Choose the edge of `kind` masks from now on (kept in the choice file). BiRefNet Matting needs the model.
    pub fn set_edge(&self, kind: EdgeKind, edge: Edge) -> Result<(), String> {
        if !self.available(edge, kind) {
            return Err(match edge {
                Edge::Matting if Self::AVAILABLE => {
                    format!("BiRefNet Matting is not downloaded yet ({} MB; maskEdges.download)", MODEL_BYTES.div_ceil(1_000_000))
                }
                Edge::Vision => "Apple Vision masks need macOS 14 or later on a Mac".into(),
                _ => format!("{} is not available in this build", edge.label()),
            });
        }
        let path = lock(&self.inner.paths).choice.clone();
        // the new choice is written while the lock is held, and kept only once it is written: two calls at once
        // never interleave, and a failed write leaves the choice as it was
        let mut c = lock(&self.inner.choice);
        let mut next = *c;
        match kind {
            EdgeKind::People => next.people = edge,
            EdgeKind::Subject => next.subject = edge,
        }
        if let Some(path) = path {
            write_choice(&path, next)?;
        }
        *c = next;
        Ok(())
    }

    /// Choose BiRefNet Matting for `kind` once the download that is running finishes (`maskEdges.download {kind}`):
    /// a small thread watches the download and applies the choice when it ends, whether or not Settings is open. A
    /// download that fails or stops chooses nothing.
    pub fn choose_when_downloaded(&self, kind: EdgeKind) {
        *lock(&self.inner.pending) = Some(kind);
        let weak = Arc::downgrade(&self.inner);
        let spawned = std::thread::Builder::new().name("matting-choice".into()).spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let Some(inner) = weak.upgrade() else { return };
                let this = HairEdges { inner };
                if !this.download_status().running {
                    this.settle_download();
                    return;
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("hair edges: could not watch the download ({e}); choose BiRefNet Matting once it has finished");
        }
    }

    /// Apply the choice waiting for the download, if it has finished: `Some(kind)` when it was chosen now.
    pub fn settle_download(&self) -> Option<EdgeKind> {
        if self.download_status().running {
            return None;
        }
        let kind = lock(&self.inner.pending).take()?;
        self.installed().then(|| self.set_edge(kind, Edge::Matting).ok()).flatten().map(|()| kind)
    }

    /// How many times a matte was made by the model (not the cache).
    pub fn runs(&self) -> usize {
        self.inner.runs.load(Ordering::Relaxed)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_runner(&self, runner: Option<Runner>) {
        *lock(&self.inner.runner) = runner;
    }

    fn remembered(&self, key: u64) -> Option<Arc<Matte>> {
        lock(&self.inner.mattes).iter().find(|e| e.0 == key).map(|e| e.1.clone())
    }

    fn remember(&self, key: u64, m: Arc<Matte>) {
        let mut all = lock(&self.inner.mattes);
        all.retain(|e| e.0 != key);
        all.push_back((key, m));
        while all.len() > IN_MEMORY {
            all.pop_front();
        }
    }

    /// BiRefNet Matting's matte of an 8-bit RGB picture ([`HairEdges::matte_for`]).
    pub fn matte(&self, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Arc<Matte>, String> {
        self.matte_for(Edge::Matting, EdgeKind::People, rgb, w, h, cancel)
    }

    /// The matte `edge` draws for `kind` masks of an 8-bit RGB picture (`w × h`, row-major): from memory when the same
    /// pixels were matted the same way before, else made on this thread, with a row "Refining hair edges" in the
    /// activity stack whose ✕ sets `cancel`.
    pub fn matte_for(&self, edge: Edge, kind: EdgeKind, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Arc<Matte>, String> {
        if w == 0 || h == 0 || w.checked_mul(h).and_then(|n| n.checked_mul(3)) != Some(rgb.len()) {
            return Err("the picture to refine is damaged".into());
        }
        if edge == Edge::Current {
            return Err("the Current edge has no matte".into());
        }
        if edge == Edge::Vision {
            // (Vision's mattes are made in `vision`, off the GPU run lock and with their own memory)
            return Err("Apple Vision's edge is made by hair_edges::vision".into());
        }
        // (BiRefNet's matte is the same for both kinds; an edge model whose matte depends on the kind mixes it in here)
        let key = pixel_key(rgb, w, h) ^ (edge as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        if let Some(m) = self.remembered(key) {
            return Ok(m);
        }
        if !self.available(edge, kind) {
            // (another edge model says here what it lacks)
            return Err(format!("{} is not downloaded ({} MB; maskEdges.download)", edge.label(), MODEL_BYTES.div_ceil(1_000_000)));
        }
        let row = self.inner.activity.start("matting", "Refining hair edges", Cancel::Flag(cancel.clone()));
        row.detail(edge.label());
        let _one_at_a_time = lock(&self.inner.one_run);
        // (one model on the GPU at a time, people parsing and LaMa included: `crate::gpu_run`)
        if let Some(text) = crate::gpu_run::waiting_text() {
            // (a Glare Repair repaint holds the GPU for minutes: say so instead of looking stuck)
            row.detail(&text);
        }
        let _gpu = crate::gpu_run::turn();
        row.detail(edge.label());
        // (another caller may have made it while this one waited)
        if let Some(m) = self.remembered(key) {
            return Ok(m);
        }
        let stop = || cancel.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down();
        if stop() {
            return Err("cancelled".into());
        }
        self.inner.runs.fetch_add(1, Ordering::Relaxed);
        let t = web_time::Instant::now();
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.make(edge, kind, rgb, w, h, cancel)));
        let m = match ran {
            Ok(r) => r?,
            Err(_) => {
                #[cfg(feature = "sam")]
                {
                    *lock(&self.inner.loaded) = None;
                }
                return Err(format!("{} stopped unexpectedly", edge.label()));
            }
        };
        if !m.is_valid() {
            return Err(format!("{} produced an invalid matte", edge.label()));
        }
        log::info!("{}: {w}×{h} in {:?}", edge.label(), t.elapsed());
        let m = Arc::new(m);
        self.remember(key, m.clone());
        Ok(m)
    }

    /// Make `edge`'s matte for `kind` (another edge model adds its arm here).
    fn make(&self, edge: Edge, _kind: EdgeKind, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Matte, String> {
        match edge {
            Edge::Matting => self.run(rgb, w, h, cancel),
            Edge::Current => Err("the Current edge has no matte".into()),
            Edge::Vision => Err("Apple Vision's edge is made by hair_edges::vision".into()),
        }
    }

    fn run(&self, rgb: &[u8], w: usize, h: usize, cancel: &Arc<AtomicBool>) -> Result<Matte, String> {
        let runner = lock(&self.inner.runner).clone();
        if let Some(r) = runner {
            return r(rgb, w, h, &|| cancel.load(Ordering::Relaxed));
        }
        self.run_model(rgb, w, h, cancel)
    }

    #[cfg(not(feature = "sam"))]
    fn run_model(&self, _: &[u8], _: usize, _: usize, _: &Arc<AtomicBool>) -> Result<Matte, String> {
        Err("BiRefNet Matting is not available in this build".into())
    }
}

#[cfg(feature = "sam")]
mod model;

/// How one attempt to make a matte on one device ended.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) enum Attempt {
    Done(Matte),
    /// The GPU could not load or run the model: try the CPU.
    Gpu(String),
    /// An error that another device would not fix (a damaged file, cancelled).
    Fail(String),
}

/// Run `attempt` on the GPU, then on the CPU when the GPU fails or panics (`cpu` says which). A panic is caught for
/// each device on its own, so a Metal kernel panic still reaches the CPU attempt; a panic on the CPU is the final
/// error. Once the GPU has failed, `gpu_refused` is set and later calls go straight to the CPU (the 885 MB model is
/// not loaded onto the GPU again for every photo).
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) fn on_devices(gpu_refused: &AtomicBool, mut attempt: impl FnMut(bool) -> Attempt) -> Result<Matte, String> {
    let devices: &[bool] = if gpu_refused.load(Ordering::Relaxed) { &[true] } else { &[false, true] };
    for &cpu in devices {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| attempt(cpu))) {
            Ok(Attempt::Done(m)) => return Ok(m),
            Ok(Attempt::Fail(e)) => return Err(e),
            Ok(Attempt::Gpu(e)) if !cpu => {
                log::warn!("BiRefNet Matting failed on the GPU ({e}); using the CPU from now on");
                gpu_refused.store(true, Ordering::Relaxed);
            }
            Ok(Attempt::Gpu(e)) => return Err(e),
            Err(_) if !cpu => {
                log::warn!("BiRefNet Matting stopped unexpectedly on the GPU; using the CPU from now on");
                gpu_refused.store(true, Ordering::Relaxed);
            }
            Err(_) => return Err("BiRefNet Matting stopped unexpectedly".into()),
        }
    }
    Err("BiRefNet Matting could not run on this Mac".into())
}

/// A Subject selection `base` with its edge redrawn by the matte `m`: `base` decides away from its edge, the matte
/// within a band around it (`lightcraft_birefnet::combine`). Marked `<base model>+matting`; `None` when it then holds
/// nothing or `base` is damaged.
#[cfg(feature = "sam")]
pub fn subject_edge(base: &lightcraft_develop::SegMask, m: &Matte, edge: Edge) -> Option<lightcraft_develop::SegMask> {
    let logits = base.logits()?;
    let matte = lightcraft_birefnet::Matte { side: m.side, logits: m.logits.clone() };
    let mixed = lightcraft_birefnet::combine::refine_edges(&logits, base.side as usize, &matte);
    if !mixed.iter().any(|l| *l > 0.0) {
        return None;
    }
    let id = format!("{}{}", base.model.as_deref().unwrap_or("sam3"), edge.suffix());
    Some(lightcraft_develop::SegMask::from_logits(m.side, &mixed).made_by(&id))
}

#[cfg(not(feature = "sam"))]
impl HairEdges {
    pub fn download_status(&self) -> crate::segment::DownloadStatus {
        crate::segment::DownloadStatus::default()
    }

    pub fn start_download(&self, _: &Activity) -> Result<bool, String> {
        Err("BiRefNet Matting is not available in this build".into())
    }

    pub fn cancel_download(&self) -> bool {
        false
    }
}

/// A hash of the picture's size and pixels: equal pictures share a matte.
fn pixel_key(rgb: &[u8], w: usize, h: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut s = std::collections::hash_map::DefaultHasher::new();
    (w, h).hash(&mut s);
    rgb.hash(&mut s);
    s.finish()
}

/// The saved choice; a missing or damaged file (or an unknown edge) is Current.
fn read_choice(path: &Path) -> Choice {
    let v: Option<serde_json::Value> = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok());
    let edge = |k: EdgeKind| v.as_ref().and_then(|v| v.get(k.id())).and_then(serde_json::Value::as_str).and_then(Edge::parse).unwrap_or_default();
    Choice { people: edge(EdgeKind::People), subject: edge(EdgeKind::Subject) }
}

/// Write the choice (replacing the file whole, so a crash never leaves half of it).
fn write_choice(path: &Path, c: Choice) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not keep the choice: {e}"))?;
    }
    let tmp = path.with_extension("json.part");
    let body = serde_json::json!({ EdgeKind::People.id(): c.people.id(), EdgeKind::Subject.id(): c.subject.id() }).to_string();
    std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| format!("could not keep the choice: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_the_default_is_current() {
        for e in Edge::ALL {
            assert_eq!(Edge::parse(e.id()), Some(e));
        }
        for k in EdgeKind::ALL {
            assert_eq!(EdgeKind::parse(k.id()), Some(k));
        }
        assert_eq!(Edge::default(), Edge::Current);
        assert_eq!(Edge::parse("matting"), None);
    }

    #[test]
    fn a_damaged_choice_file_is_current() {
        let dir = std::env::temp_dir().join(format!("lc-hair-choice-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join(CHOICE_FILE);
        for text in ["", "{", "[]", r#"{"people": 3}"#, r#"{"people": "lasso"}"#] {
            std::fs::write(&f, text).unwrap();
            let c = read_choice(&f);
            assert_eq!((c.people, c.subject), (Edge::Current, Edge::Current), "{text:?}");
        }
        write_choice(&f, Choice { people: Edge::Matting, subject: Edge::Current }).unwrap();
        assert_eq!(read_choice(&f).people, Edge::Matting);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn coverage_reads_the_matte_and_survives_bad_sizes() {
        let m = Matte { side: 2, logits: vec![10.0, -10.0, 10.0, -10.0] };
        assert!(m.coverage(0.0, 0.5) > 0.99 && m.coverage(1.0, 0.5) < 0.01);
        assert_eq!(Matte { side: 3, logits: vec![] }.coverage(0.5, 0.5), 0.0);
        assert!(!Matte { side: 2, logits: vec![f32::NAN; 4] }.is_valid());
        assert!(!Matte { side: 0, logits: vec![] }.is_valid());
    }
}
