//! Camera colour profiles of our own, pooled from many photos of one camera model.
//!
//! A raw file without colour matrices (Sony ARW, Nikon NEF, Fujifilm RAF) gets its look fitted to its own embedded camera
//! JPEG (`camera_preview`), but one photo shows too little of some colours: a lime shirt covering a
//! few dozen proxy pixels next to a hillside of foliage at the same hue. `lightcraft-cli calibrate`
//! pools the colour pairs of many photos per model and fits one matrix and hue/saturation/value
//! table; photos of that model then only fit their tone and chroma curves.
//!
//! Profiles are JSON files. Camera Matching reads the ones built in ([`BUNDLED`], from
//! `assets/camera-profiles/`) through [`get`]. The files in [`dir`] (`LIGHTCRAFT_CAMERA_PROFILES`, else
//! `<config>/camera-profiles`) are *local* profiles: each is its own choice in the Profile picker
//! (`CameraColour::Local`) for the cameras it is made for, named from its `name` field or its file name, and never
//! changes Camera Matching ([`local_profiles`]). A damaged or hostile file is ignored with a warning. Profiles hold
//! aggregate colour statistics only, never image content.
use lightcraft_color::Mat3;
use lightcraft_raw::profile::HsvTable;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Format version of the profile files.
const VERSION: u32 = 1;
/// Largest profile file read (a 5 × 72 × 5 table is ~60 KB of JSON).
const MAX_FILE: u64 = 4 << 20;

