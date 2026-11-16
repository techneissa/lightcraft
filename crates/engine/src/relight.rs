//! Fork (issue #288, `fork/relight.md`): each photo's Relight field, made once and kept.
//!
//! A photo's depth is its file's own depth map when it has one (an iPhone Portrait HEIC, read as Portrait ▸
//! Background reads it), else Depth Anything 3 Mono Large's (`lightcraft-depth`, in the shared models folder, on the
//! GPU through Metal). The field ([`lightcraft_relight::Field`]) is that depth brought to a guide picture of the
//! photo ([`FIELD_EDGE`] on its long side, as it was shot: no edits but its rotation, before the crop and the lens
//! corrections) with its normals and edges.
//!
//! It is made off the UI thread, with a row in the activity stack, the first time a render of a photo with Relight
//! on asks for it, and kept by the photo's content and rotation (the newest [`KEEP`]): no edit of the photo makes it
//! again. Renders get it in their `SourceInfo` once it is there, so their keys change and the photo is drawn again;
//! until then they render without the light. An export (and a synchronous render: the CLI, MCP) waits for it on its
//! own thread, or makes it there.

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
/// Fields kept (bytes: about 40 MB each).
pub const KEEP: usize = 3;
/// The long side the depth model reads the photo at.
pub const MODEL_LONG: usize = 504;
/// The folder of the depth model under the models folder.
pub const MODEL_DIR: &str = "da3-mono-large";
/// The depth box's picture (pixels on its long side).
pub const THUMB: usize = 240;
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

#[derive(Default)]
struct State {
    /// Newest first.
    made: Vec<(String, Arc<Made>)>,
    busy: Vec<String>,
    failed: Vec<(String, String)>,
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

    fn get(&self, key: &str) -> Option<Arc<Made>> {
        self.lock().made.iter().find(|(k, _)| k == key).map(|(_, m)| m.clone())
    }

    fn put(&self, key: &str, result: Result<Made, String>) {
        let mut st = self.lock();
        st.busy.retain(|k| k != key);
        st.failed.retain(|(k, _)| k != key);
        match result {
            Ok(m) => {
                st.made.retain(|(k, _)| k != key);
                st.made.insert(0, (key.to_string(), Arc::new(m)));
                st.made.truncate(KEEP);
            }
            Err(e) => st.failed.push((key.to_string(), e)),
        }
        drop(st);
        self.shared.done.notify_all();
    }

