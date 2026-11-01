//! Auto AI Denoise: a high-ISO raw photo gets AI Denoise switched on by itself when it is opened.
//!
//! The switch is an ordinary edit (the Detail panel's AI Denoise switch, as one undo step named "Auto AI Denoise"), so it
//! stays on with the photo, shows in History and survives a restart. The picture behind it is made by the usual background
//! queue (`denoise.pump`), which shows the activity row.
//!
//! **A photo is handled once.** An undo, a Reset or switching the photo's AI Denoise off would otherwise leave the photo
//! looking untouched and open to the next open. So the photos that got the edit are recorded in `auto-denoise.json` beside
//! the library's other settings files: their ids, which an undo does not touch. A photo in that list is never changed
//! again, and neither is a photo whose AI Denoise switch or Amount the user already set. The list is kept in memory and
//! written a couple of seconds after a change and on close (never inside the command, so quick browsing does not sync a
//! file per photo), only after the catalog edits are saved. A file that is too large or damaged is never overwritten and
//! turns the feature off for the session (`recordUnreadable`): the list cannot be rebuilt, and guessing would switch
//! AI Denoise on again for photos the user undid.
//!
//! Nothing happens without a model, a library folder on disk, a raw photo with a known ISO at or above the threshold, and
//! the setting (`denoise.settings` → `autoOpen`, off unless the user turns it on). Such a photo is not recorded, so it is
//! handled the first time it is opened once the model is installed.

use super::{Deserialize, Duration, HashSet, PhotoId, Serialize, Session, Value, eligible, json};
use crate::{EngineError, Result};

/// The library file holding the handled photos.
const FILE: &str = "auto-denoise.json";

/// Most bytes of that file that are read (a hostile or damaged file must not fill memory).
const MAX_FILE: usize = 8 << 20;

/// How long a newly recorded photo waits in memory before the file is written (browsing quickly through high-ISO
/// photos must not write and sync a file for each one).
pub(super) const FLUSH_AFTER: Duration = Duration::from_secs(2);

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Handled {
    photos: Vec<u64>,
}

/// The handled photos in memory: read from the library on first use, written back some seconds after a change and
/// when the library is saved on close.
#[derive(Default)]
pub(crate) struct Record {
    photos: HashSet<PhotoId>,
    /// Since when the set has changes the file lacks.
    dirty_since: Option<web_time::Instant>,
    /// Why the file could not be read (too large, damaged, unreadable). It is then never written this session, and
    /// nothing is switched on by itself: without the list, a photo the user undid could not be told from a new one.
    unreadable: Option<String>,
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
    /// `auto-denoise.json` is damaged or too large: it is left as it is and nothing is switched on this session.
    RecordUnreadable,
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
            Reason::RecordUnreadable => "recordUnreadable",
            Reason::AlreadySet => "alreadySet",
            Reason::AlreadyDone => "alreadyDone",
            Reason::NoModel => "noModel",
            Reason::NoLibraryFolder => "noLibraryFolder",
        }
    }
}

impl Session {
    /// The record of handled photos, read from the library on first use (at most [`MAX_FILE`] bytes of it).
    fn denoise_auto_handled(&mut self) -> &mut Record {
        if self.denoise.auto_done.is_none() {
            let read = self.library.as_mut().map_or(Ok(None), |l| l.read_file(FILE, MAX_FILE));
            let mut record = Record::default();
            match read {
                Ok(None) => {}
                Ok(Some(bytes)) => match serde_json::from_slice::<Handled>(&bytes) {
                    Ok(h) => record.photos = h.photos.into_iter().map(PhotoId).collect(),
                    Err(e) => record.unreadable = Some(format!("damaged: {e}")),
                },
                Err(e) => record.unreadable = Some(e.to_string()),
            }
            if let Some(why) = &record.unreadable {
                log::warn!("auto AI Denoise is off for this session: {FILE} in the library is not usable ({why}); the file is left as it is");
            }
            self.denoise.auto_done = Some(record);
        }
        self.denoise.auto_done.get_or_insert_with(Record::default)
    }

    /// Record that photo `id` was handled; the file is written by [`Self::denoise_auto_flush`].
    fn denoise_auto_record(&mut self, id: PhotoId) {
        let record = self.denoise_auto_handled();
        if record.unreadable.is_none() && record.photos.insert(id) {
            record.dirty_since.get_or_insert_with(web_time::Instant::now);
        }
    }

    /// Does the record have changes the file lacks?
    pub(crate) fn denoise_auto_pending(&self) -> bool {
        self.denoise.auto_done.as_ref().is_some_and(|r| r.dirty_since.is_some())
    }

    /// Write the record when it has waited [`FLUSH_AFTER`] (or `force`: the library is closing or being left). The library
    /// calls it after the catalog edits are saved, so a crash cannot leave a photo recorded without its edit. A failed
    /// write (a full disk) is logged and tried again later.
    pub(crate) fn denoise_auto_flush(&mut self, force: bool) {
        let after = self.denoise.auto_flush_after;
        let Some(record) = self.denoise.auto_done.as_mut() else { return };
        let Some(since) = record.dirty_since else { return };
        if record.unreadable.is_some() || !(force || since.elapsed() >= after) {
            return;
        }
        let mut photos: Vec<u64> = record.photos.iter().map(|p| p.0).collect();
        photos.sort_unstable();
        let Ok(bytes) = serde_json::to_vec(&Handled { photos }) else { return };
        match self.library.as_mut().map(|l| l.write_file(FILE, &bytes)) {
            Some(Err(e)) => {
                log::warn!("auto AI Denoise: could not write {FILE}: {e}");
                record.dirty_since = Some(web_time::Instant::now());
            }
            _ => record.dirty_since = None,
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
        let record = self.denoise_auto_handled();
        if record.unreadable.is_some() {
            return Ok(Reason::RecordUnreadable);
        }
        if record.photos.contains(&id) {
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
