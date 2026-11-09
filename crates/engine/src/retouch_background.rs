//! Fork (`fork/retouch-background.md`): the picture Background Swap puts behind the person, and the photo's depth map.
//!
//! A picture file the user picks is copied into the library's own folder ([`FOLDER`], `<library>/Backgrounds/
//! <sha256>.<ext>`, with its file name beside it in `<sha256>.name`) and the settings keep only its hash, so no
//! sidecar, DNG or catalog carries a path. A library photo is named by its id and used as it is developed.
//!
//! Loading never runs on the UI thread: as soon as a render of a photo whose Swap names a picture asks for it, it
//! is loaded on a worker (decoded, or the library photo developed), at most [`EDGE`] pixels on its long side, into
//! the render's working space (scene-linear Rec.2020), with a row in the activity stack; renders get it with the
//! photo's faces (`Input::with_backdrop`) once it is there, so their cache keys change and the photo is drawn again.
//! An export starts the same load when it is set up and waits for it in its own worker ([`ExportBackdrop::fill`]);
//! a picture that is missing or can't be loaded fails the export with its name. Pictures are kept by content (the
//! file's hash, the photo's version), the newest [`KEEP`] within [`KEEP_BYTES`]; a failed load is tried again when
//! its source changes (the file appears, the photo is edited) or in a new session. A picture over [`MAX_PIXELS`] is
//! refused from its header, before it is decoded.

use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use lightcraft_catalog::{Photo, PhotoId};
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::Input;
use lightcraft_develop::retouch::background::{Backdrop, Depth, Swap};
use lightcraft_raster::{Rgb32f, Rgba8};
use serde_json::{Value, json};

use crate::Session;

/// The long side (pixels) a Swap picture is loaded at: enough for an export's background, which is soft or covered.
pub const EDGE: usize = 3072;
/// The largest picture Swap takes (pixels, from its header): 64 MP.
pub const MAX_PIXELS: u64 = 1 << 26;
/// The library folder Swap pictures are copied into.
pub const FOLDER: &str = "Backgrounds";
/// Pictures kept…
const KEEP: usize = 4;
/// …and at most this much memory of them (bytes; the newest is always kept).
const KEEP_BYTES: usize = 384 << 20;
/// Faces with a picture kept (one per photo being looked at).
const KEEP_WITH: usize = 8;
/// How long an export waits for a picture another load is making before it gives up (a guard against a hang).
const WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// A Swap picture's identity: its key (`picture:<hash>`, `photo:<id>`) and the version of its source.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Id {
    key: String,
    version: u64,
}

#[derive(Default)]
struct State {
    /// Newest first.
    loaded: Vec<(Id, Backdrop)>,
    busy: Vec<Id>,
    failed: Vec<(Id, String)>,
    /// The faces of a photo with a picture and a depth map: (the faces' fingerprint, the picture's and the depth
    /// map's keys, all of them), newest first.
    with: Vec<(u64, u64, u64, Arc<Input>)>,
    /// Photos' depth maps by file (path and stamp: its length and modification time; `None`: it has none), newest
    /// first. Each map's own key is a hash of the file's bytes.
    depths: Vec<(String, u64, Option<Depth>)>,
    depth_busy: Vec<(String, u64)>,
    /// Files whose depth map was left out for its shape (logged once).
    depth_warned: Vec<String>,
}

/// The loaded Swap pictures and depth maps (shared by the session and its workers).
#[derive(Clone, Default)]
pub struct Backdrops(Arc<(Mutex<State>, Condvar)>);

/// How a picture is loaded: a file's bytes are decoded, a library photo is rendered as it is developed.
enum Job {
    File(PathBuf),
    Photo(Box<crate::RenderJob>),
}

/// What Swap needs loaded for `s`, when Swap is on and uses a picture.
fn swap_picture(s: &DevelopSettings) -> Option<&Swap> {
    let b = &s.retouch.global.background;
    (b.swap.is_finite() && b.swap > 0.0 && b.with.picture().is_some()).then_some(&b.with)
}

