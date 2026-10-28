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
    if let Some(dir) = DIR_OVERRIDE.with(|d| d.borrow().clone()) {
        return Some(dir);
    }
    std::env::var_os("LIGHTCRAFT_CAMERA_PROFILES").map(PathBuf::from).or_else(|| config_dir().map(|d| d.join("camera-profiles")))
}

thread_local! {
    /// The folder tests read profiles from on this thread, instead of [`dir`]'s (so they never touch the user's own,
    /// and a test that chooses one never shows its folder to another test running at the same time).
    static DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Makes [`dir`] return `dir` on this thread until the guard is dropped. For tests: whatever reads the folder must
/// run on the test's own thread (the key of a profile is found again from the process-wide registry, not the folder).
pub struct DirGuard(());

impl Drop for DirGuard {
    fn drop(&mut self) {
        DIR_OVERRIDE.with(|d| *d.borrow_mut() = None);
    }
}

/// See [`DirGuard`].
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
    *KEY.get_or_init(|| {
        let mut h = lightcraft_preview::Hasher128::new();
        for (model, json) in BUNDLED {
            h.str(model).str(json);
        }
        h.finish().0 as u64
    })
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
    /// Changes with the file's content: part of the render keys of the photos on this profile.
    pub key: u64,
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

/// `(length, modified)` of a file read once: the entry is read again when either changes.
type FileStamp = (u64, std::time::Duration);

fn files() -> &'static Mutex<HashMap<PathBuf, (FileStamp, Arc<LocalProfile>)>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, (FileStamp, Arc<LocalProfile>)>>> = OnceLock::new();
    FILES.get_or_init(Default::default)
}

/// A profile file, read again only when its length or modification time changed.
fn read_local(path: &Path, id: &str) -> Option<Arc<LocalProfile>> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
    let stamp = (meta.len(), modified);
    let mut cache = files().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((seen, hit)) = cache.get(path)
        && *seen == stamp
        && hit.id == id
    {
        return Some(hit.clone());
    }
    let read = if stamp.0 > MAX_FILE {
        Err(format!("{}: {} bytes is too large for a camera profile", path.display(), stamp.0))
    } else {
        std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let (key, profile) = match read {
        Ok(bytes) => (lightcraft_preview::Hasher128::new().str(id).update(&bytes).finish().0 as u64, parse(&bytes, &path.display().to_string())),
        Err(e) => (lightcraft_preview::Hasher128::new().str(id).str("unreadable").finish().0 as u64, Err(e)),
    };
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
    if cache.len() >= 4 * MAX_LOCAL {
        cache.clear();
    }
    cache.insert(path.to_owned(), (stamp, local.clone()));
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
    let mut out: Vec<Arc<LocalProfile>> = stems.iter().filter_map(|(id, path)| read_local(path, id)).collect();
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
    read_local(&path, id)
}

/// What a photo's [`LoadOptions`](crate::media::LoadOptions) remembers of its local profile.
struct Registered {
    id: String,
    local: Option<Arc<LocalProfile>>,
}