/// Profiles built into LightCraft (`assets/camera-profiles/`, see `assets/ATTRIBUTION.md`):
/// `(model, JSON)`.
pub const BUNDLED: &[(&str, &str)] = &[
    ("ILCA-99M2", include_str!("../../../assets/camera-profiles/ILCA-99M2.json")),
    ("ILCE-7CR", include_str!("../../../assets/camera-profiles/ILCE-7CR.json")),
    ("ILCE-7M4", include_str!("../../../assets/camera-profiles/ILCE-7M4.json")),
    ("ILCE-7RM5", include_str!("../../../assets/camera-profiles/ILCE-7RM5.json")),
    ("NIKON D5300", include_str!("../../../assets/camera-profiles/NIKON_D5300.json")),
    ("NIKON Z 6_2", include_str!("../../../assets/camera-profiles/NIKON_Z_6_2.json")),
    ("NIKON Z 8", include_str!("../../../assets/camera-profiles/NIKON_Z_8.json")),
    ("X-H2S", include_str!("../../../assets/camera-profiles/X-H2S.json")),
    ("X-T4", include_str!("../../../assets/camera-profiles/X-T4.json")),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraProfile {
    pub version: u32,
    /// The camera model as the files report it (Exif `Model`), e.g. `ILCE-7M4`.
    pub model: String,
    /// What a local profile is called in the Profile picker (file `name` field); the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Photos and colour pairs the profile was fitted on.
    pub files: usize,
    pub samples: usize,
    /// White-balanced camera RGB (with the baseline exposure) → linear Rec.2020, rows.
    matrix: [[f64; 3]; 3],
    /// Hue/saturation/value correction after `matrix` (linear ProPhoto RGB).
    pub hue_sat: Option<HsvTable>,
}

impl CameraProfile {
    pub fn new(model: &str, files: usize, samples: usize, matrix: Mat3, hue_sat: Option<HsvTable>) -> CameraProfile {
        CameraProfile { version: VERSION, model: model.to_owned(), name: None, files, samples, matrix: matrix.0, hue_sat }
    }

    /// The profile under `name` in the Profile picker.
    pub fn named(mut self, name: &str) -> CameraProfile {
        let name = name.trim();
        self.name = (!name.is_empty()).then(|| name.chars().take(MAX_NAME).collect());
        self
    }

    pub fn matrix(&self) -> Mat3 {
        Mat3(self.matrix)
    }

    /// Whether the data is usable (bounded matrix, table shape matching its data).
    fn valid(&self) -> bool {
        let matrix = self.matrix.iter().flatten().all(|v| v.is_finite() && v.abs() < 8.0) && Mat3(self.matrix).inverse().is_some();
        let table = self.hue_sat.as_ref().is_none_or(|t| {
            let dims = (1..=4096).contains(&t.hue_divisions) && (2..=4096).contains(&t.sat_divisions) && (1..=4096).contains(&t.val_divisions);
            let len = t.hue_divisions.checked_mul(t.sat_divisions).and_then(|n| n.checked_mul(t.val_divisions));
            dims && len == Some(t.data.len()) && t.data.iter().flatten().all(|v| v.is_finite()) && t.data.iter().all(|e| e[1] >= 0.0 && e[2] >= 0.0)
        });
        self.version == VERSION && !self.model.is_empty() && matrix && table
    }
}

/// LightCraft's configuration folder (settings, GPU marker, camera profiles); one helper for the app.
pub use crate::config::config_dir;

/// Where camera profiles are read from and written to.
pub fn dir() -> Option<PathBuf> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(dir) = DIR_OVERRIDE.with(|d| d.borrow().clone()) {
        return Some(dir);
    }
    std::env::var_os("LIGHTCRAFT_CAMERA_PROFILES").map(PathBuf::from).or_else(|| config_dir().map(|d| d.join("camera-profiles")))
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    /// The folder tests read profiles from on this thread, instead of [`dir`]'s (so they never touch the user's own,
    /// and a test that chooses one never shows its folder to another test running at the same time).
    static DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Makes [`dir`] return `dir` on this thread until the guard is dropped. Tests only: it exists in the engine's own
/// tests and in builds with the `test-hooks` feature (a dev-dependency of the crates whose tests choose a folder), never
/// in the app. Whatever reads the folder must run on the test's own thread.
#[cfg(any(test, feature = "test-hooks"))]
pub struct DirGuard(());

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for DirGuard {
    fn drop(&mut self) {
        DIR_OVERRIDE.with(|d| *d.borrow_mut() = None);
    }
}

/// See [`DirGuard`].
#[cfg(any(test, feature = "test-hooks"))]
pub fn use_dir_for_tests(dir: &Path) -> DirGuard {
    DIR_OVERRIDE.with(|d| *d.borrow_mut() = Some(dir.to_owned()));
    DirGuard(())
}

/// File name of `model`'s profile: letters, digits, `-` and `_` kept, anything else `_`.
pub fn file_name(model: &str) -> String {
    let name: String = model.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{name}.json")
}

/// Most characters of a local profile's id (its file stem) and of its display name.
const MAX_NAME: usize = 60;

/// Whether `id` can be the stem of a local profile file: not empty, short, no path separators or control characters
/// (it becomes `<dir>/<id>.json`, so a hostile id must never leave the folder).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= 2 * MAX_NAME
        && id == id.trim()
        && !id.starts_with('.')
        && !id.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
}