/// A fast hash of `bytes`.
fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

/// A file's length and modification time, hashed (0 when it can't be read).
fn stamp(path: &Path) -> u64 {
    let Ok(m) = std::fs::metadata(path) else { return 0 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_u64(m.len());
    if let Ok(t) = m.modified().map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default()) {
        h.write_u128(t.as_nanos());
    }
    h.finish() | 1
}

/// A hash is 64 hex digits (what [`Session::backdrop_import`] writes): nothing else names a file.
fn is_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Backdrops {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, id: &Id) -> Option<Backdrop> {
        self.lock().loaded.iter().find(|(i, _)| i == id).map(|(_, b)| b.clone())
    }

    fn failed(&self, id: &Id) -> Option<String> {
        self.lock().failed.iter().find(|(i, _)| i == id).map(|(_, e)| e.clone())
    }

    fn busy(&self, id: &Id) -> bool {
        self.lock().busy.contains(id)
    }

    fn put(&self, id: &Id, result: Result<Rgb32f, String>) {
        let mut st = self.lock();
        st.busy.retain(|i| i != id);
        // (an older version of the same picture goes too)
        st.failed.retain(|(i, _)| i.key != id.key);
        match result {
            Ok(img) => {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                h.write(id.key.as_bytes());
                h.write_u64(id.version);
                st.loaded.retain(|(i, _)| i.key != id.key);
                st.loaded.insert(0, (id.clone(), Backdrop { image: Arc::new(img), key: h.finish() }));
                st.loaded.truncate(KEEP);
                let mut total = 0usize;
                st.loaded.retain(|(_, b)| {
                    let fits = total == 0 || total + bytes_of(b) <= KEEP_BYTES;
                    total += bytes_of(b);
                    fits
                });
            }
            Err(e) => st.failed.push((id.clone(), e)),
        }
        drop(st);
        self.0.1.notify_all();
    }

    /// Wait (on a worker) until picture `id` is no longer being loaded; its picture, or why it could not be loaded.
    fn wait(&self, id: &Id) -> Result<Backdrop, String> {
        let start = std::time::Instant::now();
        let mut st = self.lock();
        while st.busy.contains(id) {
            let left = WAIT.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Err("the background picture took too long to load".into());
            }
            st = self.0.1.wait_timeout(st, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        if let Some((_, b)) = st.loaded.iter().find(|(i, _)| i == id) {
            return Ok(b.clone());
        }
        Err(st.failed.iter().find(|(i, _)| i == id).map_or_else(|| "the background picture was not loaded".into(), |(_, e)| e.clone()))
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

    /// The depth map of the file at `path` as it is now (`stamp`): `Some(None)` when it has none, `None` while not
    /// read.
    fn depth(&self, path: &str, stamp: u64) -> Option<Option<Depth>> {
        self.lock().depths.iter().find(|(p, s, _)| p == path && *s == stamp).map(|(_, _, d)| d.clone())
    }

    fn put_depth(&self, path: &str, stamp: u64, d: Option<Depth>) {
        let mut st = self.lock();
        st.depth_busy.retain(|(p, s)| !(p == path && *s == stamp));
        st.depths.retain(|(p, _, _)| p != path);
        st.depths.insert(0, (path.to_string(), stamp, d));
        st.depths.truncate(KEEP_WITH);
    }

    /// Forget picture `key` in every version (Swap chosen again: it loads anew).
    pub fn forget(&self, key: &str) {
        let mut st = self.lock();
        st.loaded.retain(|(i, _)| i.key != key);
        st.failed.retain(|(i, _)| i.key != key);
    }
}

fn bytes_of(b: &Backdrop) -> usize {
    b.image.data.len().saturating_mul(std::mem::size_of::<[f32; 3]>())
}

/// Read the picture file at `path` into the working space, at most [`EDGE`] on its long side; a picture over
/// [`MAX_PIXELS`] is refused from its header, before any pixel is decoded.
fn load_file(path: &Path) -> Result<Rgb32f, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let h = lightcraft_codecs::read_header(&bytes).map_err(|e| e.to_string())?;
    let px = u64::from(h.width).saturating_mul(u64::from(h.height));
    if px > MAX_PIXELS {
        return Err(format!("it is {} × {} pixels; Swap takes pictures of up to {} megapixels", h.width, h.height, MAX_PIXELS >> 20));
    }
    let edge = EDGE as u32;
    let opts = lightcraft_codecs::DecodeOptions { max_pixels: MAX_PIXELS, ..lightcraft_codecs::DecodeOptions::fit(edge, edge) };
    let d = lightcraft_codecs::decode(&bytes, opts).map_err(|e| e.to_string())?;
    Ok(d.to_working().into_oriented(lightcraft_geom::Orientation::from_exif(d.orientation)))
}

