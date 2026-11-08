//! Fork feature (`fork/learn-my-edits.md`): Learn My Edits. Profiles trained on the user's own edited photos
//! ([`lightcraft_learn`]) that apply as ordinary slider values.
//!
//! [`LearnEdits`] keeps the profiles (one `.lclearn` file each in `<config>/learn-profiles`), the image encoder
//! (DINOv2-small, `ArtCraft/models/dinov2-small`, shared with PhotoCraft; downloaded only when asked), and one
//! background job at a time ([`jobs`]): collecting examples, training, or working out a profile's sliders for
//! photos. Each job has a row in the activity stack that ✕ stops. An apply's results are committed on the
//! session's thread by `learn.pump` (the app calls it every frame) or at once with `wait`.
//!
//! Never crashes: a missing or damaged model or profile, a failed render, a cancelled or panicking job are errors
//! the window and the commands show.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, WbMode};
use lightcraft_learn::Profile;

use crate::Session;
use crate::activity::Activity;

mod download;
pub mod jobs;

pub use crate::lama_fill::Download;

/// Most profile files listed.
const MAX_PROFILES: usize = 200;
/// Most photos' features kept in memory.
const MAX_CACHED: usize = 2000;

/// A profile as listed: its id (the file's name without `.lclearn`) and the profile, or why it can't be read.
#[derive(Clone)]
pub struct Listed {
    pub id: String,
    pub path: PathBuf,
    pub profile: Result<Arc<Profile>, String>,
}

type Stamp = (u64, Option<SystemTime>);

struct Inner {
    activity: Activity,
    model_dir: Mutex<Option<PathBuf>>,
    profiles_dir: Mutex<Option<PathBuf>>,
    #[cfg(not(target_arch = "wasm32"))]
    encoder: Mutex<Option<Arc<lightcraft_learn::encoder::Encoder>>>,
    download: Mutex<Download>,
    /// Profiles read, by file, with the file's size and time when read.
    read: Mutex<HashMap<PathBuf, (Stamp, Result<Arc<Profile>, String>)>>,
    /// Features of photos at their defaults, by photo and the defaults' hash.
    features: Mutex<HashMap<(PhotoId, u64), Arc<Vec<f32>>>>,
    job: Mutex<Option<jobs::Running>>,
    /// The last thing worth telling the user (a job's result or failure).
    message: Mutex<Option<String>>,
    /// Tests only: features without the encoder.
    statistics_only: std::sync::atomic::AtomicBool,
}

/// Learn My Edits for a session. Cheap to clone; every clone is the same.
#[derive(Clone)]
pub struct LearnEdits {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the encoder is kept: a non-empty `LIGHTCRAFT_LEARN_DIR`; else on the Mac the folder shared with
/// PhotoCraft, `~/Library/Application Support/ArtCraft/models/dinov2-small`; else `<config>/models/dinov2-small`.
pub fn default_model_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("LIGHTCRAFT_LEARN_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    if let Some(shared) = crate::shared_models::shared_models_dir(std::env::var_os("HOME")) {
        return Some(shared.join(lightcraft_learn::ENCODER_ID));
    }
    crate::config::config_dir().map(|c| c.join("models").join(lightcraft_learn::ENCODER_ID))
}

/// Where profiles are kept: `<config>/learn-profiles`.
pub fn default_profiles_dir() -> Option<PathBuf> {
    crate::config::config_dir().map(|c| c.join("learn-profiles"))
}

/// A file name for `name`: lower-case letters, digits and dashes.
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 60 {
            break;
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() { "profile".into() } else { s }
}

fn stamp(path: &Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| (m.len(), m.modified().ok()))
}

