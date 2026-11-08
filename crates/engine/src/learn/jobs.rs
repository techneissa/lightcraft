//! Learn My Edits' background jobs, one at a time: gather examples and train a profile, or work out a profile's
//! sliders for photos. Each is a thread with a row in the activity stack ("Learning “Portraits”", ✕ stops it).
//!
//! The session's thread prepares each photo ([`prepare`]: its defaults, its edit as targets, a render job of its
//! defaults when its features aren't known yet); the thread renders, runs the encoder and trains. A training job
//! saves the profile itself. An apply job's features come back to the session's thread ([`pump`]), which writes the
//! sliders as one undo step with one History step per photo, named after the profile.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_learn::{Example, Profile};

use super::{LearnEdits, lock};
use crate::Session;
use crate::activity::Cancel;
use crate::media::RenderJob;

/// One photo, ready for the job's thread.
pub struct Item {
    pub id: PhotoId,
    /// The example's name in the profile.
    pub key: String,
    defaults_hash: u64,
    cached: Option<Arc<Vec<f32>>>,
    render: Option<RenderJob>,
    /// The photo's edit (for an example).
    targets: Option<Vec<f32>>,
}

/// What a job ended with.
pub enum Outcome {
    /// A profile was trained and saved: `(profile id, what to tell the user)`.
    Trained(String, String),
    /// Features for an apply.
    Applied {
        profile: Arc<Profile>,
        items: Vec<(PhotoId, Arc<Vec<f32>>)>,
        failed: Vec<String>,
    },
    Failed(String),
    Cancelled,
}

/// The running job.
pub struct Running {
    pub profile: String,
    pub name: String,
    pub label: String,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<Outcome>>,
}

/// The label of an edit made by `profile` (History and Undo).
pub fn edit_label(profile: &Profile) -> String {
    format!("Learned: {}", profile.name)
}

/// Prepare photo `id`: its defaults, and its edit when `example` is set. `None` with a reason for a photo that
/// can't be used (a video, a missing photo, nothing edited in the profile's groups).
pub fn prepare(s: &mut Session, id: PhotoId, example: Option<&Profile>) -> Result<Item, String> {
    let photo = s.catalog.photo(id).ok_or_else(|| format!("no photo {}", id.0))?.clone();
    let defaults = s.learn_defaults(id).ok_or_else(|| format!("no photo {}", id.0))?;
    let hash = defaults.hash64();
    let targets = match example {
        Some(profile) => {
            let current = s.develop_of(id).ok_or_else(|| format!("no photo {}", id.0))?;
            let t = lightcraft_learn::sliders::targets(&s.learn_effective(id, &current), &defaults);
            let edited = lightcraft_learn::SLIDERS.iter().zip(&t).any(|(sl, v)| profile.groups.contains(&sl.group) && v.abs() > 1e-4);
            if !edited {
                return Err(format!("{} has no edits in what the profile learns", photo.file_name));
            }
            Some(t)
        }
        None => None,
    };
    let cached = s.learn.cached_features(id, hash);
    let render = match cached {
        Some(_) => None,
        None => {
            let edge = lightcraft_learn::PREVIEW_EDGE as usize;
            Some(s.preview_job(id, edge, edge, false, &defaults).ok_or_else(|| format!("{} can't be rendered", photo.file_name))?)
        }
    };
    Ok(Item { id, key: format!("{} {}", id.0, photo.file_name), defaults_hash: hash, cached, render, targets })
}

impl LearnEdits {
    /// The running job's profile id, name and label, if one runs.
    pub fn running(&self) -> Option<(String, String, String)> {
        lock(&self.inner.job).as_ref().map(|j| (j.profile.clone(), j.name.clone(), j.label.clone()))
    }

    /// Stop the running job (its thread notices between photos).
    pub fn cancel(&self) -> bool {
        lock(&self.inner.job).as_ref().map(|j| j.cancel.store(true, Ordering::Relaxed)).is_some()
    }