/// Read and validate a profile file.
pub fn load(path: &Path) -> Result<CameraProfile, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > MAX_FILE {
        return Err(format!("{}: {size} bytes is too large for a camera profile", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes, &path.display().to_string())
}

/// Parse and validate a profile's JSON (`origin` names it in errors).
fn parse(json: &[u8], origin: &str) -> Result<CameraProfile, String> {
    let profile: CameraProfile = serde_json::from_slice(json).map_err(|e| format!("{origin}: {e}"))?;
    if !profile.valid() {
        return Err(format!("{origin}: not a usable camera profile (version {}, model {:?})", profile.version, profile.model));
    }
    Ok(profile)
}

/// The built-in profile for `model`, if any.
fn bundled(model: &str) -> Option<CameraProfile> {
    let (_, json) = BUNDLED.iter().find(|(m, _)| *m == model)?;
    parse(json.as_bytes(), &format!("built-in profile {model}")).inspect_err(|e| eprintln!("lightcraft: ignoring camera profile {e}")).ok()
}

/// Write `profile` to `dir` (created if needed) as `<model>.json`; returns the path.
pub fn save(profile: &CameraProfile, dir: &Path) -> Result<PathBuf, String> {
    write_profile(profile, dir, &file_name(&profile.model))
}

/// Write `profile` to `dir` under `name`: the file is `<name>.json` (path separators and other unsafe characters
/// become `_`) and the profile's `name` field is `name`, so it shows in the Profile picker as "name".
pub fn save_named(profile: &CameraProfile, dir: &Path, name: &str) -> Result<PathBuf, String> {
    let stem: String = name.trim().chars().take(MAX_NAME).map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':') { '_' } else { c }).collect();
    let stem = stem.trim_start_matches('.').trim().to_owned();
    if !valid_id(&stem) {
        return Err(format!("{name:?} can't be a profile name"));
    }
    write_profile(&profile.clone().named(name), dir, &format!("{stem}.json"))
}

fn write_profile(profile: &CameraProfile, dir: &Path, file: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file);
    let json = serde_json::to_vec_pretty(profile).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn cache() -> &'static Mutex<HashMap<String, Option<Arc<CameraProfile>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<CameraProfile>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The built-in profile for camera `model`, if any; read once per process. Camera Matching uses this and nothing
/// else: the user's own files are [`local_profiles`].
pub fn get(model: &str) -> Option<Arc<CameraProfile>> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hit) = cache.get(model) {
        return hit.clone();
    }
    let profile = bundled(model).map(Arc::new);
    cache.insert(model.to_owned(), profile.clone());
    profile
}

/// Changes when the built-in profiles change: part of the render cache keys of every photo. The user's own profile
/// files are not in it (they are [`local_key`]s of the photos that use them), so adding one never re-renders a
/// photo on Camera Matching.
pub fn cache_key() -> u64 {
    static KEY: OnceLock<u64> = OnceLock::new();
    *KEY.get_or_init(bundled_key)
}

/// [`cache_key`] without the once-per-process memo: nothing but the built-in profiles goes into it.
fn bundled_key() -> u64 {
    let mut h = lightcraft_preview::Hasher128::new();
    for (model, json) in BUNDLED {
        h.str(model).str(json);
    }
    h.finish().0 as u64
}

/// A profile file in [`dir`], read and checked: one choice in the Profile picker.
#[derive(Debug)]
pub struct LocalProfile {
    /// The file name without `.json`: what a photo stores to name it.
    pub id: String,
    /// What the picker calls it: the file's `name`, else [`Self::id`].
    pub name: String,
    /// The Exif model of the cameras it is for (empty for a file that could not be read).
    pub model: String,
    /// Changes with the file's content (the full 128-bit hash of the id and the bytes): part of the render keys of the
    /// photos on this profile.
    pub key: u128,
    /// The checked profile, or why the file is not usable.
    pub profile: Result<Arc<CameraProfile>, String>,
}

impl LocalProfile {
    pub fn valid(&self) -> bool {
        self.profile.is_ok()
    }

    /// Whether the profile is for camera `model` (Exif `Model`, trimmed).
    pub fn is_for(&self, model: &str) -> bool {
        self.valid() && self.model.trim() == model.trim()
    }

    /// Whether the profile is for a photo whose catalog camera is `camera`: the Exif make and model joined
    /// (`SONY ILCE-7M5`, `lightcraft_catalog::Meta::camera`), so the model is all of it or its last words.
    pub fn is_for_camera(&self, camera: &str) -> bool {
        let (camera, model) = (camera.trim(), self.model.trim());
        self.valid() && !model.is_empty() && (camera == model || camera.strip_suffix(model).is_some_and(|rest| rest.ends_with(' ')))
    }
}