/// Load `job` into the working space, at most [`EDGE`] on its long side.
fn load(job: Job) -> Result<Rgb32f, String> {
    match job {
        Job::File(path) => load_file(&path),
        Job::Photo(job) => {
            let r = job.run();
            let img = r.rendered.map_err(|e| format!("the photo could not be developed: {e}"))?.image;
            Ok(srgb8_to_working(&img))
        }
    }
}

/// The depth map of the photo file at `path` (`None`: it has none, or it can't be read), keyed by the file's bytes.
fn read_depth(path: &str) -> Option<Depth> {
    let bytes = std::fs::read(path).ok()?;
    let (w, h, data) = lightcraft_codecs::decode_depth(&bytes)?;
    let plane = lightcraft_raster::Plane { width: w, height: h, data };
    (plane.data.len() == w.saturating_mul(h) && w > 0 && h > 0).then(|| Depth { plane: Arc::new(plane), key: hash_bytes(&bytes) | 1 })
}

/// Whether depth map `d` has photo `p`'s shape (either way round: the catalog's size may be before the photo's
/// rotation) within [`lightcraft_develop::retouch::background::falloff::DEPTH_SHAPE`].
fn depth_fits(d: &Depth, width: u32, height: u32) -> bool {
    let (w, h) = (d.plane.width as f64, d.plane.height as f64);
    let (pw, ph) = (f64::from(width), f64::from(height));
    if !(w > 0.0 && h > 0.0 && pw > 0.0 && ph > 0.0) {
        return true;
    }
    let tol = lightcraft_develop::retouch::background::falloff::DEPTH_SHAPE;
    [(pw / ph), (ph / pw)].iter().any(|a| ((w / h) / a - 1.0).abs() <= tol)
}

/// The path of photo `p`'s file when Blur could read a depth map from it (a HEIF photo) and Blur is on.
fn depth_path(p: &Photo, s: &DevelopSettings) -> Option<String> {
    let b = &s.retouch.global.background;
    if !(b.blur.is_finite() && b.blur > 0.0) {
        return None;
    }
    let lightcraft_catalog::Source::File { path } = &p.source else { return None };
    let ext = Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
    matches!(ext.as_str(), "heic" | "heif" | "hif").then(|| path.clone())
}

/// 8-bit sRGB → linear Rec.2020.
fn srgb8_to_working(img: &Rgba8) -> Rgb32f {
    let m = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020);
    let lin = img.to_linear();
    lin.map(|p| m.apply_f32(p))
}

/// Read the depth map of the file at `path` now, logging once when its shape is not photo `p`'s.
fn read_depth_of(store: &Backdrops, path: &str, p: &Photo) -> Option<Depth> {
    let d = crate::guard::catch("the depth map", || read_depth(path)).ok().flatten();
    if let Some(d) = &d
        && !depth_fits(d, p.width, p.height)
    {
        let mut st = store.lock();
        if !st.depth_warned.iter().any(|w| w == path) {
            st.depth_warned.push(path.to_string());
            log::warn!(
                "portrait: the depth map of {} ({} × {}) does not have the photo's shape; Blur uses the distance from the person",
                p.file_name,
                d.plane.width,
                d.plane.height
            );
        }
    }
    d
}

