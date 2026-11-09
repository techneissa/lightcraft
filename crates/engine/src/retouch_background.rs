//! Fork (`fork/retouch-background.md`): the picture Background Swap puts behind the person.
//!
//! The Portrait tab's Background Swap can use a picture file or a photo of the library (as it is developed). It is
//! loaded once, off the UI thread, at most [`EDGE`] pixels on its long side, into the render's working space
//! (scene-linear Rec.2020), with a row in the activity stack, and kept for the newest [`KEEP`] pictures. A render
//! of a photo whose Swap names it gets it with the photo's faces (`Input::with_backdrop`), so the render cache keys
//! change when it arrives and the photo is drawn again. An export loads it first, on its own thread.

use std::hash::Hasher;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::Input;
use lightcraft_develop::retouch::background::{Backdrop, Depth};
use lightcraft_raster::{Rgb32f, Rgba8};
use serde_json::{Value, json};

use crate::Session;

/// The long side (pixels) a Swap picture is loaded at: enough for an export's background, which is soft or covered.
pub const EDGE: usize = 3072;
/// Pictures kept.
const KEEP: usize = 4;
/// Faces with a picture kept (one per photo being looked at).
const KEEP_WITH: usize = 8;

#[derive(Default)]
struct State {
    /// (key, picture), newest first.
    loaded: Vec<(String, Backdrop)>,
    busy: Vec<String>,
    failed: Vec<(String, String)>,
    /// The faces of a photo with a picture and a depth map: (the faces' fingerprint, the picture's and the depth
    /// map's keys, all of them), newest first.
    with: Vec<(u64, u64, u64, Arc<Input>)>,
    /// Photos' depth maps by file path (`None`: the file has none), newest first.
    depths: Vec<(String, Option<Depth>)>,
    depth_busy: Vec<String>,
}

/// The loaded Swap pictures (shared by the session and its workers).
#[derive(Clone, Default)]
pub struct Backdrops(Arc<Mutex<State>>);

/// How a picture is loaded: a file's bytes are decoded, a library photo is rendered as it is developed.
enum Job {
    File(String),
    Photo(Box<crate::RenderJob>),
}

/// The Swap picture key of `s` (`file:<path>`, `photo:<id>`), when Swap is on and uses a picture.
pub fn picture_key(s: &DevelopSettings) -> Option<String> {
    let b = &s.retouch.global.background;
    (b.swap.is_finite() && b.swap > 0.0).then(|| b.with.picture()).flatten()
}

impl Backdrops {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The picture `key`, when loaded.
    pub fn get(&self, key: &str) -> Option<Backdrop> {
        self.lock().loaded.iter().find(|(k, _)| k == key).map(|(_, b)| b.clone())
    }

    /// Why picture `key` could not be loaded, when it could not.
    pub fn failed(&self, key: &str) -> Option<String> {
        self.lock().failed.iter().find(|(k, _)| k == key).map(|(_, e)| e.clone())
    }

    pub fn busy(&self, key: &str) -> bool {
        self.lock().busy.iter().any(|k| k == key)
    }

    fn put(&self, key: &str, result: Result<Rgb32f, String>) {
        let mut st = self.lock();
        st.busy.retain(|k| k != key);
        st.failed.retain(|(k, _)| k != key);
        match result {
            Ok(img) => {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                h.write(key.as_bytes());
                h.write(&(img.width as u64).to_le_bytes());
                h.write(&(img.height as u64).to_le_bytes());
                for p in img.data.iter().step_by(97) {
                    for v in p {
                        h.write(&v.to_bits().to_le_bytes());
                    }
                }
                st.loaded.retain(|(k, _)| k != key);
                st.loaded.insert(0, (key.to_string(), Backdrop { image: Arc::new(img), key: h.finish() }));
                st.loaded.truncate(KEEP);
            }
            Err(e) => st.failed.push((key.to_string(), e)),
        }
    }

    /// `input` with picture `b` and depth map `d`, kept so every render of the photo shares one copy of its faces.
    pub fn with(&self, input: &Arc<Input>, b: Option<Backdrop>, d: Option<Depth>) -> Arc<Input> {
        if b.is_none() && d.is_none() {
            return input.clone();
        }
        let fp = input.fingerprint();
        let (bk, dk) = (b.as_ref().map_or(0, |b| b.key), d.as_ref().map_or(0, |d| d.key));
        let mut st = self.lock();
        if let Some((_, _, _, i)) = st.with.iter().find(|(f, x, y, _)| *f == fp && *x == bk && *y == dk) {
            return i.clone();
        }
        let made = Arc::new(input.with_backdrop(b).with_depth(d));
        st.with.insert(0, (fp, bk, dk, made.clone()));
        st.with.truncate(KEEP_WITH);
        made
    }

    /// The depth map of the photo file at `path`: `Some(None)` when it has none, `None` while not read.
    fn depth(&self, path: &str) -> Option<Option<Depth>> {
        self.lock().depths.iter().find(|(p, _)| p == path).map(|(_, d)| d.clone())
    }