/// Why a photo cannot use its local profile, and so renders as Camera Matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalUnavailable {
    /// No file of that name in the folder (the folder is empty, on another computer, or the file was renamed).
    Missing,
    /// The file is not a usable profile.
    Invalid,
    /// The file is a profile for another camera.
    OtherCamera,
}

impl LocalUnavailable {
    /// The permanent note under the Profile picker, in plain words (the setting stays as it is).
    pub fn note(&self, id: &str) -> String {
        match self {
            LocalUnavailable::Missing => {
                format!("Edited with the profile '{id}', which isn't available on this computer. This picture uses Camera Matching.")
            }
            LocalUnavailable::Invalid => format!("Edited with the profile '{id}', whose file can't be used. This picture uses Camera Matching."),
            LocalUnavailable::OtherCamera => {
                format!("Edited with the profile '{id}', which is made for another camera. This picture uses Camera Matching.")
            }
        }
    }
}

/// Most profile files read from the folder (a hostile folder must not make every refresh read gigabytes).
const MAX_LOCAL: usize = 256;

/// `(length, modified)` of a file as it was when it was read.
type FileStamp = (u64, std::time::Duration);

/// A profile file as it was read, with the stamp it had.
struct Cached {
    stamp: FileStamp,
    local: Arc<LocalProfile>,
}

fn files() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    FILES.get_or_init(Default::default)
}

/// A profile file. With `verify` the content is always read and hashed, so a replacement of the same length and
/// modification time (a copy that keeps its time, a coarse file system clock) is seen too; without it a file whose
/// length and time are unchanged is not read again (the listing the picker draws every frame). The file is read with no
/// lock held. A file that is gone leaves no entry behind.
fn read_local(path: &Path, id: &str, verify: bool) -> Option<Arc<LocalProfile>> {
    let Some(meta) = std::fs::metadata(path).ok().filter(|m| m.is_file()) else {
        files().lock().unwrap_or_else(PoisonError::into_inner).remove(path);
        return None;
    };
    let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
    let stamp = (meta.len(), modified);
    let cached = files().lock().unwrap_or_else(PoisonError::into_inner).get(path).filter(|c| c.local.id == id).map(|c| (c.stamp, c.local.clone()));
    if !verify
        && let Some((seen, hit)) = &cached
        && *seen == stamp
    {
        return Some(hit.clone());
    }
    let read = if stamp.0 > MAX_FILE {
        Err(format!("{}: {} bytes is too large for a camera profile", path.display(), stamp.0))
    } else {
        std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let key = match &read {
        Ok(bytes) => lightcraft_preview::Hasher128::new().str(id).update(bytes).finish().0,
        Err(_) => lightcraft_preview::Hasher128::new().str(id).str("unreadable").finish().0,
    };
    if let Some((_, hit)) = &cached
        && hit.key == key
    {
        // the same content under a new stamp: keep what was parsed
        if let Some(entry) = files().lock().unwrap_or_else(PoisonError::into_inner).get_mut(path) {
            entry.stamp = stamp;
        }
        return Some(hit.clone());
    }
    let profile = read.and_then(|bytes| parse(&bytes, &path.display().to_string()));
    if let Err(e) = &profile {
        eprintln!("lightcraft: ignoring camera profile {e}");
    }
    let (name, model) = match &profile {
        Ok(p) => (
            p.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| id.to_owned(), |n| n.chars().take(MAX_NAME).collect()),
            p.model.trim().to_owned(),
        ),
        Err(_) => (id.to_owned(), String::new()),
    };
    let local = Arc::new(LocalProfile { id: id.to_owned(), name, model, key, profile: profile.map(Arc::new) });
    let mut cache = files().lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= 4 * MAX_LOCAL {
        cache.clear();
    }
    cache.insert(path.to_owned(), Cached { stamp, local: local.clone() });
    Some(local)
}

