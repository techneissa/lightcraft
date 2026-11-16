//! Fork (issue #288, `fork/relight.md`): each photo's Relight field, made once and kept.
//!
//! A photo's depth is its file's own depth map when it has one (an iPhone Portrait HEIC, read as Portrait ▸
//! Background reads it), else Depth Anything 3 Mono Large's (`lightcraft-depth`, in the shared models folder, on the
//! GPU through Metal). The field ([`lightcraft_relight::Field`]) is that depth brought to a guide picture of the
//! photo ([`FIELD_EDGE`] on its long side, as it was shot: no edits but its rotation, before the crop and the lens
//! corrections) with its normals and edges.
//!
//! It is made off the UI thread, by one worker taking the newest request first ([`QUEUE`] wait at most), with a row
//! in the activity stack, the first time a render of a photo with Relight on asks for it. It is kept in memory by the
//! photo's content and rotation while it is among the most recently used ([`BUDGET`] bytes): no edit of the photo
//! makes it again, but a photo out of that set, or any photo after a restart, finds its depth again. Renders get it
//! in their `SourceInfo` once it is there, so their keys change and the photo is drawn again; until then they render
//! without the light. An export (and a synchronous render: the CLI, MCP) waits for it on its own thread, or makes it
//! there. A finding that is cancelled is not tried again by renders; the section's Find Depth (`relight.findDepth`)
//! or an export tries it again. On the web nothing is made: Relight needs the desktop app.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use lightcraft_catalog::{Photo, PhotoId};
use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::relight::Input;
use lightcraft_raster::{Rgb32f, Rgba8};
use lightcraft_relight::{DepthMap, Field, Kind};
use serde_json::{Value, json};

use crate::Session;

/// The guide's long side (pixels): the field's size.
pub const FIELD_EDGE: usize = 1600;
/// Memory the kept fields may hold (bytes; a 1600-pixel field holds about 42 MB, so about seven are kept). The
/// newest is always kept.
pub const BUDGET: usize = 300 << 20;
/// Photos waiting for their depth at most (a grid of pasted settings asks for many at once): past it the oldest
/// request is dropped, and a later render of that photo asks again.
pub const QUEUE: usize = 12;
/// The long side the depth model reads the photo at.
pub const MODEL_LONG: usize = 504;
/// The folder of the depth model under the models folder.
pub const MODEL_DIR: &str = "da3-mono-large";
/// The depth box's picture (pixels on its long side).
pub const THUMB: usize = 240;
/// What Relight says on the web build.
pub const NO_WEB: &str = "Relight needs the desktop app";
/// How long an export waits for a field another job is making before it gives up (a guard against a hang).
const WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// Where a field's depth came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The photo file's own depth map.
    File,
    /// The depth model.
    Model,
}

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Source::File => "file",
            Source::Model => "model",
        }
    }
}

/// A made field and what the section shows of it.
pub struct Made {
    pub input: Input,
    pub source: Source,
    /// The depth box's picture: near light, far dark, 8-bit, `w × h`.
    pub thumb: (usize, usize, Vec<u8>),
}

impl Made {
    /// Memory held (bytes).
    fn bytes(&self) -> usize {
        self.input.field.bytes() + self.thumb.2.len()
    }
}

/// A photo waiting for its field.
struct Queued {
    key: String,
    maker: Maker,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    label: String,
}

#[derive(Default)]
struct State {
    /// Most recently used first.
    made: Vec<(String, Arc<Made>)>,
    /// Being made or waiting in `queue`.
    busy: Vec<String>,
    failed: Vec<(String, String)>,
    /// Cancelled from the activity stack: renders don't start it again.
    cancelled: Vec<String>,
    /// Newest last.
    queue: std::collections::VecDeque<Queued>,
    /// Whether the worker runs.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    working: bool,
    /// Tests: a smaller [`BUDGET`].
    #[cfg(test)]
    budget: Option<usize>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    done: Condvar,
    /// The depth model, loaded at the first photo that needs it and kept (one job runs it at a time).
    #[cfg(feature = "sam")]
    model: Mutex<Option<lightcraft_depth::DepthModel>>,
    #[cfg(feature = "sam")]
    download: crate::segment::download::Downloader,
}