    /// Wait (on a worker) while field `key` is being made; it, or why it could not be.
    fn wait(&self, key: &str) -> Result<Arc<Made>, String> {
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
            return Ok(m.clone());
        }
        Err(st.failed.iter().find(|(k, _)| k == key).map_or_else(|| "the photo's depth was not found".into(), |(_, e)| e.clone()))
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

/// What makes a photo's field: the guide render, the file's depth map (when it may have one), the photo's rotation
/// and the model's folder. Set up on the session's thread, run anywhere.
pub(crate) struct Maker {
    key: String,
    guide: Box<crate::RenderJob>,
    file: Option<String>,
    orientation: lightcraft_geom::Orientation,
    store: RelightStore,
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
    /// Make the field (this takes seconds: run it on a worker). `cancel` stops the depth model between its blocks.
    fn make(self, cancel: Option<Arc<AtomicBool>>) -> Result<Made, String> {
        let Maker { key, guide, file, orientation, store } = self;
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
        let depth = model_depth(&store, &img, cancel)?;
        let field = Field::prepare(DepthMap { width: depth.width, height: depth.height, values: &depth.data, kind: Kind::Depth }, &guide)
            .map_err(|e| e.to_string())?;
        Ok(made(&key, field, Source::Model))
    }
}

/// The depth of `img` from the model in `store`'s folder (loaded once and kept).
#[cfg(feature = "sam")]
fn model_depth(store: &RelightStore, img: &Rgba8, cancel: Option<Arc<AtomicBool>>) -> Result<lightcraft_raster::Plane, String> {
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
    Err("this build has no depth model; only photos with their own depth map can be relit".into())
}

/// What a photo without a depth map of its own needs.
pub const MISSING: &str = "the depth model is not installed";

fn made(key: &str, field: Field, source: Source) -> Made {
    let thumb = field.thumbnail(THUMB);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(key, &mut h);
    let k = std::hash::Hasher::finish(&h) | 1;
    Made { input: Input { field: Arc::new(field), key: k }, source, thumb }
}

/// An export's wait for its photo's field ([`Session::relight_export`]): made on the export's worker when nothing
/// else is making it.
pub struct ExportRelight {
    store: RelightStore,
    key: String,
    maker: Option<Maker>,
    label: String,
}

impl ExportRelight {
    /// Make (or wait for) the field and give it to `jobs`. Fails, naming the photo, when it can't be made (never a
    /// silent export without the light).
    pub fn fill(self, jobs: &mut [&mut crate::RenderJob]) -> Result<(), String> {
        let busy = self.store.lock().busy.contains(&self.key);
        let m = match self.store.get(&self.key) {
            Some(m) => m,
            None if busy => self.store.wait(&self.key).map_err(|e| format!("{}: {e}", self.label))?,
            None => {
                let maker = self.maker.ok_or_else(|| format!("{}: the photo's depth can't be found", self.label))?;
                self.store.lock().busy.push(self.key.clone());
                let r = crate::guard::catch("the photo's depth", || maker.make(None)).and_then(|r| r);
                let err = r.as_ref().err().map(|e| format!("{}: {e}", self.label));
                self.store.put(&self.key, r);
                if let Some(e) = err {
                    return Err(e);
                }
                self.store.get(&self.key).ok_or_else(|| format!("{}: the photo's depth was not kept", self.label))?
            }
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
        Ok(Maker { key: self.relight_key(p), guide: Box::new(guide), file, orientation: p.develop.orientation, store: self.relight.clone() })
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
        self.relight_start(p, &key);
        (0x5e1f_0000_0000_0001 ^ p.id.0.rotate_left(7), None)
    }

    /// Start making photo `p`'s field (`key`) on a worker, unless it is being made or failed.
    fn relight_start(&mut self, p: &Photo, key: &str) {
        {
            let st = self.relight.lock();
            if st.busy.iter().any(|k| k == key) || st.failed.iter().any(|(k, _)| k == key) {
                return;
            }
        }
        let maker = match self.relight_maker(p) {
            Ok(m) => m,
            Err(e) => {
                self.relight.put(key, Err(e));
                return;
            }
        };
        self.relight.lock().busy.push(key.to_string());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cancel = Arc::new(AtomicBool::new(false));
            let guard = self.activity.start_background("relight.depth", "Finding depth", crate::activity::Cancel::Flag(cancel.clone()));
            guard.detail(&p.file_name);
            let (store, k) = (self.relight.clone(), key.to_string());
            let spawned = std::thread::Builder::new().name("relight-depth".into()).spawn(move || {
                let r = crate::guard::catch("the photo's depth", || maker.make(Some(cancel))).and_then(|r| r);
                if let Err(e) = &r {
                    log::warn!("relight: {e}");
                }
                store.put(&k, r);
                drop(guard);
            });
            if let Err(e) = spawned {
                self.relight.put(key, Err(format!("could not start finding the photo's depth: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.relight.put(key, maker.make(None));
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
        let maker = if self.relight.get(&key).is_some() { None } else { self.relight_maker(&p).ok() };
        Some(ExportRelight { store: self.relight.clone(), key, maker, label: p.file_name.clone() })
    }

    /// Photo `id`'s field, when it is made (the section's depth box).
    pub fn relight_made(&self, id: PhotoId) -> Option<Arc<Made>> {
        let p = self.catalog.photo(id)?;
        self.relight.get(&self.relight_key(p))
    }

    /// Where photo `id`'s field stands: `{ready, finding, source?, error?, model: {installed, bytes, downloading}}`.
    pub fn relight_status(&self, id: PhotoId) -> Value {
        let model = self.relight_model_status();
        let Some(p) = self.catalog.photo(id) else { return json!({"ready": false, "finding": false, "model": model}) };
        let key = self.relight_key(p);
        let st = self.relight.lock();
        let made = st.made.iter().find(|(k, _)| *k == key).map(|(_, m)| m.source.id());
        let error = st.failed.iter().find(|(k, _)| *k == key).map(|(_, e)| e.clone());
        json!({"ready": made.is_some(), "finding": st.busy.contains(&key), "source": made, "error": error, "model": model})
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
    fn fields_are_kept_newest_first_up_to_the_cap() {
        let store = RelightStore::default();
        for i in 0..5 {
            let field = Field::from_parts(4, 3, vec![0.0; 12], vec![0.0; 12]).unwrap();
            store.put(&format!("k{i}"), Ok(made(&format!("k{i}"), field, Source::Model)));
        }
        let st = store.lock();
        assert_eq!(st.made.len(), KEEP);
        assert_eq!(st.made[0].0, "k4");
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