    fn put_depth(&self, path: &str, d: Option<Depth>) {
        let mut st = self.lock();
        st.depth_busy.retain(|p| p != path);
        st.depths.retain(|(p, _)| p != path);
        st.depths.insert(0, (path.to_string(), d));
        st.depths.truncate(KEEP_WITH);
    }

    /// Forget picture `key` (a file changed on disk, a photo re-edited: Swap with it again loads it anew).
    pub fn forget(&self, key: &str) {
        let mut st = self.lock();
        st.loaded.retain(|(k, _)| k != key);
        st.failed.retain(|(k, _)| k != key);
    }
}

/// Load `job` into the working space, at most [`EDGE`] on its long side.
fn load(job: Job) -> Result<Rgb32f, String> {
    match job {
        Job::File(path) => {
            let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
            let edge = EDGE as u32;
            let d = lightcraft_codecs::decode(&bytes, lightcraft_codecs::DecodeOptions::fit(edge, edge)).map_err(|e| format!("{path}: {e}"))?;
            Ok(d.to_working().into_oriented(lightcraft_geom::Orientation::from_exif(d.orientation)))
        }
        Job::Photo(job) => {
            let r = job.run();
            let img = r.rendered.map_err(|e| format!("the photo could not be developed: {e}"))?.image;
            Ok(srgb8_to_working(&img))
        }
    }
}

/// The depth map of the photo file at `path` (`None`: it has none, or it can't be read).
fn read_depth(path: &str) -> Option<Depth> {
    let bytes = std::fs::read(path).ok()?;
    let (w, h, data) = lightcraft_codecs::decode_depth(&bytes)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    hash.write(path.as_bytes());
    hash.write(&(bytes.len() as u64).to_le_bytes());
    let plane = lightcraft_raster::Plane { width: w, height: h, data };
    (plane.data.len() == w.saturating_mul(h) && w > 0 && h > 0).then(|| Depth { plane: Arc::new(plane), key: hash.finish() })
}

/// The path of photo `p`'s file when Blur could read a depth map from it (a HEIF photo) and Blur is on.
fn depth_path(p: &lightcraft_catalog::Photo, s: &DevelopSettings) -> Option<String> {
    let b = &s.retouch.global.background;
    if !(b.blur.is_finite() && b.blur > 0.0) {
        return None;
    }
    let lightcraft_catalog::Source::File { path } = &p.source else { return None };
    let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
    matches!(ext.as_str(), "heic" | "heif" | "hif").then(|| path.clone())
}

/// 8-bit sRGB → linear Rec.2020.
fn srgb8_to_working(img: &Rgba8) -> Rgb32f {
    let m = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020);
    let lin = img.to_linear();
    lin.map(|p| m.apply_f32(p))
}