impl LearnEdits {
    /// Learn My Edits with no folders (nothing installed, no profiles): the session's activity stack shows its rows.
    pub fn new(activity: Activity) -> Self {
        LearnEdits {
            inner: Arc::new(Inner {
                activity,
                model_dir: Mutex::new(None),
                profiles_dir: Mutex::new(None),
                #[cfg(not(target_arch = "wasm32"))]
                encoder: Mutex::new(None),
                download: Mutex::new(Download::Idle),
                read: Mutex::new(HashMap::new()),
                features: Mutex::new(HashMap::new()),
                job: Mutex::new(None),
                message: Mutex::new(None),
                statistics_only: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Use `models` for the encoder and `profiles` for the profile files.
    pub fn set_dirs(&self, models: Option<PathBuf>, profiles: Option<PathBuf>) {
        *lock(&self.inner.model_dir) = models;
        *lock(&self.inner.profiles_dir) = profiles;
        #[cfg(not(target_arch = "wasm32"))]
        {
            *lock(&self.inner.encoder) = None;
        }
        lock(&self.inner.read).clear();
    }

    /// Tests and headless checks: make features from the photo statistics only, so no model is needed.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_statistics_only(&self, on: bool) {
        self.inner.statistics_only.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn statistics_only(&self) -> bool {
        self.inner.statistics_only.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn model_dir(&self) -> Option<PathBuf> {
        lock(&self.inner.model_dir).clone()
    }

    pub fn profiles_dir(&self) -> Option<PathBuf> {
        lock(&self.inner.profiles_dir).clone()
    }

    /// The encoder's weights file, whether or not it is there.
    pub fn model_path(&self) -> Option<PathBuf> {
        self.model_dir().map(|d| d.join(lightcraft_learn::ENCODER_FILE))
    }

    /// Whether the encoder's file is in place with its published size (its hash was checked when it was downloaded).
    pub fn installed(&self) -> bool {
        self.model_path().is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == lightcraft_learn::ENCODER_BYTES))
    }

    pub fn download_state(&self) -> Download {
        lock(&self.inner.download).clone()
    }

    pub fn message(&self) -> Option<String> {
        lock(&self.inner.message).clone()
    }

    pub(crate) fn say(&self, text: impl Into<String>) {
        *lock(&self.inner.message) = Some(text.into());
    }

    /// The encoder, loaded once (on a job's thread).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn encoder(&self) -> Result<Option<Arc<lightcraft_learn::encoder::Encoder>>, String> {
        if self.statistics_only() {
            return Ok(None);
        }
        if let Some(e) = lock(&self.inner.encoder).clone() {
            return Ok(Some(e));
        }
        if !self.installed() {
            return Err(format!(
                "the image model Learn My Edits uses isn't installed: download it first ({} MB)",
                (lightcraft_learn::ENCODER_BYTES + 500_000) / 1_000_000
            ));
        }
        let path = self.model_path().ok_or("no model folder")?;
        let e = Arc::new(lightcraft_learn::encoder::Encoder::load(&path).map_err(|e| e.to_string())?);
        *lock(&self.inner.encoder) = Some(e.clone());
        Ok(Some(e))
    }

    /// The feature layout new profiles get.
    pub fn layout(&self) -> lightcraft_learn::Features {
        let with = !self.statistics_only();
        lightcraft_learn::Features { encoder: with.then(|| lightcraft_learn::ENCODER_ID.to_string()), layout: lightcraft_learn::layout(with) }
    }

    pub(crate) fn cached_features(&self, id: PhotoId, defaults_hash: u64) -> Option<Arc<Vec<f32>>> {
        lock(&self.inner.features).get(&(id, defaults_hash)).cloned()
    }

    pub(crate) fn keep_features(&self, id: PhotoId, defaults_hash: u64, f: Arc<Vec<f32>>) {
        let mut c = lock(&self.inner.features);
        if c.len() >= MAX_CACHED {
            c.clear();
        }
        c.insert((id, defaults_hash), f);
    }

    /// Every profile in the folder, by name.
    pub fn list(&self) -> Vec<Listed> {
        let Some(dir) = self.profiles_dir() else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut paths: Vec<PathBuf> =
            entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == lightcraft_learn::profile::EXTENSION)).take(MAX_PROFILES).collect();
        paths.sort();
        let mut out: Vec<Listed> = paths
            .into_iter()
            .filter_map(|path| {
                let id = path.file_stem()?.to_str()?.to_string();
                Some(Listed { profile: self.read_cached(&path), id, path })
            })
            .collect();
        out.sort_by_key(|l| l.profile.as_ref().map(|p| p.name.to_lowercase()).unwrap_or_default());
        out
    }

    fn read_cached(&self, path: &Path) -> Result<Arc<Profile>, String> {
        let Some(st) = stamp(path) else { return Err("the file is gone".into()) };
        if let Some((s, p)) = lock(&self.inner.read).get(path)
            && *s == st
        {
            return p.clone();
        }
        let p = Profile::read(path).map(Arc::new).map_err(|e| e.to_string());
        lock(&self.inner.read).insert(path.to_path_buf(), (st, p.clone()));
        p
    }