    fn start(
        &self,
        profile: &str,
        name: &str,
        label: String,
        work: impl FnOnce(&LearnEdits, &crate::activity::TaskGuard, &AtomicBool) -> Outcome + Send + 'static,
    ) -> Result<(), String> {
        let mut job = lock(&self.inner.job);
        if let Some(j) = job.as_ref() {
            return Err(format!("Learn My Edits is busy: {}", j.label));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let row = self.inner.activity.start("learn", &label, Cancel::Flag(cancel.clone()));
        let (this, flag) = (self.clone(), cancel.clone());
        let handle = std::thread::Builder::new()
            .name("learn-edits".into())
            .spawn(move || {
                let row = row;
                while this.held() && !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                crate::guard::catch("Learn My Edits", || work(&this, &row, &flag)).unwrap_or_else(Outcome::Failed)
            })
            .map_err(|e| format!("could not start: {e}"))?;
        *job = Some(Running { profile: profile.to_string(), name: name.to_string(), label, cancel, handle: Some(handle) });
        Ok(())
    }

    /// The features of each item, in order (`Err` per photo that failed). Stops with `None` when cancelled.
    fn features(
        &self,
        items: Vec<Item>,
        row: &crate::activity::TaskGuard,
        cancel: &AtomicBool,
        total: u64,
    ) -> Result<Option<Vec<(Item, Result<Arc<Vec<f32>>, String>)>>, String> {
        #[cfg(not(target_arch = "wasm32"))]
        let encoder = if items.iter().all(|i| i.cached.is_some()) { None } else { self.encoder()? };
        let mut out = Vec::with_capacity(items.len());
        for (n, mut item) in items.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            row.progress(n as u64, total);
            let f = match (item.cached.clone(), item.render.take()) {
                (Some(f), _) => Ok(f),
                (None, Some(job)) => match job.run().rendered {
                    Ok(r) => {
                        let rgb: Vec<u8> = r.image.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
                        #[cfg(not(target_arch = "wasm32"))]
                        let f = lightcraft_learn::photo_features(encoder.as_deref(), r.image.width, r.image.height, &rgb).map_err(|e| e.to_string());
                        #[cfg(target_arch = "wasm32")]
                        let f: Result<Vec<f32>, String> = Err("the web build has no image model".into());
                        f.map(|f| {
                            let f = Arc::new(f);
                            self.keep_features(item.id, item.defaults_hash, f.clone());
                            f
                        })
                    }
                    Err(e) => Err(e),
                },
                (None, None) => Err("nothing to render".into()),
            };
            out.push((item, f));
        }
        Ok(Some(out))
    }

    /// Add `items` to profile `id` as examples (none: train on what it has), train it and save it.
    pub fn start_training(&self, id: &str, mut profile: Profile, items: Vec<Item>) -> Result<(), String> {
        let label = format!("Learning “{}”", profile.name);
        let pid = id.to_string();
        let name = profile.name.clone();
        self.start(id, &name, label, move |this, row, cancel| {
            let total = items.len() as u64 + 1;
            let gathered = match this.features(items, row, cancel, total) {
                Ok(Some(g)) => g,
                Ok(None) => return Outcome::Cancelled,
                Err(e) => return Outcome::Failed(e),
            };
            let mut failed = Vec::new();
            for (item, f) in gathered {
                let (Ok(features), Some(targets)) = (f.map_err(|e| failed.push(format!("{}: {e}", item.key))), item.targets) else { continue };
                if let Err(e) = profile.add_example(Example { photo: item.key.clone(), features: (*features).clone(), targets }) {
                    failed.push(format!("{}: {e}", item.key));
                }
            }
            row.progress(total - 1, total);
            row.detail("Training");
            let result = profile.train(&|| cancel.load(Ordering::Relaxed), &mut |_, _| {}).cloned();
            if matches!(result, Err(lightcraft_learn::Error::Cancelled)) {
                return Outcome::Cancelled;
            }
            if let Err(e) = this.save(&pid, &profile) {
                return Outcome::Failed(e);
            }
            let skipped = if failed.is_empty() {
                String::new()
            } else {
                format!(" ({} photos skipped: {})", failed.len(), failed.first().cloned().unwrap_or_default())
            };
            match result {
                Ok(r) => Outcome::Trained(
                    pid,
                    format!(
                        "“{}” learned from {} photos: closer to your edits than the defaults on {} of them{skipped}",
                        profile.name, r.examples, r.beats_defaults
                    ),
                ),
                Err(e) => Outcome::Failed(format!("“{}”: {e}{skipped}", profile.name)),
            }
        })
    }