/// Every `*.json` directly in [`dir`], valid or not, by display name: the user's local profiles. The id is the file
/// stem. Cheap to call often (a file is read again only when it changed).
pub fn local_profiles() -> Vec<Arc<LocalProfile>> {
    let Some(dir) = dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut stems: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).filter(|s| valid_id(s)).map(|s| (s.to_owned(), p.clone())))
        .collect();
    stems.sort();
    stems.truncate(MAX_LOCAL);
    let mut out: Vec<Arc<LocalProfile>> = stems.iter().filter_map(|(id, path)| read_local(path, id, false)).collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    out
}

/// The local profiles that are for camera `model` and usable: the rows of its photos' Profile picker.
pub fn local_profiles_for(model: &str) -> Vec<Arc<LocalProfile>> {
    local_profiles().into_iter().filter(|p| p.is_for(model)).collect()
}

/// [`local_profiles_for`] by the catalog's camera string (`SONY ILCE-7M5`): what a photo's Profile picker lists.
pub fn local_profiles_for_camera(camera: &str) -> Vec<Arc<LocalProfile>> {
    local_profiles().into_iter().filter(|p| p.is_for_camera(camera)).collect()
}

/// The local profile file `<id>.json`, whether usable or not; `None` when there is no such file (or `id` is no file
/// stem: a stored id is hostile input and never leaves the folder).
pub fn local_profile(id: &str) -> Option<Arc<LocalProfile>> {
    if !valid_id(id) {
        return None;
    }
    let path = dir()?.join(format!("{id}.json"));
    read_local(&path, id, true)
}

/// What a photo's [`LoadOptions`](crate::media::LoadOptions) remembers of its local profile.
struct Registered {
    id: String,
    local: Option<Arc<LocalProfile>>,
}

/// The profiles keys were made for: the decoder gets only the key and finds the profile here. A key stays as long as
/// the process runs, except that an id keeps its newest [`KEYS_PER_ID`] (a file edited that often while a render of an
/// older version is still in flight does not happen); a key that is gone is found again from the folder.
#[derive(Default)]
struct Registry {
    by_key: HashMap<u128, Arc<Registered>>,
    by_id: HashMap<String, std::collections::VecDeque<u128>>,
}

/// Versions of one profile file the registry keeps.
const KEYS_PER_ID: usize = 8;
/// Most keys the registry holds (a catalog full of junk ids must not grow it without end).
const MAX_REGISTERED: usize = 100_000;

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn register(key: u128, registered: Registered) {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    if r.by_key.contains_key(&key) {
        return;
    }
    if r.by_key.len() >= MAX_REGISTERED {
        return;
    }
    let id = registered.id.clone();
    r.by_key.insert(key, Arc::new(registered));
    let keys = r.by_id.entry(id).or_default();
    keys.push_back(key);
    let old = if keys.len() > KEYS_PER_ID { keys.pop_front() } else { None };
    if let Some(old) = old {
        r.by_key.remove(&old);
    }
}

/// The key of local profile `id` as it is now: a hash of the id and the file's content (a missing file has a key of
/// its own), read from the file every time, so a replaced file is seen whatever its time says. It is part of the photo's
/// [`LoadOptions`](crate::media::LoadOptions), so a changed, replaced, removed or restored file decodes the photo again,
/// and of the render keys of the photos on the profile. The profile itself is found again from the key by
/// [`resolve_local`].
pub fn local_key(id: &str) -> u128 {
    let local = local_profile(id);
    let key = local.as_ref().map_or_else(|| lightcraft_preview::Hasher128::new().str(id).str("missing").finish().0, |l| l.key);
    register(key, Registered { id: id.to_owned(), local });
    key
}