/// Every photo's Relight field (shared by the session and its workers).
#[derive(Clone, Default)]
pub struct RelightStore {
    shared: Arc<Shared>,
    /// The folder holding the model's folder (`ArtCraft/models` on the Mac); `None`: no model (fields come only from
    /// files' depth maps).
    pub root: Option<PathBuf>,
    /// The session is setting up a guide render ([`Session::relight_maker`]): that render asks for no field.
    building: bool,
    /// The session is setting up an export's render ([`Session::relight_held`]): it takes a kept field and starts
    /// none (the export's worker makes it).
    hold: bool,
    /// Tests: a flat depth in place of the model's.
    #[cfg(test)]
    fake_model: bool,
}

/// The models folder of the desktop app and the headless hosts (as BiRefNet's: `ArtCraft/models` on the Mac,
/// shared with PhotoCraft, else `<config>/models`).
pub fn default_root() -> Option<PathBuf> {
    crate::shared_models::default_shared_models_dir().or_else(|| crate::config::config_dir().map(|c| c.join("models")))
}

impl RelightStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Field `key`, when it is kept (it becomes the most recently used).
    fn get(&self, key: &str) -> Option<Arc<Made>> {
        let mut st = self.lock();
        let i = st.made.iter().position(|(k, _)| k == key)?;
        let hit = st.made.remove(i);
        let m = hit.1.clone();
        st.made.insert(0, hit);
        Some(m)
    }

    /// Keep field `key`, or why it could not be made.
    fn put(&self, key: &str, result: Result<Made, String>) {
        let mut st = self.lock();
        st.busy.retain(|k| k != key);
        st.failed.retain(|(k, _)| k != key);
        st.cancelled.retain(|k| k != key);
        match result {
            Ok(m) => {
                st.made.retain(|(k, _)| k != key);
                st.made.insert(0, (key.to_string(), Arc::new(m)));
                #[cfg(test)]
                let budget = st.budget.unwrap_or(BUDGET);
                #[cfg(not(test))]
                let budget = BUDGET;
                let mut used = 0usize;
                let keep = st.made.iter().take_while(|(_, m)| {
                    used = used.saturating_add(m.bytes());
                    used <= budget
                });
                let keep = keep.count().max(1);
                st.made.truncate(keep);
            }
            Err(e) => st.failed.push((key.to_string(), e)),
        }
        drop(st);
        self.shared.done.notify_all();
    }

    /// Field `key` was cancelled: nothing is kept, and renders don't start it again.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn cancelled(&self, key: &str) {
        let mut st = self.lock();
        st.busy.retain(|k| k != key);
        if !st.cancelled.iter().any(|k| k == key) {
            st.cancelled.push(key.to_string());
        }
        drop(st);
        self.shared.done.notify_all();
    }

    /// Wait (on a worker) while field `key` is being made. `Ok(None)`: it was not made and did not fail (cancelled,
    /// or dropped from the queue); `Err`: why it could not be made.
    fn wait(&self, key: &str) -> Result<Option<Arc<Made>>, String> {
        let start = std::time::Instant::now();
        let mut st = self.lock();
        while st.busy.iter().any(|k| k == key) {
            let left = WAIT.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Err("finding the photo's depth took too long".into());
            }
            st = self.shared.done.wait_timeout(st, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        if let Some((_, m)) = st.made.iter().find(|(k, _)| k == key) {
            return Ok(Some(m.clone()));
        }
        match st.failed.iter().find(|(k, _)| k == key) {
            Some((_, e)) => Err(e.clone()),
            None => Ok(None),
        }
    }

    /// Field `key`, made here on this thread with `maker` unless it is kept or being made (then waited for). One
    /// waiting in the queue is taken out of it and made here.
    fn make_here(&self, key: &str, maker: Option<Maker>) -> Result<Arc<Made>, String> {
        if let Some(m) = self.get(key) {
            return Ok(m);
        }
        let queued = {
            let mut st = self.lock();
            let i = st.queue.iter().position(|q| q.key == key);
            i.and_then(|i| st.queue.remove(i)).map(|q| q.maker)
        };
        if queued.is_none()
            && self.lock().busy.iter().any(|k| k == key)
            && let Some(m) = self.wait(key)?
        {
            return Ok(m);
        }
        let maker = queued.or(maker).ok_or("the photo's depth can't be found")?;
        {
            let mut st = self.lock();
            if !st.busy.iter().any(|k| k == key) {
                st.busy.push(key.to_string());
            }
        }
        let r = crate::guard::catch("the photo's depth", || maker.make(self, None)).and_then(|r| r);
        let err = r.as_ref().err().cloned();
        self.put(key, r);
        if let Some(e) = err {
            return Err(e);
        }
        self.lock().made.iter().find(|(k, _)| k == key).map(|(_, m)| m.clone()).ok_or_else(|| "the photo's depth was not kept".into())
    }

    /// Queue field `key`'s making (newest first) and start the worker when it is not running.
    #[cfg(not(target_arch = "wasm32"))]
    fn enqueue(&self, q: Queued, activity: &crate::activity::Activity) {
        let mut st = self.lock();
        st.busy.push(q.key.clone());
        st.queue.push_back(q);
        while st.queue.len() > QUEUE {
            if let Some(old) = st.queue.pop_front() {
                st.busy.retain(|k| *k != old.key);
            }
        }
        if st.working {
            return;
        }
        st.working = true;
        drop(st);
        let (store, activity) = (self.clone(), activity.clone());
        let spawned = std::thread::Builder::new().name("relight-depth".into()).spawn(move || store.work(&activity));
        if let Err(e) = spawned {
            let mut st = self.lock();
            st.working = false;
            let keys: Vec<String> = st.queue.drain(..).map(|q| q.key).collect();
            drop(st);
            for k in keys {
                self.put(&k, Err(format!("could not start finding the photo's depth: {e}")));
            }
        }
        self.shared.done.notify_all();
    }

    /// The worker: makes the queued fields, newest first, until none waits.
    #[cfg(not(target_arch = "wasm32"))]
    fn work(&self, activity: &crate::activity::Activity) {
        loop {
            let q = {
                let mut st = self.lock();
                match st.queue.pop_back() {
                    Some(q) => q,
                    None => {
                        st.working = false;
                        return;
                    }
                }
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let guard = activity.start_background("relight.depth", "Finding depth", crate::activity::Cancel::Flag(cancel.clone()));
            guard.detail(&q.label);
            let c = cancel.clone();
            let r = crate::guard::catch("the photo's depth", || q.maker.make(self, Some(c))).and_then(|r| r);
            match r {
                Err(_) if cancel.load(std::sync::atomic::Ordering::Relaxed) => self.cancelled(&q.key),
                r => {
                    if let Err(e) = &r {
                        log::warn!("relight: {e}");
                    }
                    self.put(&q.key, r);
                }
            }
            drop(guard);
        }
    }

    /// The model's folder (`<root>/da3-mono-large`).
    pub fn model_dir(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join(MODEL_DIR))
    }

    /// Whether the depth model is in place (and this build runs it).
    pub fn model_installed(&self) -> bool {
        #[cfg(feature = "sam")]
        if let Some(dir) = self.model_dir() {
            return lightcraft_depth::is_model_dir(&dir);
        }
        false
    }

    /// Forget every failure (the model was installed: photos are tried again).
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    fn retry_failed(&self) {
        self.lock().failed.clear();
    }
}