    /// Work out `profile`'s sliders for `items`; [`pump`] writes them.
    pub fn start_apply(&self, id: &str, profile: Arc<Profile>, items: Vec<Item>) -> Result<(), String> {
        let label = format!("Applying “{}”", profile.name);
        let name = profile.name.clone();
        self.start(id, &name, label, move |this, row, cancel| {
            let total = items.len() as u64;
            let gathered = match this.features(items, row, cancel, total) {
                Ok(Some(g)) => g,
                Ok(None) => return Outcome::Cancelled,
                Err(e) => return Outcome::Failed(e),
            };
            let mut failed = Vec::new();
            let mut done = Vec::with_capacity(gathered.len());
            for (item, f) in gathered {
                match f {
                    Ok(f) => done.push((item.id, f)),
                    Err(e) => failed.push(format!("{}: {e}", item.key)),
                }
            }
            Outcome::Applied { profile, items: done, failed }
        })
    }

    /// The running job's outcome when it has ended (or at once, waiting for it, with `wait`).
    fn take_finished(&self, wait: bool) -> Option<Outcome> {
        let handle = {
            let mut job = lock(&self.inner.job);
            let done = job.as_ref().and_then(|j| j.handle.as_ref()).is_some_and(|h| wait || h.is_finished());
            if !done {
                return None;
            }
            job.take().and_then(|mut j| j.handle.take())
        };
        Some(handle?.join().unwrap_or_else(|_| Outcome::Failed("Learn My Edits stopped unexpectedly".into())))
    }
}

/// What [`pump`] did.
#[derive(Default)]
pub struct Pumped {
    pub applied: usize,
    pub trained: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
}

/// Take in a job that has ended (waiting for the running one with `wait`): write an apply's sliders (one undo
/// step; a History step per photo named after the profile) and say how it went.
pub fn pump(s: &mut Session, wait: bool) -> Pumped {
    let mut out = Pumped::default();
    let Some(outcome) = s.learn.take_finished(wait) else { return out };
    match outcome {
        Outcome::Trained(id, msg) => {
            out.trained = Some(id);
            out.message = Some(msg);
        }
        Outcome::Cancelled => out.message = Some("Learn My Edits was stopped".into()),
        Outcome::Failed(e) => out.error = Some(e),
        Outcome::Applied { profile, items, mut failed } => {
            let label = edit_label(&profile);
            let mut ops = Vec::with_capacity(items.len());
            for (id, f) in items {
                match s.learn_apply_to(id, &profile, &f) {
                    Ok(settings) => ops.extend(s.develop_op(id, settings, &label)),
                    Err(e) => failed.push(format!("{}: {e}", id.0)),
                }
            }
            out.applied = ops.len();
            if !ops.is_empty()
                && let Err(e) = s.commit(&label, Op::Batch { ops })
            {
                out.applied = 0;
                failed.push(e.to_string());
            }
            let mut msg = format!("“{}” applied to {} photo{}", profile.name, out.applied, if out.applied == 1 { "" } else { "s" });
            if let Some(first) = failed.first() {
                msg.push_str(&format!("; {} failed ({first})", failed.len()));
            }
            if out.applied == 0 && !failed.is_empty() { out.error = Some(msg) } else { out.message = Some(msg) }
        }
    }
    if let Some(m) = out.error.clone().or_else(|| out.message.clone()) {
        s.learn.say(m);
    }
    out
}