/// Read the depth map of the file at `path` on a worker, unless it is being read.
fn depth_load(store: &Backdrops, path: String, activity: &crate::activity::Activity) {
    {
        let mut st = store.lock();
        if st.depth_busy.contains(&path) {
            return;
        }
        st.depth_busy.push(path.clone());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let guard = activity.start("retouch.depth", "Reading Depth Map", crate::activity::Cancel::No);
        let worker = store.clone();
        let p = path.clone();
        let spawned = std::thread::Builder::new().name("retouch-depth".into()).spawn(move || {
            let d = crate::guard::catch("the depth map", || read_depth(&p)).ok().flatten();
            worker.put_depth(&p, d);
            drop(guard);
        });
        if spawned.is_err() {
            store.put_depth(&path, None);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = activity;
        store.put_depth(&path, read_depth(&path));
    }
}

impl Session {
    /// The job that loads picture `key`; `Err` for a key that names no file or photo.
    fn backdrop_job(&mut self, key: &str) -> Result<Job, String> {
        if let Some(path) = key.strip_prefix("file:") {
            return Ok(Job::File(path.to_string()));
        }
        let id = key.strip_prefix("photo:").and_then(|v| v.parse::<u64>().ok()).ok_or_else(|| format!("not a picture: {key}"))?;
        let id = lightcraft_catalog::PhotoId(id);
        let job = self.render_job(id, EDGE, EDGE, false, true).ok_or_else(|| format!("photo {} is not in the library", id.0))?;
        Ok(Job::Photo(Box::new(job)))
    }

    /// Start loading picture `key` on a worker, unless it is loaded, loading or failed.
    pub(crate) fn backdrop_load(&mut self, key: &str) {
        let store = self.retouch_faces.backdrops.clone();
        if store.get(key).is_some() || store.busy(key) || store.failed(key).is_some() {
            return;
        }
        let job = match self.backdrop_job(key) {
            Ok(job) => job,
            Err(e) => {
                store.put(key, Err(e));
                return;
            }
        };
        store.lock().busy.push(key.to_string());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let guard = self.activity.start("retouch.backdrop", "Loading Background Picture", crate::activity::Cancel::No);
            let k = key.to_string();
            let worker = store.clone();
            let spawned = std::thread::Builder::new().name("retouch-backdrop".into()).spawn(move || {
                let result = crate::guard::catch("the background picture", || load(job)).and_then(|r| r);
                if let Err(e) = &result {
                    log::warn!("portrait: background picture not loaded: {e}");
                }
                worker.put(&k, result);
                drop(guard);
            });
            if let Err(e) = spawned {
                store.put(key, Err(format!("could not start loading the background picture: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        store.put(key, load(job));
    }

    /// Load the Swap picture of photo `p` with settings `s` and its depth map now, on this thread, when they are not
    /// loaded (exports, synchronous renders).
    pub(crate) fn backdrop_now(&mut self, p: &lightcraft_catalog::Photo, s: &DevelopSettings) {
        let store = self.retouch_faces.backdrops.clone();
        if let Some(path) = depth_path(p, s)
            && store.depth(&path).is_none()
        {
            store.put_depth(&path, read_depth(&path));
        }
        let Some(key) = picture_key(s) else { return };
        if store.get(&key).is_some() {
            return;
        }
        let result = self.backdrop_job(&key).and_then(|job| crate::guard::catch("the background picture", || load(job)).and_then(|r| r));
        if let Err(e) = &result {
            log::warn!("portrait: the export renders without its background picture: {e}");
        }
        store.put(&key, result);
    }

    /// `input` with what Background reads besides the faces, when it is loaded: Swap's picture and the photo's depth
    /// map. Starts loading what is not (`start`).
    pub(crate) fn with_backdrop(&mut self, input: Arc<Input>, p: &lightcraft_catalog::Photo, s: &DevelopSettings, start: bool) -> Arc<Input> {
        let store = self.retouch_faces.backdrops.clone();
        let picture = picture_key(s);
        let b = picture.as_deref().and_then(|k| store.get(k));
        if start && let Some(k) = picture.as_deref().filter(|_| b.is_none()) {
            self.backdrop_load(k);
        }
        let path = depth_path(p, s);
        let d = path.as_deref().and_then(|path| store.depth(path)).flatten();
        if start && let Some(path) = path.filter(|path| store.depth(path).is_none()) {
            depth_load(&store, path, &self.activity);
        }
        store.with(&input, b, d)
    }

    /// [`Self::with_backdrop`] without starting anything (cache keys of renders made elsewhere).
    pub(crate) fn with_backdrop_now(&self, input: Arc<Input>, p: &lightcraft_catalog::Photo, s: &DevelopSettings) -> Arc<Input> {
        let store = &self.retouch_faces.backdrops;
        let b = picture_key(s).and_then(|k| store.get(&k));
        let d = depth_path(p, s).and_then(|path| store.depth(&path)).flatten();
        store.with(&input, b, d)
    }

    /// The state of the Swap picture of photo `s`'s settings: `{picture?, loaded, loading, error?}`.
    pub fn backdrop_status(&self, s: &DevelopSettings) -> Value {
        let b = &s.retouch.global.background;
        let Some(key) = b.with.picture() else { return json!({"loaded": false, "loading": false}) };
        let store = &self.retouch_faces.backdrops;
        let size = store.get(&key).map(|b| json!([b.image.width, b.image.height]));
        json!({"picture": key, "loaded": size.is_some(), "size": size, "loading": store.busy(&key), "error": store.failed(&key)})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_srgb_is_white_in_the_working_space() {
        let img = Rgba8::filled(2, 1, [255, 255, 255, 255]);
        let w = srgb8_to_working(&img);
        assert!(w.data.iter().flatten().all(|v| (v - 1.0).abs() < 1e-3), "{:?}", w.data);
    }

    #[test]
    fn a_missing_file_fails_with_its_path_and_never_panics() {
        let store = Backdrops::default();
        let r = load(Job::File("/no/such/picture.jpg".into()));
        assert!(r.as_ref().is_err_and(|e| e.contains("/no/such/picture.jpg")), "{:?}", r.err());
        store.put("file:/no/such/picture.jpg", r);
        assert!(store.failed("file:/no/such/picture.jpg").is_some() && store.get("file:/no/such/picture.jpg").is_none());
        let junk = std::env::temp_dir().join(format!("lightcraft-backdrop-junk-{}.jpg", std::process::id()));
        if std::fs::write(&junk, b"\xff\xd8\xff not a jpeg").is_ok() {
            assert!(load(Job::File(junk.to_string_lossy().into_owned())).is_err());
            let _ = std::fs::remove_file(&junk);
        }
    }

    #[test]
    fn a_loaded_picture_is_kept_and_shared_by_the_faces_of_a_photo() {
        let store = Backdrops::default();
        store.put("file:a.png", Ok(Rgb32f::filled(4, 3, [0.5; 3])));
        let b = store.get("file:a.png").unwrap();
        let input = Arc::new(Input::new(Vec::new()));
        let (x, y) = (store.with(&input, Some(b.clone()), None), store.with(&input, Some(b.clone()), None));
        assert!(Arc::ptr_eq(&x, &y));
        assert_ne!(x.fingerprint(), input.fingerprint(), "the picture is in the render keys");
        for i in 0..6 {
            store.put(&format!("file:{i}.png"), Ok(Rgb32f::filled(1, 1, [0.1; 3])));
        }
        assert!(store.get("file:a.png").is_none(), "only the newest are kept");
    }
}