/// What makes a photo's field: the guide render, the file's depth map (when it may have one) and the photo's
/// rotation. Set up on the session's thread, run anywhere.
pub(crate) struct Maker {
    key: String,
    guide: Box<crate::RenderJob>,
    file: Option<String>,
    orientation: lightcraft_geom::Orientation,
}

/// 8-bit sRGB → linear Rec.2020.
fn srgb8_to_working(img: &Rgba8) -> Rgb32f {
    let m = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020);
    let lin = img.to_linear();
    lin.map(|p| m.apply_f32(p))
}

/// The depth map in the file at `path` turned as the photo is (`None`: it has none, or it can't be read).
fn file_depth(path: &str, orientation: lightcraft_geom::Orientation) -> Option<lightcraft_raster::Plane> {
    let bytes = std::fs::read(path).ok()?;
    let (w, h, data) = lightcraft_codecs::decode_depth(&bytes)?;
    let plane = lightcraft_raster::Plane { width: w, height: h, data };
    (w > 1 && h > 1 && plane.data.len() == w.saturating_mul(h)).then(|| plane.into_oriented(orientation))
}

impl Maker {
    /// Make the field with `store`'s model (this takes seconds: run it on a worker). `cancel` stops the depth model
    /// between its blocks. On the web it is refused (the guide render and the model would run on the page's thread).
    fn make(self, store: &RelightStore, cancel: Option<Arc<AtomicBool>>) -> Result<Made, String> {
        if cfg!(target_arch = "wasm32") {
            return Err(NO_WEB.into());
        }
        let Maker { key, guide, file, orientation } = self;
        let r = guide.run();
        let img = r.rendered.map_err(|e| format!("the photo could not be read for its depth: {e}"))?.image;
        let guide = srgb8_to_working(&img);
        // the file's own depth map first (an iPhone Portrait photo), when it has the photo's shape
        if let Some(d) = file.as_deref().and_then(|p| file_depth(p, orientation)) {
            match Field::prepare(DepthMap { width: d.width, height: d.height, values: &d.data, kind: Kind::Disparity }, &guide) {
                Ok(field) => return Ok(made(&key, field, Source::File)),
                Err(e) => log::warn!("relight: the file's depth map is not used ({e}); the depth model is"),
            }
        }
        let depth = model_depth(store, &img, cancel)?;
        let field = Field::prepare(DepthMap { width: depth.width, height: depth.height, values: &depth.data, kind: Kind::Depth }, &guide)
            .map_err(|e| e.to_string())?;
        Ok(made(&key, field, Source::Model))
    }
}

