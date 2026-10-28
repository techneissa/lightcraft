//! Auto AI Denoise: a high-ISO raw photo gets AI Denoise switched on by itself when it is opened.
//!
//! The switch is an ordinary edit (the Detail panel's AI Denoise switch, as one undo step named "Auto AI Denoise"), so it
//! stays on with the photo, shows in History and survives a restart. The picture behind it is made by the usual background
//! queue (`denoise.pump`), which shows the activity row.
//!
//! **A photo is handled once.** An undo, a Reset or switching the photo's AI Denoise off would otherwise leave the photo
//! looking untouched and open to the next open. So the photos that got the edit are recorded in `auto-denoise.json` beside
//! the library's other settings files: their ids, which an undo does not touch. A photo in that list is never changed
//! again, and neither is a photo whose AI Denoise switch or Amount the user already set.
//!
//! Nothing happens without a model, a library folder on disk, a raw photo with a known ISO at or above the threshold, and
//! the setting (`denoise.settings` → `autoOpen`, off unless the user turns it on). Such a photo is not recorded, so it is
//! handled the first time it is opened once the model is installed.

use super::*;
use crate::{EngineError, Result};

/// The library file holding the handled photos.
const FILE: &str = "auto-denoise.json";

/// Most bytes of that file that are read (a hostile or damaged file must not fill memory).
const MAX_FILE: usize = 8 << 20;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Handled {
    photos: Vec<u64>,
}

/// Why `denoise.autoOpen` did or did not switch AI Denoise on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Applied,
    /// The setting is off.
    Off,
    /// Not a raw photo AI Denoise can develop (or no such photo).
    NotApplicable,
    /// The file has no ISO.
    NoIso,
    BelowThreshold,
    /// The photo's AI Denoise switch or Amount was set already (by the user, a preset or a copy).
    AlreadySet,
    /// The photo was handled before (the user may have undone or switched off the edit).
    AlreadyDone,
    NoModel,
    /// The pictures are kept in the library's folder, and there is none.
    NoLibraryFolder,
}

impl Reason {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Reason::Applied => "applied",
            Reason::Off => "off",
            Reason::NotApplicable => "notApplicable",
            Reason::NoIso => "noIso",
            Reason::BelowThreshold => "belowThreshold",
            Reason::AlreadySet => "alreadySet",
            Reason::AlreadyDone => "alreadyDone",
            Reason::NoModel => "noModel",
            Reason::NoLibraryFolder => "noLibraryFolder",
        }
    }
}

impl Session {
    /// The photos handled so far, read from the library on first use.
    fn denoise_auto_handled(&mut self) -> &mut HashSet<PhotoId> {
        if self.denoise.auto_done.is_none() {
            let read = self.library.as_mut().and_then(|l| l.read_file(FILE)).filter(|b| b.len() <= MAX_FILE);
            let set = read
                .and_then(|b| serde_json::from_slice::<Handled>(&b).ok())
                .map(|h| h.photos.into_iter().map(PhotoId).collect())
                .unwrap_or_default();
            self.denoise.auto_done = Some(set);
        }
        self.denoise.auto_done.get_or_insert_with(HashSet::new)
    }

    /// Record that photo `id` was handled. A write that fails (a full disk) leaves it recorded for this session only.
    fn denoise_auto_record(&mut self, id: PhotoId) {
        let handled = self.denoise_auto_handled();
        handled.insert(id);
        let mut photos: Vec<u64> = handled.iter().map(|p| p.0).collect();
        photos.sort_unstable();
        let Ok(bytes) = serde_json::to_vec(&Handled { photos }) else { return };
        if let Some(Err(e)) = self.library.as_mut().map(|l| l.write_file(FILE, &bytes)) {
            log::warn!("auto AI Denoise: could not write {FILE}: {e}");
        }
    }

    /// Photo `id` was opened. See the module docs; `reason` says what was decided.
    pub(crate) fn denoise_auto_open_photo(&mut self, id: PhotoId) -> Result<Reason> {
        self.denoise_refresh_active(false);
        let settings = self.denoise.settings.clone();
        if !settings.auto_open() {
            return Ok(Reason::Off);
        }
        let Some(p) = self.catalog.photo(id) else { return Ok(Reason::NotApplicable) };
        if !eligible(p) {
            return Ok(Reason::NotApplicable);
        }
        let Some(iso) = p.meta.iso.filter(|iso| *iso > 0) else { return Ok(Reason::NoIso) };
        if iso < settings.auto_iso() {
            return Ok(Reason::BelowThreshold);
        }
        let switched = p.develop.enhance.denoise_on.is_some() || p.develop.enhance.denoise > 0.0;
        if self.denoise_auto_handled().contains(&id) {
            return Ok(Reason::AlreadyDone);
        }
        if switched {
            // the user's own choice counts as handled: a later Reset must not bring the automatic edit back
            self.denoise_auto_record(id);
            return Ok(Reason::AlreadySet);
        }
        if self.denoise.active.is_none() {
            return Ok(Reason::NoModel);
        }
        if self.denoise_products_dir().is_none() {
            return Ok(Reason::NoLibraryFolder);
        }
        crate::cmd::denoise::switch(self, &json!({"id": id.0, "enabled": true}), "Auto AI Denoise")?;
        self.denoise_auto_record(id);
        Ok(Reason::Applied)
    }

    /// `denoise.autoOpen`.
    pub(crate) fn denoise_auto_open(&mut self, id: PhotoId) -> Result<Value> {
        if self.catalog.photo(id).is_none() {
            return Err(EngineError::Other(format!("denoise.autoOpen: no photo {}", id.0)));
        }
        let reason = self.denoise_auto_open_photo(id)?;
        let mut v = json!({"id": id.0, "applied": reason == Reason::Applied, "reason": reason.name()});
        if let (Some(o), Some(p)) = (v.as_object_mut(), self.catalog.photo(id)) {
            o.insert("threshold".into(), json!(self.denoise.settings.auto_iso()));
            if let Some(iso) = p.meta.iso {
                o.insert("iso".into(), json!(iso));
            }
            if reason == Reason::Applied {
                o.insert("amount".into(), json!(p.develop.enhance.denoise));
            }
        }
        Ok(v)
    }
}