/// The profile a photo's [`local_key`] stands for, for a photo of camera `model`; `Err(id, why)` when it cannot be
/// used (the id is for the note). A key the registry no longer holds is looked for among the folder's files; one that is
/// in neither counts as a missing file.
pub fn resolve_local(key: u128, model: &str) -> Result<(String, Arc<CameraProfile>), (String, LocalUnavailable)> {
    let held = registry().lock().unwrap_or_else(PoisonError::into_inner).by_key.get(&key).cloned();
    let registered = held.or_else(|| {
        let found = local_profiles().into_iter().find(|l| l.key == key)?;
        register(key, Registered { id: found.id.clone(), local: Some(found.clone()) });
        Some(Arc::new(Registered { id: found.id.clone(), local: Some(found) }))
    });
    let Some(registered) = registered else { return Err((String::new(), LocalUnavailable::Missing)) };
    let id = registered.id.clone();
    match registered.local.as_deref() {
        None => Err((id, LocalUnavailable::Missing)),
        Some(l) => match &l.profile {
            Err(_) => Err((id, LocalUnavailable::Invalid)),
            Ok(p) if l.is_for(model) => Ok((id, p.clone())),
            Ok(_) => Err((id, LocalUnavailable::OtherCamera)),
        },
    }
}

/// Forget every key (tests: what a registry that lost a key looks like).
#[cfg(test)]
fn forget_registry() {
    *registry().lock().unwrap_or_else(PoisonError::into_inner) = Registry::default();
}

/// Per camera model: the photos read and their pooled colour pairs.
#[derive(Default)]
pub struct Pool {
    models: HashMap<String, (usize, Vec<([f64; 3], [f64; 3])>)>,
}

/// Most colour pairs kept per photo, so a few busy photos can't dominate a profile.
const PAIRS_PER_FILE: usize = 4000;

impl Pool {
    /// Add one raw file's colour pairs. `Ok(None)` when the file can't contribute (not an ARW,
    /// NEF or RAF without colour matrices, no usable camera JPEG, too little colour).
    pub fn add(&mut self, bytes: &[u8]) -> Result<Option<String>, String> {
        let mut raw = lightcraft_raw::decode(bytes).map_err(|e| e.to_string())?;
        // colour only: geometric lens corrections would stop the sensor proxy from binning (the pairs still follow
        // the warp the camera applied to its JPEG)
        let lens = crate::camera_preview::lens_geometry(&raw.opcodes.list3);
        raw.opcodes.list3.retain(|op| !op.is_lens_correction());
        let Some(model) = raw.metadata.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) else { return Ok(None) };
        let Some(pairs) = crate::camera_preview::profile_pairs(&raw, bytes, lens.as_ref()) else { return Ok(None) };
        let step = pairs.len().div_ceil(PAIRS_PER_FILE).max(1);
        let entry = self.models.entry(model.to_owned()).or_default();
        entry.0 += 1;
        entry.1.extend(pairs.into_iter().step_by(step));
        Ok(Some(model.to_owned()))
    }

    /// Fit a profile per model with at least `min_files` photos.
    pub fn fit(&self, min_files: usize) -> Vec<Result<CameraProfile, String>> {
        let mut models: Vec<_> = self.models.iter().collect();
        models.sort_by(|a, b| a.0.cmp(b.0));
        models
            .into_iter()
            .filter(|(_, (files, _))| *files >= min_files)
            .map(|(model, (files, pairs))| {
                let (matrix, hue_sat) = crate::camera_preview::fit_profile(pairs).ok_or_else(|| format!("{model}: no usable colour fit"))?;
                Ok(CameraProfile::new(model, *files, pairs.len(), matrix, hue_sat))
            })
            .collect()
    }

    /// Add another pool's photos and pairs (pools filled on separate threads).
    pub fn merge(&mut self, other: Pool) {
        for (model, (files, pairs)) in other.models {
            let entry = self.models.entry(model).or_default();
            entry.0 += files;
            entry.1.extend(pairs);
        }
    }

    /// Photos read per model.
    pub fn files(&self) -> Vec<(String, usize)> {
        let mut v: Vec<_> = self.models.iter().map(|(m, (n, _))| (m.clone(), *n)).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests;