/// Read the depth map of the file at `path` on a worker, unless it is being read.
fn depth_load(store: &Backdrops, path: String, stamp: u64, p: &Photo, activity: &crate::activity::Activity) {
    {
        let mut st = store.lock();
        if st.depth_busy.iter().any(|(q, s)| *q == path && *s == stamp) {
            return;
        }
        st.depth_busy.push((path.clone(), stamp));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let guard = activity.start("retouch.depth", "Reading depth map", crate::activity::Cancel::No);
        let worker = store.clone();
        let (q, photo) = (path.clone(), p.clone());
        let spawned = std::thread::Builder::new().name("retouch-depth".into()).spawn(move || {
            let d = read_depth_of(&worker, &q, &photo);
            worker.put_depth(&q, stamp, d);
            drop(guard);
        });
        if spawned.is_err() {
            store.put_depth(&path, stamp, None);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = activity;
        store.put_depth(&path, stamp, read_depth_of(store, &path, p));
    }
}

/// What an export waits for before it renders: Background Swap's picture and the photo's depth map
/// ([`Session::retouch_export_backdrop`]), handed to its jobs' faces on the export's worker.
pub struct ExportBackdrop {
    store: Backdrops,
    /// The photo's name, for errors.
    label: String,
    picture: Option<Picture>,
    depth: Option<(String, u64, Arc<Photo>)>,
}

enum Picture {
    /// Being loaded (or loaded): wait for it.
    Wait(Id, String),
    /// It can't be: the export fails with this.
    Missing(String),
}

impl ExportBackdrop {
    /// Wait for the picture and the depth map and give them to the faces of `jobs`. Fails, naming the picture,
    /// when Swap's picture is missing or could not be loaded (never a silent export without it).
    pub fn fill(self, jobs: &mut [&mut crate::RenderJob]) -> Result<(), String> {
        let b = match &self.picture {
            None => None,
            Some(Picture::Missing(why)) => return Err(format!("{}: {why}", self.label)),
            Some(Picture::Wait(id, name)) => {
                Some(self.store.wait(id).map_err(|e| format!("{}: the background picture {name} could not be loaded: {e}", self.label))?)
            }
        };
        let d = match &self.depth {
            Some((path, stamp, p)) => match self.store.depth(path, *stamp) {
                Some(d) => d,
                None => {
                    let d = read_depth_of(&self.store, path, p);
                    self.store.put_depth(path, *stamp, d.clone());
                    d
                }
            },
            None => None,
        };
        for j in jobs.iter_mut() {
            if let Some(input) = &j.info.retouch {
                j.info.retouch = Some(self.store.with(input, b.clone(), d.clone()));
            }
        }
        Ok(())
    }
}

/// The name of picture file `hash` in the library: the name it was chosen under, else its file there.
fn picture_name(dir: Option<&Path>, hash: &str) -> String {
    let short = hash.get(..8).unwrap_or(hash);
    dir.and_then(|d| std::fs::read_to_string(d.join(format!("{hash}.name"))).ok())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{short}…"))
}

impl Session {
    /// The library's folder of Swap pictures ([`FOLDER`]); a session without a library on disk keeps them in the
    /// system's temporary folder.
    pub fn backdrop_dir(&self) -> PathBuf {
        match self.library.as_ref().filter(|l| l.on_disk) {
            Some(l) => l.dir.join(FOLDER),
            None => std::env::temp_dir().join(format!("LightCraft {FOLDER}")),
        }
    }

    /// The file of picture `hash` in [`Self::backdrop_dir`], when it is there.
    fn backdrop_file(&self, hash: &str) -> Option<PathBuf> {
        if !is_hash(hash) {
            return None;
        }
        let dir = self.backdrop_dir();
        std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(hash)) && p.extension().is_some_and(|e| e != "name"))
    }

    /// Copy the picture file at `path` into the library's folder of Swap pictures as `<sha256>.<ext>` (once: the
    /// same picture chosen again is the same file) and give its hash. Its header is read first: a file that is not
    /// a picture, or one over [`MAX_PIXELS`], is refused.
    pub fn backdrop_import(&mut self, path: &Path) -> Result<String, String> {
        use sha2::Digest;
        let name = path.file_name().map_or_else(|| "picture".into(), |n| n.to_string_lossy().into_owned());
        let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        let h = lightcraft_codecs::read_header(&bytes).map_err(|e| format!("{name} is not a picture LightCraft can read: {e}"))?;
        if u64::from(h.width).saturating_mul(u64::from(h.height)) > MAX_PIXELS {
            return Err(format!("{name} is {} × {} pixels; Swap takes pictures of up to {} megapixels", h.width, h.height, MAX_PIXELS >> 20));
        }
        let hash = format!("{:x}", sha2::Sha256::digest(&bytes));
        if self.backdrop_file(&hash).is_none() {
            let dir = self.backdrop_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("the library's {FOLDER} folder can't be made: {e}"))?;
            let ext: String = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .filter(|e| !e.is_empty() && e.len() <= 5 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
                .unwrap_or_else(|| "img".into());
            let file = dir.join(format!("{hash}.{ext}"));
            let part = dir.join(format!("{hash}.{ext}.part"));
            std::fs::write(&part, &bytes).and_then(|()| std::fs::rename(&part, &file)).map_err(|e| {
                let _ = std::fs::remove_file(&part);
                format!("{name} could not be copied into the library: {e}")
            })?;
            // (the name it was chosen under, for the section and errors; never its folder)
            let _ = std::fs::write(dir.join(format!("{hash}.name")), name.as_bytes());
        }
        Ok(hash)
    }

    /// The version of library photo `id` (its settings and its file), when it is in the library.
    fn photo_version(&self, id: PhotoId) -> Option<u64> {
        let p = self.catalog.photo(id)?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        h.write_usize(Arc::as_ptr(&p.develop) as usize);
        h.write(p.edited.as_deref().unwrap_or("").as_bytes());
        h.write_u64(p.file_size);
        Some(h.finish() | 1)
    }

    /// Swap `with`'s picture as it is now, its name, and how to load it; `Err` (why, its name) when it is missing.
    fn backdrop_source(&self, with: &Swap) -> Option<Result<(Id, String, Box<dyn FnOnce(&mut Session) -> Result<Job, String>>), (String, String)>> {
        let key = with.picture()?;
        Some(match with {
            Swap::Colour { .. } => return None,
            Swap::Picture { hash } => {
                let name = picture_name(Some(&self.backdrop_dir()), hash);
                match self.backdrop_file(hash) {
                    Some(file) => {
                        let id = Id { key, version: stamp(&file) };
                        Ok((id, name, Box::new(move |_: &mut Session| Ok(Job::File(file)))))
                    }
                    None => Err((format!("the background picture {name} is missing from the library's {FOLDER} folder"), name)),
                }
            }
            Swap::Photo { id } => {
                let pid = PhotoId(*id);
                let name = self.catalog.photo(pid).map_or_else(|| format!("photo {id}"), |p| p.file_name.clone());
                match self.photo_version(pid) {
                    Some(version) => Ok((
                        Id { key, version },
                        name,
                        Box::new(move |s: &mut Session| {
                            let job = s.render_job(pid, EDGE, EDGE, false, true).ok_or_else(|| format!("photo {} is not in the library", pid.0))?;
                            Ok(Job::Photo(Box::new(job)))
                        }),
                    )),
                    None => Err((format!("the background photo {name} is not in the library"), name)),
                }
            }
        })
    }

    /// Start loading Swap `with`'s picture on a worker, unless it is loaded, loading or failed in this version; its
    /// identity and name, or why it is missing.
    fn backdrop_start(&mut self, with: &Swap) -> Option<Result<(Id, String), String>> {
        let (id, name, job) = match self.backdrop_source(with)? {
            Ok(v) => v,
            Err((why, _)) => return Some(Err(why)),
        };
        let store = self.retouch_faces.backdrops.clone();
        if store.get(&id).is_some() || store.busy(&id) || store.failed(&id).is_some() {
            return Some(Ok((id, name)));
        }
        let job = match job(self) {
            Ok(job) => job,
            Err(e) => {
                store.put(&id, Err(e));
                return Some(Ok((id, name)));
            }
        };
        store.lock().busy.push(id.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let guard = self.activity.start("retouch.backdrop", "Loading background picture", crate::activity::Cancel::No);
            guard.detail(&name);
            let (worker, i) = (store.clone(), id.clone());
            let spawned = std::thread::Builder::new().name("retouch-backdrop".into()).spawn(move || {
                let result = crate::guard::catch("the background picture", || load(job)).and_then(|r| r);
                if let Err(e) = &result {
                    log::warn!("portrait: background picture not loaded: {e}");
                }
                worker.put(&i, result);
                drop(guard);
            });
            if let Err(e) = spawned {
                store.put(&id, Err(format!("could not start loading the background picture: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        store.put(&id, load(job));
        Some(Ok((id, name)))
    }

    /// Start loading Swap's picture in settings `s` on a worker, when it uses one and it is not loaded (Swap set, a
    /// photo with Swap opened before its faces are known).
    pub(crate) fn backdrop_prefetch(&mut self, s: &DevelopSettings) {
        if let Some(with) = s.retouch.global.background.with.picture().is_some().then(|| s.retouch.global.background.with.clone()) {
            let _ = self.backdrop_start(&with);
        }
    }

    /// Load photo `p`'s Swap picture (settings `s`) and depth map now, on this thread, when they are not loaded
    /// (synchronous renders: the CLI, MCP, tests). `Err` names a picture that is missing or could not be loaded.
    pub(crate) fn backdrop_now(&mut self, p: &Photo, s: &DevelopSettings) -> Result<(), String> {
        let store = self.retouch_faces.backdrops.clone();
        if let Some(path) = depth_path(p, s) {
            let st = stamp(Path::new(&path));
            if store.depth(&path, st).is_none() {
                store.put_depth(&path, st, read_depth_of(&store, &path, p));
            }
        }
        let Some(with) = swap_picture(s).cloned() else { return Ok(()) };
        let (id, name, job) = self.backdrop_source(&with).into_iter().next().ok_or("no picture")?.map_err(|(why, _)| why)?;
        if store.get(&id).is_some() {
            return Ok(());
        }
        if store.busy(&id) {
            return store.wait(&id).map(|_| ()).map_err(|e| format!("the background picture {name} could not be loaded: {e}"));
        }
        let result = job(self).and_then(|job| crate::guard::catch("the background picture", || load(job)).and_then(|r| r));
        let err = result.as_ref().err().map(|e| format!("the background picture {name} could not be loaded: {e}"));
        store.put(&id, result);
        err.map_or(Ok(()), Err)
    }

    /// `input` with what Background reads besides the faces, when it is loaded: Swap's picture and the photo's depth
    /// map. Starts loading what is not (`start`).
    pub(crate) fn with_backdrop(&mut self, input: Arc<Input>, p: &Photo, s: &DevelopSettings, start: bool) -> Arc<Input> {
        if !start {
            return self.with_backdrop_now(input, p, s);
        }
        let store = self.retouch_faces.backdrops.clone();
        let b = match swap_picture(s).cloned().and_then(|with| self.backdrop_start(&with)) {
            Some(Ok((id, _))) => store.get(&id),
            _ => None,
        };
        let d = match depth_path(p, s) {
            Some(path) => {
                let st = stamp(Path::new(&path));
                match store.depth(&path, st) {
                    Some(d) => d,
                    None => {
                        depth_load(&store, path, st, p, &self.activity);
                        None
                    }
                }
            }
            None => None,
        };
        store.with(&input, b, d)
    }

    /// [`Self::with_backdrop`] without starting anything (cache keys of renders made elsewhere).
    pub(crate) fn with_backdrop_now(&self, input: Arc<Input>, p: &Photo, s: &DevelopSettings) -> Arc<Input> {
        let store = &self.retouch_faces.backdrops;
        let b = swap_picture(s).and_then(|w| self.backdrop_source(w)).and_then(Result::ok).and_then(|(id, _, _)| store.get(&id));
        let d = depth_path(p, s).and_then(|path| store.depth(&path, stamp(Path::new(&path)))).flatten();
        store.with(&input, b, d)
    }

    /// For an export set up now and run later: start loading photo `id`'s Swap picture (it is waited for in the
    /// export's worker, [`ExportBackdrop::fill`]) and note its depth map. `None` when it needs neither.
    pub(crate) fn retouch_export_backdrop(&mut self, id: PhotoId) -> Option<ExportBackdrop> {
        let p = self.catalog.photo(id)?.clone();
        if !lightcraft_develop::retouch::active(&p.develop) {
            return None;
        }
        let s = p.develop.clone();
        let picture = match swap_picture(&s).cloned() {
            Some(with) => match self.backdrop_start(&with)? {
                Ok((i, name)) => Some(Picture::Wait(i, name)),
                Err(why) => Some(Picture::Missing(why)),
            },
            None => None,
        };
        let depth = depth_path(&p, &s).map(|path| {
            let st = stamp(Path::new(&path));
            (path, st, p.clone())
        });
        (picture.is_some() || depth.is_some()).then(|| ExportBackdrop {
            store: self.retouch_faces.backdrops.clone(),
            label: p.file_name.clone(),
            picture,
            depth,
        })
    }

    /// The state of Swap's picture in settings `s`: `{picture?, name?, loaded, loading, missing, error?}`.
    pub fn backdrop_status(&self, s: &DevelopSettings) -> Value {
        let b = &s.retouch.global.background;
        if b.with.picture().is_none() {
            return json!({"loaded": false, "loading": false, "missing": false});
        }
        let with = b.with.clone();
        let store = self.retouch_faces.backdrops.clone();
        match self.backdrop_source(&with) {
            Some(Ok((id, name, _))) => {
                let size = store.get(&id).map(|b| json!([b.image.width, b.image.height]));
                json!({"picture": id.key, "name": name, "loaded": size.is_some(), "size": size, "loading": store.busy(&id), "missing": false, "error": store.failed(&id)})
            }
            Some(Err((why, name))) => {
                json!({"picture": with.picture(), "name": name, "loaded": false, "loading": false, "missing": true, "error": why})
            }
            None => json!({"loaded": false, "loading": false, "missing": false}),
        }
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

    fn id(key: &str, version: u64) -> Id {
        Id { key: key.into(), version }
    }

    #[test]
    fn a_missing_or_broken_file_fails_and_never_panics() {
        let store = Backdrops::default();
        let r = load_file(Path::new("/no/such/picture.jpg"));
        assert!(r.is_err());
        store.put(&id("picture:a", 1), r);
        assert!(store.failed(&id("picture:a", 1)).is_some() && store.get(&id("picture:a", 1)).is_none());
        // (a new version of the source is tried again)
        assert!(store.failed(&id("picture:a", 2)).is_none());
        let junk = std::env::temp_dir().join(format!("lightcraft-backdrop-junk-{}.jpg", std::process::id()));
        if std::fs::write(&junk, b"\xff\xd8\xff not a jpeg").is_ok() {
            assert!(load_file(&junk).is_err());
            let _ = std::fs::remove_file(&junk);
        }
    }

    /// A picture whose header says it is over 2^26 pixels is refused before it is decoded.
    #[test]
    fn a_huge_picture_is_refused_from_its_header() {
        // a PNG header of 10000 × 10000 pixels and no pixel data
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend_from_slice(&10_000u32.to_be_bytes());
        ihdr.extend_from_slice(&10_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&[0; 4]);
        let file = std::env::temp_dir().join(format!("lightcraft-backdrop-huge-{}.png", std::process::id()));
        if std::fs::write(&file, &png).is_ok() {
            let r = load_file(&file);
            let _ = std::fs::remove_file(&file);
            match lightcraft_codecs::read_header(&png) {
                Ok(_) => assert!(r.as_ref().is_err_and(|e| e.contains("megapixels")), "{r:?}"),
                Err(_) => assert!(r.is_err()),
            }
        }
    }

    #[test]
    fn a_loaded_picture_is_kept_by_its_version_and_shared_by_the_faces_of_a_photo() {
        let store = Backdrops::default();
        store.put(&id("picture:a", 1), Ok(Rgb32f::filled(4, 3, [0.5; 3])));
        let b = store.get(&id("picture:a", 1)).unwrap();
        assert!(store.get(&id("picture:a", 2)).is_none(), "another version is loaded anew");
        let input = Arc::new(Input::new(Vec::new()));
        let (x, y) = (store.with(&input, Some(b.clone()), None), store.with(&input, Some(b.clone()), None));
        assert!(Arc::ptr_eq(&x, &y));
        assert_ne!(x.fingerprint(), input.fingerprint(), "the picture is in the render keys");
        for i in 0..6 {
            store.put(&id(&format!("picture:{i}"), 1), Ok(Rgb32f::filled(1, 1, [0.1; 3])));
        }
        assert!(store.get(&id("picture:a", 1)).is_none(), "only the newest are kept");
    }

    /// Pictures are kept within the byte cap; the newest always.
    #[test]
    fn the_kept_pictures_are_capped_by_bytes() {
        let store = Backdrops::default();
        // (each 160 MB)
        let big = || Ok(Rgb32f::new(4096, 3413));
        store.put(&id("picture:a", 1), big());
        store.put(&id("picture:b", 1), big());
        store.put(&id("picture:c", 1), big());
        assert!(store.get(&id("picture:c", 1)).is_some());
        let total: usize = store.lock().loaded.iter().map(|(_, b)| bytes_of(b)).sum();
        assert!(total <= KEEP_BYTES, "{total}");
        assert!(store.get(&id("picture:a", 1)).is_none());
    }

    #[test]
    fn an_export_waits_for_a_picture_another_worker_loads() {
        let store = Backdrops::default();
        let i = id("photo:3", 9);
        store.lock().busy.push(i.clone());
        let worker = store.clone();
        let j = i.clone();
        let t = std::thread::spawn(move || worker.put(&j, Ok(Rgb32f::filled(2, 2, [0.2; 3]))));
        let got = store.wait(&i);
        assert!(got.is_ok_and(|b| b.image.width == 2));
        let _ = t.join();
        store.lock().busy.push(id("photo:4", 1));
        store.put(&id("photo:4", 1), Err("broken".into()));
        assert_eq!(store.wait(&id("photo:4", 1)).err().as_deref(), Some("broken"));
    }

    #[test]
    fn a_depth_map_of_another_shape_is_noticed() {
        let d = |w, h| Depth { plane: Arc::new(lightcraft_raster::Plane::new(w, h)), key: 1 };
        assert!(depth_fits(&d(768, 576), 4032, 3024));
        assert!(depth_fits(&d(576, 768), 4032, 3024), "either way round");
        assert!(!depth_fits(&d(640, 640), 4032, 3024));
    }
}