/// The depth of `img` from the model in `store`'s folder (loaded once and kept).
#[cfg(feature = "sam")]
fn model_depth(store: &RelightStore, img: &Rgba8, cancel: Option<Arc<AtomicBool>>) -> Result<lightcraft_raster::Plane, String> {
    #[cfg(test)]
    if store.fake_model {
        return Ok(tests::fake_depth(img));
    }
    let dir = store.model_dir().filter(|d| lightcraft_depth::is_model_dir(d)).ok_or(MISSING)?;
    let mut slot = store.shared.model.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(lightcraft_depth::DepthModel::load(&dir).map_err(|e| e.to_string())?);
    }
    let model = slot.as_mut().ok_or("the depth model did not load")?;
    model.set_cancel(cancel);
    let rgb: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let e = model.estimate(&rgb, img.width, img.height, MODEL_LONG).map_err(|e| e.to_string())?;
    model.set_cancel(None);
    Ok(lightcraft_raster::Plane { width: e.width, height: e.height, data: e.depth })
}

#[cfg(not(feature = "sam"))]
fn model_depth(_store: &RelightStore, _img: &Rgba8, _cancel: Option<Arc<AtomicBool>>) -> Result<lightcraft_raster::Plane, String> {
    #[cfg(test)]
    if _store.fake_model {
        return Ok(tests::fake_depth(_img));
    }
    Err("this build has no depth model; only photos with their own depth map can be relit".into())
}

/// What a photo without a depth map of its own needs.
pub const MISSING: &str = "the depth model is not installed";

/// The salt of a render made while photo `id`'s field is not there (its keys change when it lands).
fn unknown_salt(id: PhotoId) -> u64 {
    0x5e1f_0000_0000_0001 ^ id.0.rotate_left(7)
}

fn made(key: &str, field: Field, source: Source) -> Made {
    let thumb = field.thumbnail(THUMB);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(key, &mut h);
    let k = std::hash::Hasher::finish(&h) | 1;
    Made { input: Input { field: Arc::new(field), key: k }, source, thumb }
}