    /// The file of profile `id`.
    pub fn path_of(&self, id: &str) -> Result<PathBuf, String> {
        let dir = self.profiles_dir().ok_or("Learn My Edits has no profiles folder")?;
        if id.is_empty() || id.len() > 100 || !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("no profile `{id}`"));
        }
        Ok(dir.join(format!("{id}.{}", lightcraft_learn::profile::EXTENSION)))
    }

    /// Profile `id`, read from its file.
    pub fn get(&self, id: &str) -> Result<Arc<Profile>, String> {
        let path = self.path_of(id)?;
        if stamp(&path).is_none() {
            return Err(format!("no profile `{id}`"));
        }
        self.read_cached(&path)
    }

    /// Save profile `id`.
    pub fn save(&self, id: &str, p: &Profile) -> Result<(), String> {
        let path = self.path_of(id)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        p.write(&path).map_err(|e| e.to_string())?;
        lock(&self.inner.read).remove(&path);
        Ok(())
    }

    /// An unused id for a profile called `name`.
    pub fn new_id(&self, name: &str) -> Result<String, String> {
        let base = slug(name);
        for n in 1..1000 {
            let id = if n == 1 { base.clone() } else { format!("{base}-{n}") };
            if stamp(&self.path_of(&id)?).is_none() {
                return Ok(id);
            }
        }
        Err("too many profiles of that name".into())
    }

    /// Delete profile `id`.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.check_idle(id)?;
        let path = self.path_of(id)?;
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        lock(&self.inner.read).remove(&path);
        Ok(())
    }

    /// An error when a job is working on profile `id`.
    pub fn check_idle(&self, id: &str) -> Result<(), String> {
        match lock(&self.inner.job).as_ref() {
            Some(j) if j.profile == id => Err(format!("profile \"{}\" is busy: {}", j.name, j.label)),
            _ => Ok(()),
        }
    }
}

impl Session {
    /// Learn My Edits with its default folders ([`default_model_dir`], [`default_profiles_dir`]).
    pub fn with_default_learn(self) -> Self {
        self.learn.set_dirs(default_model_dir(), default_profiles_dir());
        self
    }

    /// A photo's own defaults, as Reset gives them (its import look, else its built-in defaults with its as-shot
    /// white balance).
    pub fn learn_defaults(&self, id: PhotoId) -> Option<DevelopSettings> {
        let photo = self.catalog.photo(id)?;
        if let Some(look) = photo.import_look.clone() {
            return Some(self.learn_effective(id, &look));
        }
        let info = self.source_info(id);
        let defaults = photo.camera_defaults();
        let mut fresh = DevelopSettings {
            wb: lightcraft_develop::WhiteBalance { mode: WbMode::AsShot, temp: info.as_shot_temp, tint: info.as_shot_tint, ..Default::default() },
            ..defaults
        };
        lightcraft_pipeline::local::normalize_wb(&mut fresh, &info);
        Some(fresh)
    }

    /// `settings` with the white balance it renders with: an as-shot white balance gets the photo's as-shot values.
    pub fn learn_effective(&self, id: PhotoId, settings: &DevelopSettings) -> DevelopSettings {
        let info = self.source_info(id);
        let mut d = settings.clone();
        lightcraft_pipeline::local::normalize_wb(&mut d, &info);
        if d.wb.mode == WbMode::AsShot {
            d.wb.temp = info.as_shot_temp;
            d.wb.tint = info.as_shot_tint;
        }
        d
    }

    /// Photo `id`'s settings with `profile`'s sliders for features `f` written in (only the learned groups change).
    pub fn learn_apply_to(&self, id: PhotoId, profile: &Profile, f: &[f32]) -> Result<DevelopSettings, String> {
        let current = self.develop_of(id).ok_or("no such photo")?;
        let defaults = self.learn_defaults(id).ok_or("no such photo")?;
        let writes_wb = profile.learned_groups().contains(&lightcraft_learn::Group::WhiteBalance);
        let base = if writes_wb { self.learn_effective(id, &current) } else { (*current).clone() };
        let mut out = lightcraft_learn::apply(profile, f, &base, &defaults).map_err(|e| e.to_string())?;
        if writes_wb {
            out.wb.mode = WbMode::Custom;
        }
        Ok(out)
    }
}