fn registry() -> &'static Mutex<HashMap<u64, Arc<Registered>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<Registered>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// The key of local profile `id` as it is now: a hash of the id and the file's content (a missing file has a key of
/// its own). It is part of the photo's [`LoadOptions`](crate::media::LoadOptions), so a changed, replaced, removed or
/// restored file decodes the photo again, and of the render keys of the photos on the profile. The profile itself
/// is found again from the key by [`resolve_local`].
pub fn local_key(id: &str) -> u64 {
    let local = local_profile(id);
    let key = local.as_ref().map_or_else(|| lightcraft_preview::Hasher128::new().str(id).str("missing").finish().0 as u64, |l| l.key);
    let mut registry = registry().lock().unwrap_or_else(PoisonError::into_inner);
    if registry.len() >= 4096 {
        registry.clear();
    }
    registry.insert(key, Arc::new(Registered { id: id.to_owned(), local }));
    key
}

/// The profile a photo's [`local_key`] stands for, for a photo of camera `model`; `Err(id, why)` when it cannot be
/// used (the id is for the note). A key this process never made counts as a missing file.
pub fn resolve_local(key: u64, model: &str) -> Result<(String, Arc<CameraProfile>), (String, LocalUnavailable)> {
    let registered = registry().lock().unwrap_or_else(PoisonError::into_inner).get(&key).cloned();
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
mod tests {
    use super::*;

    fn profile() -> CameraProfile {
        let table = HsvTable { hue_divisions: 4, sat_divisions: 2, val_divisions: 1, data: vec![[5.0, 1.2, 1.0]; 8], srgb_value: false };
        CameraProfile::new("ILCE-7M4", 12, 3456, Mat3([[1.6, -0.5, -0.1], [-0.2, 1.3, -0.1], [-0.1, -0.2, 1.3]]), Some(table))
    }

    #[test]
    fn profiles_round_trip_and_reject_bad_files() {
        let dir = std::env::temp_dir().join(format!("lc-camera-profiles-{}", std::process::id()));
        let path = save(&profile(), &dir).unwrap();
        assert_eq!(path.file_name().unwrap(), "ILCE-7M4.json");
        assert_eq!(load(&path).unwrap(), profile());
        let bad = |p: CameraProfile| {
            let path = dir.join("bad.json");
            std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
            load(&path).is_err()
        };
        let mut p = profile();
        p.matrix[0][0] = f64::NAN;
        assert!(bad(p), "non-finite matrix");
        let mut p = profile();
        p.matrix = [[0.0; 3]; 3];
        assert!(bad(p), "singular matrix");
        let mut p = profile();
        if let Some(t) = p.hue_sat.as_mut() {
            t.data.truncate(3);
        }
        assert!(bad(p), "table data doesn't match its shape");
        let mut p = profile();
        p.version = 99;
        assert!(bad(p), "unknown version");
        std::fs::write(dir.join("junk.json"), b"{not json").unwrap();
        assert!(load(&dir.join("junk.json")).is_err());
        assert!(load(&dir.join("missing.json")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_profiles_are_valid_and_named_after_their_model() {
        assert!(!BUNDLED.is_empty());
        for (model, json) in BUNDLED {
            let p = parse(json.as_bytes(), model).unwrap();
            assert_eq!(p.model, *model);
            assert!(p.files >= 5 && p.hue_sat.is_some(), "{model}: {} photos", p.files);
            assert_eq!(bundled(model), Some(p));
        }
        assert!(bundled("No Such Camera").is_none());
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lc-local-profiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, file: &str, p: &CameraProfile) -> PathBuf {
        let path = dir.join(file);
        std::fs::write(&path, serde_json::to_vec(p).unwrap()).unwrap();
        path
    }

    fn for_model(model: &str, name: Option<&str>) -> CameraProfile {
        let mut p = profile();
        p.model = model.into();
        p.name = name.map(Into::into);
        p
    }

    /// LT2: every valid `*.json` directly in the folder is a local profile, named from its `name` (else the file
    /// name), found for the cameras it is made for; damaged files are listed as unusable and never offered.
    #[test]
    fn local_profiles_are_named_listed_and_matched_by_camera() {
        let dir = temp("list");
        put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("  A7 V Chart  ")));
        put(&dir, "plain.json", &for_model("ILCE-7M5", None));
        put(&dir, "blank name.json", &for_model("ILCE-7M5", Some("   ")));
        put(&dir, "long.json", &for_model("ILCE-7M4", Some(&"x".repeat(200))));
        put(&dir, "UPPER.JSON", &for_model("ILCE-7M4", Some("Upper Case Extension")));
        std::fs::write(dir.join("junk.json"), b"{not json").unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        std::fs::write(dir.join(".hidden.json"), serde_json::to_vec(&for_model("ILCE-7M5", None)).unwrap()).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        put(&dir.join("sub"), "nested.json", &for_model("ILCE-7M5", None));
        std::fs::create_dir_all(dir.join("folder.json")).unwrap();
        let _dir = use_dir_for_tests(&dir);
        let all = local_profiles();
        let listed: Vec<_> = all.iter().map(|p| (p.id.as_str(), p.name.chars().count(), p.valid())).collect();
        assert_eq!(
            listed,
            [
                ("A7 V Chart", 10, true),
                ("blank name", 10, true),
                ("junk", 4, false),
                ("plain", 5, true),
                ("UPPER", 20, true),
                ("long", MAX_NAME, true)
            ],
            "sorted by display name; the stem is the id; a long name is cut; junk, notes, hidden files, folders and sub-folders are not profiles"
        );
        let names = |model: &str| local_profiles_for(model).iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("ILCE-7M5"), ["A7 V Chart", "blank name", "plain"]);
        assert_eq!(names("  ILCE-7M5 "), ["A7 V Chart", "blank name", "plain"], "the camera model is compared trimmed");
        assert_eq!(names("ILCE-7M4").len(), 2);
        assert!(names("ILCE-7CR").is_empty() && names("").is_empty());
        let by_camera = |camera: &str| local_profiles_for_camera(camera).len();
        assert_eq!((by_camera("SONY ILCE-7M5"), by_camera("ILCE-7M5"), by_camera("SONY ILCE-7M4"), by_camera("SONY XILCE-7M5")), (3, 3, 2, 0));
        let junk = local_profile("junk").unwrap();
        assert!(!junk.valid() && !junk.is_for("ILCE-7M5"));
        assert!(local_profile("nothing").is_none() && local_profile("../A7 V Chart").is_none() && local_profile("sub/nested").is_none());
        drop(_dir);
        let _empty = use_dir_for_tests(&dir.join("does-not-exist"));
        assert!(local_profiles().is_empty(), "no folder, no profiles");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// LT3: Camera Matching reads only the built-in profiles: a local `ILCE-7M4.json` does not replace the built-in
    /// one, and a local file for a camera that has none gives Camera Matching nothing.
    #[test]
    fn camera_matching_reads_only_the_built_in_profiles() {
        let dir = temp("get");
        let mut mine = for_model("ILCE-7M4", None);
        mine.files = 999;
        put(&dir, "ILCE-7M4.json", &mine);
        put(&dir, "ILCE-7M5.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
        let _dir = use_dir_for_tests(&dir);
        let used = get("ILCE-7M4").expect("a built-in profile for the model");
        assert_eq!(used.files, bundled("ILCE-7M4").unwrap().files);
        assert_ne!(used.files, 999, "the local file did not replace it");
        assert!(get("ILCE-7M5").is_none(), "a local profile of a camera without a built-in one is no input of Camera Matching");
        assert!(local_profiles_for("ILCE-7M5").iter().any(|p| p.name == "A7 V Chart"), "it is a local profile instead");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// LT4 (profiles half): the render key every photo carries is the built-in profiles' alone, the same value as
    /// before local profiles were choices, whatever the user's folder holds.
    #[test]
    fn the_render_key_of_camera_matching_ignores_the_users_folder() {
        let formula = || {
            // (the key as it was for a user whose folder was empty)
            let mut h = lightcraft_preview::Hasher128::new();
            for (model, json) in BUNDLED {
                h.str(model).str(json);
            }
            h.finish().0 as u64
        };
        let dir = temp("key");
        let _dir = use_dir_for_tests(&dir);
        let before = cache_key();
        put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
        put(&dir, "ILCE-7M4.json", &for_model("ILCE-7M4", None));
        assert_eq!(cache_key(), before);
        assert_eq!(before, formula());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// LT4 / LT8 (profiles half): a local profile's key follows its file's content, also for a same-second,
    /// same-length replacement, and a removed file has a key of its own; restoring the file restores the key. The
    /// profile is found again from the key, and says why it cannot be used.
    #[test]
    fn a_local_profile_key_follows_the_file_and_resolves_to_the_profile() {
        let dir = temp("local-key");
        let _dir = use_dir_for_tests(&dir);
        let path = put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
        let stamp = |millis| {
            let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000) + std::time::Duration::from_millis(millis);
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
        };
        stamp(100);
        let first = local_key("A7 V Chart");
        assert_eq!(local_key("A7 V Chart"), first, "the same file, the same key");
        assert_ne!(first, local_key("Another"), "the id is part of it");
        assert!(resolve_local(first, "ILCE-7M5").is_ok());
        // a replacement of the same length within the same second
        let mut second = for_model("ILCE-7M5", Some("A7 V Chart"));
        second.files = 13;
        let bytes = serde_json::to_vec(&second).unwrap();
        assert_eq!(bytes.len(), serde_json::to_vec(&for_model("ILCE-7M5", Some("A7 V Chart"))).unwrap().len(), "same length");
        std::fs::write(&path, bytes).unwrap();
        stamp(350);
        let replaced = local_key("A7 V Chart");
        assert_ne!(replaced, first);
        assert_eq!(resolve_local(replaced, "ILCE-7M5").unwrap().1.files, 13);
        // removed: a key of its own, and the reason
        let original = serde_json::to_vec(&for_model("ILCE-7M5", Some("A7 V Chart"))).unwrap();
        std::fs::remove_file(&path).unwrap();
        let missing = local_key("A7 V Chart");
        assert!(missing != first && missing != replaced);
        assert_eq!(resolve_local(missing, "ILCE-7M5").unwrap_err(), ("A7 V Chart".to_owned(), LocalUnavailable::Missing));
        // restored
        std::fs::write(&path, original).unwrap();
        stamp(100);
        assert_eq!(local_key("A7 V Chart"), first, "the file is back: the profile is used again");
        // other cameras, unusable files, hostile ids, unknown keys
        assert_eq!(resolve_local(first, "ILCE-7M4").unwrap_err().1, LocalUnavailable::OtherCamera);
        std::fs::write(dir.join("broken.json"), b"[]").unwrap();
        assert_eq!(resolve_local(local_key("broken"), "ILCE-7M5").unwrap_err().1, LocalUnavailable::Invalid);
        assert_eq!(resolve_local(local_key("../A7 V Chart"), "ILCE-7M5").unwrap_err().1, LocalUnavailable::Missing);
        assert_eq!(resolve_local(0x5eed, "ILCE-7M5").unwrap_err().1, LocalUnavailable::Missing);
        assert!(LocalUnavailable::Missing.note("A7 V Chart").contains("isn't available on this computer"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `calibrate --name`: the file is `<name>.json` with the name inside, path characters in it are made harmless,
    /// and it appears as a local profile of that name.
    #[test]
    fn a_named_profile_is_saved_as_a_local_profile() {
        let dir = temp("named");
        let _dir = use_dir_for_tests(&dir);
        let path = save_named(&for_model("ILCE-7M5", None), &dir, "A7 V Chart").unwrap();
        assert_eq!(path.file_name().unwrap(), "A7 V Chart.json");
        assert_eq!(load(&path).unwrap().name.as_deref(), Some("A7 V Chart"));
        let hostile = save_named(&for_model("ILCE-7M5", None), &dir, "../../evil/name").unwrap();
        assert_eq!(hostile.parent().unwrap(), dir, "stays in the folder");
        assert!(save_named(&for_model("ILCE-7M5", None), &dir, "   ").is_err());
        assert!(save_named(&for_model("ILCE-7M5", None), &dir, "...").is_err());
        let ids: Vec<_> = local_profiles_for("ILCE-7M5").iter().map(|p| p.name.clone()).collect();
        assert!(ids.contains(&"A7 V Chart".to_owned()) && ids.len() == 2, "{ids:?}");
        let plain = save(&for_model("ILCE-7M5", None), &dir).unwrap();
        assert_eq!(plain.file_name().unwrap(), "ILCE-7M5.json", "without a name the file is the model's, as before");
        assert!(!std::fs::read_to_string(&plain).unwrap().contains("\"name\""), "no name field for an unnamed profile");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("ILCE-7M4"), "ILCE-7M4.json");
        assert_eq!(file_name(" DSC-RX100M3 "), "DSC-RX100M3.json");
        assert_eq!(file_name("../../etc/passwd"), "______etc_passwd.json");
        assert_eq!(file_name("Ω 1/2"), "__1_2.json");
    }

    #[test]
    fn pool_skips_files_that_cannot_contribute() {
        let mut pool = Pool::default();
        assert!(pool.add(b"not a raw file").is_err());
        assert!(pool.fit(1).is_empty());
        assert!(pool.files().is_empty());
    }
}