/// An export's field ([`Session::relight_export`]): the kept one, held from when the export was set up (a batch's
/// other photos can't push it out), else made on the export's worker (or waited for when another job makes it).
pub struct ExportRelight {
    store: RelightStore,
    key: String,
    made: Option<Arc<Made>>,
    maker: Option<Maker>,
    label: String,
}

impl ExportRelight {
    /// Make (or wait for) the field and give it to `jobs`. Fails, naming the photo, when it can't be made (never a
    /// silent export without the light).
    pub fn fill(self, jobs: &mut [&mut crate::RenderJob]) -> Result<(), String> {
        let m = match self.made {
            Some(m) => m,
            None => self.store.make_here(&self.key, self.maker).map_err(|e| format!("{}: {e}", self.label))?,
        };
        for j in jobs.iter_mut() {
            j.info.relight = Some(m.input.clone());
        }
        Ok(())
    }
}

impl Session {
    /// The key photo `p`'s field is kept under: its content and rotation, and whether the model can be used (a photo
    /// tried without it is tried again once it is installed).
    fn relight_key(&self, p: &Photo) -> String {
        format!("{}|{:?}|{}", crate::media::content_key(p), p.develop.orientation, self.relight.model_installed())
    }

    /// Set up the making of photo `p`'s field.
    fn relight_maker(&mut self, p: &Photo) -> Result<Maker, String> {
        // (the guide render is of the photo with its light: it must not ask for the field it is the guide of)
        self.relight.building = true;
        let guide = self.render_job(p.id, FIELD_EDGE, FIELD_EDGE, false, false);
        self.relight.building = false;
        let mut guide = guide.ok_or("no such photo")?;
        // the photo as it was shot, turned as it is: no edit moves or changes what the depth is found on
        guide.settings = Arc::new(DevelopSettings { orientation: p.develop.orientation, ..DevelopSettings::default() });
        guide.info.retouch = None;
        guide.info.relight = None;
        guide.cache = None;
        guide.view_cache = None;
        let file = match &p.source {
            lightcraft_catalog::Source::File { path }
                if Path::new(path)
                    .extension()
                    .is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "heic" | "heif" | "hif")) =>
            {
                Some(path.clone())
            }
            _ => None,
        };
        Ok(Maker { key: self.relight_key(p), guide: Box::new(guide), file, orientation: p.develop.orientation })
    }

    /// What a render of `p` with settings `s` needs from Relight: the salt for its cache keys (0 while the light is
    /// off: every other render keeps its key) and the field for its `SourceInfo`. Starts making the field when the
    /// light is on and it is not there.
    pub(crate) fn relight_for(&mut self, p: &Photo, s: &DevelopSettings) -> (u64, Option<Input>) {
        if self.relight.building || lightcraft_pipeline::relight::light_of(s).is_none() {
            return (0, None);
        }
        let key = self.relight_key(p);
        if let Some(m) = self.relight.get(&key) {
            return (m.input.key, Some(m.input.clone()));
        }
        if !self.relight.hold {
            self.relight_start(p, &key);
        }
        (unknown_salt(p.id), None)
    }

    /// [`Self::relight_for`]'s salt alone, starting nothing (the keys of the loupe's kept views).
    pub(crate) fn relight_salt(&self, p: &Photo, s: &DevelopSettings) -> u64 {
        if lightcraft_pipeline::relight::light_of(s).is_none() {
            return 0;
        }
        let key = self.relight_key(p);
        let st = self.relight.lock();
        st.made.iter().find(|(k, _)| *k == key).map_or_else(|| unknown_salt(p.id), |(_, m)| m.input.key)
    }

    /// `f` with no field started for the renders it sets up (an export's: its worker makes the field).
    pub(crate) fn relight_held<T>(&mut self, f: impl FnOnce(&mut Session) -> T) -> T {
        let was = std::mem::replace(&mut self.relight.hold, true);
        let r = f(self);
        self.relight.hold = was;
        r
    }

    /// Queue making photo `p`'s field (`key`), unless it is being made, failed or was cancelled.
    fn relight_start(&mut self, p: &Photo, key: &str) {
        {
            let st = self.relight.lock();
            if st.busy.iter().any(|k| k == key) || st.failed.iter().any(|(k, _)| k == key) || st.cancelled.iter().any(|k| k == key) {
                return;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = p;
            self.relight.put(key, Err(NO_WEB.into()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        match self.relight_maker(p) {
            Ok(maker) => self.relight.enqueue(Queued { key: key.to_string(), maker, label: p.file_name.clone() }, &self.activity),
            Err(e) => self.relight.put(key, Err(e)),
        }
    }

    /// Find photo `id`'s depth again after it was cancelled or failed (the section's Find Depth).
    pub fn relight_retry(&mut self, id: PhotoId) {
        let Some(p) = self.catalog.photo(id).cloned() else { return };
        let key = self.relight_key(&p);
        {
            let mut st = self.relight.lock();
            st.cancelled.retain(|k| *k != key);
            st.failed.retain(|(k, _)| *k != key);
        }
        if self.relight.get(&key).is_none() {
            self.relight_start(&p, &key);
        }
    }

    /// Make photo `id`'s field now, on this thread, when Relight is on and it is not there (synchronous renders: the
    /// CLI, MCP, tests). `Err` says why it can't be made.
    pub(crate) fn relight_now(&mut self, id: PhotoId) -> Result<(), String> {
        match self.relight_export(id) {
            Some(r) => r.fill(&mut []),
            None => Ok(()),
        }
    }

    /// For an export set up now and run later: photo `id`'s field, waited for or made in the export's worker
    /// ([`ExportRelight::fill`]). `None` when the photo's light is off.
    pub(crate) fn relight_export(&mut self, id: PhotoId) -> Option<ExportRelight> {
        let p = self.catalog.photo(id)?.clone();
        lightcraft_pipeline::relight::light_of(&p.develop)?;
        let key = self.relight_key(&p);
        let made = self.relight.get(&key);
        let maker = if made.is_some() { None } else { self.relight_maker(&p).ok() };
        Some(ExportRelight { store: self.relight.clone(), key, made, maker, label: p.file_name.clone() })
    }

    /// Start making photo `id`'s field when it is not made, being made, failed or cancelled (the Relight section is
    /// open: its depth box shows the depth before the light is turned up).
    pub fn relight_prepare(&mut self, id: PhotoId) {
        let Some(p) = self.catalog.photo(id).cloned() else { return };
        let key = self.relight_key(&p);
        if self.relight.get(&key).is_none() {
            self.relight_start(&p, &key);
        }
    }

    /// Whether a field is being made (headless runs settle after it).
    pub fn relight_busy(&self) -> bool {
        !self.relight.lock().busy.is_empty()
    }

    /// Photo `id`'s field, when it is made (the section's depth box).
    pub fn relight_made(&self, id: PhotoId) -> Option<Arc<Made>> {
        let p = self.catalog.photo(id)?;
        self.relight.get(&self.relight_key(p))
    }

    /// Where photo `id`'s field stands: `{ready, finding, cancelled, source?, error?, model: {installed, bytes,
    /// downloading}}`.
    pub fn relight_status(&self, id: PhotoId) -> Value {
        let model = self.relight_model_status();
        let Some(p) = self.catalog.photo(id) else { return json!({"ready": false, "finding": false, "model": model}) };
        let key = self.relight_key(p);
        let st = self.relight.lock();
        let made = st.made.iter().find(|(k, _)| *k == key).map(|(_, m)| m.source.id());
        let error = st.failed.iter().find(|(k, _)| *k == key).map(|(_, e)| e.clone());
        let cancelled = st.cancelled.contains(&key);
        json!({"ready": made.is_some(), "finding": st.busy.contains(&key), "cancelled": cancelled, "source": made, "error": error, "model": model})
    }

    /// The depth model's state: `{installed, bytes, downloading, done, total, error?}`.
    pub fn relight_model_status(&self) -> Value {
        #[cfg(feature = "sam")]
        {
            let d = self.relight.shared.download.status();
            if d.finished && self.relight.model_installed() {
                self.relight.retry_failed();
            }
            json!({"installed": self.relight.model_installed(), "bytes": lightcraft_depth::fetch::size(), "downloading": d.running,
                   "done": d.done, "total": d.total, "error": d.error, "available": true})
        }
        #[cfg(not(feature = "sam"))]
        json!({"installed": false, "bytes": 0, "downloading": false, "available": false})
    }

    /// Start downloading the depth model into the models folder (activity row, resume, SHA-256). `Ok(false)`: it is
    /// installed or downloading already.
    pub fn relight_download(&self) -> Result<bool, String> {
        #[cfg(feature = "sam")]
        {
            let dir = self.relight.model_dir().ok_or("no folder is set for the depth model")?;
            if self.relight.model_installed() {
                return Ok(false);
            }
            let env = std::env::var(lightcraft_depth::fetch::MIRRORS_ENV).ok();
            let mirrors = lightcraft_depth::fetch::mirrors(env.as_deref());
            self.relight.shared.download.start(
                lightcraft_depth::fetch::FILES,
                mirrors,
                dir,
                lightcraft_depth::fetch::Options::default(),
                &self.activity,
                lightcraft_depth::fetch::LABEL,
            )
        }
        #[cfg(not(feature = "sam"))]
        Err("this build has no depth model".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A depth for `img` with no model: farther to the right.
    pub(super) fn fake_depth(img: &Rgba8) -> lightcraft_raster::Plane {
        let (w, h) = (img.width.max(1), img.height.max(1));
        lightcraft_raster::Plane { width: w, height: h, data: (0..w * h).map(|i| 1.0 + (i % w) as f32 / w as f32).collect() }
    }

    fn field_made(key: &str) -> Made {
        made(key, Field::from_parts(4, 3, vec![0.0; 12], vec![0.0; 12]).unwrap(), Source::Model)
    }

    /// A demo session whose first `n` photos have the light on, fields made without a model.
    fn relit(n: usize) -> (Session, Vec<PhotoId>) {
        let mut s = Session::with_demo();
        s.relight.fake_model = true;
        let ids: Vec<PhotoId> = s.catalog.photos().take(n).map(|p| p.id).collect();
        for &id in &ids {
            let mut d = s.catalog.photo(id).unwrap().develop.as_ref().clone();
            d.relight.amount = 60.0;
            let op = lightcraft_catalog::Op::SetDevelop { id, settings: Arc::new(d), label: "Relight".into(), edited: None };
            s.catalog.apply(op).unwrap();
        }
        (s, ids)
    }

    #[test]
    fn a_failed_field_is_kept_as_its_error_and_waiting_for_it_returns_it() {
        let store = RelightStore::default();
        store.lock().busy.push("a".into());
        store.put("a", Err("no depth".into()));
        assert_eq!(store.wait("a").err().as_deref(), Some("no depth"));
        assert!(store.get("a").is_none());
        store.retry_failed();
        assert!(store.lock().failed.is_empty());
    }

    #[test]
    fn fields_are_kept_most_recently_used_first_within_the_budget() {
        let store = RelightStore::default();
        let one = field_made("x").bytes();
        store.lock().budget = Some(3 * one);
        for i in 0..3 {
            store.put(&format!("k{i}"), Ok(field_made(&format!("k{i}"))));
        }
        // using the oldest makes it the newest: the next field pushes out k1, not k0
        assert!(store.get("k0").is_some());
        store.put("k3", Ok(field_made("k3")));
        let keys: Vec<String> = store.lock().made.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, ["k3", "k0", "k2"]);
        // the newest is kept however large
        store.lock().budget = Some(1);
        store.put("k4", Ok(field_made("k4")));
        assert_eq!(store.lock().made.len(), 1);
    }

    #[test]
    fn a_cancelled_field_is_not_a_failure_and_finding_it_again_starts_it() {
        let (mut s, ids) = relit(1);
        let p = s.catalog.photo(ids[0]).unwrap().clone();
        let key = s.relight_key(&p);
        s.relight.lock().busy.push(key.clone());
        s.relight.cancelled(&key);
        let st = s.relight_status(p.id);
        assert_eq!((st["cancelled"].as_bool(), st["error"].is_null()), (Some(true), true), "{st}");
        // renders don't start it again
        let _ = s.render_job(p.id, 64, 64, false, true);
        assert!(!s.relight_busy());
        // Find Depth does, and it is made
        s.relight_retry(p.id);
        assert_eq!(s.relight.wait(&key).map(|m| m.is_some()), Ok(true));
        assert_eq!(s.relight_status(p.id)["cancelled"].as_bool(), Some(false));
    }

    #[test]
    fn an_export_batch_keeps_the_field_it_was_set_up_with() {
        let (mut s, ids) = relit(5);
        // the first photo's field is kept, then the batch is set up, and only one field fits
        s.relight_now(ids[0]).unwrap();
        s.relight.lock().budget = Some(1);
        let mut batch = std::collections::VecDeque::new();
        for &id in &ids {
            // (as `export::prepare` sets up each photo: its field, then its render job, which starts none)
            batch.push_back(s.relight_export(id).unwrap());
            assert!(s.relight_held(|s| s.render_job(id, 64, 64, false, true)).is_some());
        }
        assert!(!s.relight_busy(), "setting up the export starts no finding");
        // the batch runs the first photo last: the others' fields have pushed it out by then
        batch.rotate_left(1);
        for e in batch {
            let label = e.label.clone();
            assert_eq!(e.fill(&mut []), Ok(()), "{label}");
        }
    }

    #[test]
    fn the_loupes_view_key_changes_when_the_field_lands() {
        let (mut s, ids) = relit(1);
        let p = s.catalog.photo(ids[0]).unwrap().clone();
        let before = s.view_key(&p, true);
        let quick = s.quick_view_key(p.id, true, false);
        s.relight_now(p.id).unwrap();
        assert_ne!(s.view_key(&p, true), before);
        assert_ne!(s.quick_view_key(p.id, true, false), quick);
    }

    #[test]
    fn the_light_stays_on_its_photo_unless_its_group_is_ticked() {
        let (mut s, _) = relit(0);
        let vis = s.visible_cloned();
        s.execute("library.select", &json!({"ids": [vis[0].0, vis[1].0]})).unwrap();
        s.execute("develop.autoSync", &json!({"on": true})).unwrap();
        s.execute("develop.set", &json!({"control": "relight.amount", "value": 50})).unwrap();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
        let other = s.develop_of(vis[1]).unwrap();
        assert_eq!(other.light.exposure, 0.5, "Keep in Sync is on");
        assert!(other.relight.is_default(), "Keep in Sync leaves the light");
        // Copy Settings offers it unticked; Portrait presets never carry it
        let copy = lightcraft_develop::presets::SettingsGroup::default_copy();
        assert!(!copy.contains(&lightcraft_develop::presets::SettingsGroup::Relight));
        assert!(
            !crate::portrait_presets::other_groups(&lightcraft_develop::presets::SettingsGroup::ALL)
                .contains(&lightcraft_develop::presets::SettingsGroup::Relight)
        );
        let lit = serde_json::to_value(s.develop_of(vis[0]).unwrap()).unwrap();
        let preset = crate::portrait_presets::make("p", "g", &Default::default(), lit);
        assert!(preset.settings.get(lightcraft_develop::relight::KEY).is_none());
    }

    #[test]
    fn a_missing_or_broken_depth_file_is_none() {
        assert!(file_depth("/no/such/photo.heic", lightcraft_geom::Orientation::Normal).is_none());
        let junk = std::env::temp_dir().join(format!("lightcraft-relight-junk-{}.heic", std::process::id()));
        if std::fs::write(&junk, b"not a heif file").is_ok() {
            assert!(file_depth(&junk.to_string_lossy(), lightcraft_geom::Orientation::Normal).is_none());
            let _ = std::fs::remove_file(&junk);
        }
    }
}
