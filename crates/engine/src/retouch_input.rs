//! Fork feature (`fork/retouch-foundation.md`): the faces portrait retouching works on, per photo.
//!
//! A [`FaceProvider`] finds the faces (and, once a people parser is installed, their part masks) on a render of the
//! photo before its crop, without retouching. The result is kept per photo in the session's [`FaceStore`] (in
//! memory, one entry per photo, the photos used last within [`KEEP_BYTES`]) and handed to every render of the photo
//! in `SourceInfo::retouch`; its fingerprint (hashed on the search's worker) is part of the render keys
//! ([`Session::retouch_for`]), so a preview made before the faces were known is redrawn once they are.
//!
//! - Renders never wait: a photo whose retouching is on and whose faces are unknown starts a search on a worker
//!   thread, with a row in the activity stack, and renders without faces meanwhile (every feature then changes
//!   nothing). One search runs per photo; a render asking meanwhile is redrawn when it ends, and asks again. Exports
//!   and synchronous renders find them first ([`Session::retouch_detect_now`]).
//! - While the chosen people-parts set is downloaded, the faces come from the photo's people parse, with every part
//!   mask and the face mesh (`parts`, `fork/retouch-masks.md`). Else the provider is YuNet (`crates/faces`,
//!   installed in Settings ▸ Faces): boxes and five landmarks, no masks. A provider installed with
//!   [`FaceStore::set_provider`] (tests) comes before both.
//! - A face's person is the name of the photo's named face region it overlaps most (People). Its box is also kept
//!   as it is on the photo before the user's rotation (`Face::upright`), which per-face settings are matched by. Faces are listed left
//!   to right.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::{Photo, PhotoId};
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_raster::Rgba8;

use crate::{EngineError, Session};

mod parts;

/// Finds faces (and their part masks) on a picture.
pub trait FaceProvider: Send + Sync {
    /// Its name, for the activity row and logs.
    fn name(&self) -> &str;
    /// The faces of `img` (8-bit sRGB: the photo before its crop, without retouching), in fractions of `img`'s
    /// width and height ([`lightcraft_develop::retouch::input`]).
    fn faces(&self, img: &Rgba8) -> Result<Vec<Face>, String>;
}

/// YuNet (`lightcraft_faces::yunet`): boxes and five landmarks, no masks.
pub struct Yunet(pub Arc<lightcraft_faces::yunet::Detector>);

impl FaceProvider for Yunet {
    fn name(&self) -> &str {
        "YuNet"
    }

    fn faces(&self, img: &Rgba8) -> Result<Vec<Face>, String> {
        let rgb: Vec<u8> = img.data.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        let found = self.0.detect(&rgb, img.width, img.height, &lightcraft_faces::yunet::Options::default()).map_err(|e| e.to_string())?;
        Ok(found
            .iter()
            .map(|f| Face {
                bounds: NormRect::new(f64::from(f.x0), f64::from(f.y0), f64::from(f.x1), f64::from(f.y1)),
                upright: None,
                landmarks: f.landmarks.iter().map(|(x, y)| (f64::from(*x), f64::from(*y))).collect(),
                score: f.score,
                person: None,
                masks: Vec::new(),
                key_frame: lightcraft_develop::retouch::KeyFrame::Edit,
                spots: None,
                strands: None,
            })
            .collect())
    }
}

/// Long edge of the render faces are found on.
pub const DETECT_EDGE: usize = 1600;
/// Photos whose failures and missing-model reasons are kept.
pub const KEEP: usize = 64;
/// What the faces kept in memory may take together (masks, landmarks, thumbnails); the photo used least recently
/// goes first, the newest always stays.
pub const KEEP_BYTES: usize = 256 << 20;
/// Side of the face thumbnails kept for the Portrait panel's face picker.
pub const THUMB: usize = 96;
/// The render-key salt of a retouched photo whose faces are not known yet (a fingerprint is always odd).
const UNKNOWN: u64 = 0x5245_544f_5543_4800;

/// One photo's faces as found.
#[derive(Clone)]
pub struct Found {
    pub input: Arc<Input>,
    /// A square close-up of each face (for the face picker), in the order of `input.faces`.
    pub thumbs: Arc<Vec<Rgba8>>,
}

impl Found {
    /// About what it takes in memory.
    pub fn bytes(&self) -> usize {
        let face = |f: &Face| {
            let masks: usize = f.masks.iter().map(|(_, m)| m.plane.data.len().saturating_mul(4)).sum();
            let spots = f.spots.as_ref().map_or(0, |s| s.list.len().saturating_mul(std::mem::size_of::<lightcraft_develop::retouch::input::Spot>()));
            let strands = f.strands.as_ref().map_or(0, |s| s.cells.len().saturating_mul(3).saturating_add(s.tiles.len().saturating_mul(4)));
            masks
                .saturating_add(f.landmarks.len().saturating_mul(16))
                .saturating_add(spots)
                .saturating_add(strands)
                .saturating_add(std::mem::size_of::<Face>())
        };
        let faces: usize = self.input.faces.iter().map(face).sum();
        let thumbs: usize = self.thumbs.iter().map(|t| t.data.len().saturating_mul(4)).sum();
        faces.saturating_add(thumbs)
    }
}

#[derive(Default)]
struct Store {
    /// (key, photo, faces), used least recently first; one entry per photo.
    found: Vec<(String, PhotoId, Found)>,
    /// Keys being searched now, with their photo (one background search per photo).
    busy: Vec<(String, PhotoId)>,
    /// Per photo, how many of its searches have ended: mixed into the salt of a photo whose faces are unknown, so
    /// a render made while another search of the photo ran is redrawn (and asks again) once it ends.
    ended: std::collections::HashMap<PhotoId, u64>,
    /// Background searches started (tests count them).
    started: usize,
    /// The last search's failure, per key.
    failed: Vec<(String, String)>,
    /// Why no search could start for a key (no face model, or one that can't be read): kept so a panel asking every
    /// frame never reads the model again; forgotten when a model is installed or removed ([`FaceStore::forget_no_model`]).
    no_model: Vec<(String, String)>,
    /// Keys whose faces a prepared export finds when it runs ([`ExportFaces`]): setting up other renders of them
    /// does not wait.
    deferred: Vec<String>,
}

/// The faces found per photo, and the provider that finds them.
#[derive(Clone, Default)]
pub struct FaceStore {
    store: Arc<Mutex<Store>>,
    provider: Option<Arc<dyn FaceProvider>>,
}

impl FaceStore {
    /// Find faces with `provider` from now on (instead of YuNet), and forget the faces found so far.
    pub fn set_provider(&mut self, provider: Arc<dyn FaceProvider>) {
        self.provider = Some(provider);
        let mut st = self.lock();
        st.found.clear();
        st.failed.clear();
        st.no_model.clear();
    }

    /// Forget why photos found no face model (installing or removing a model does): their next search runs.
    pub(crate) fn forget_no_model(&self) {
        self.lock().no_model.clear();
    }

    fn no_model_for(&self, key: &str) -> Option<String> {
        self.lock().no_model.iter().find(|(k, _)| k == key).map(|(_, why)| why.clone())
    }

    fn keep_no_model(&self, key: &str, why: &str) {
        let mut st = self.lock();
        st.no_model.retain(|(k, _)| k != key);
        st.no_model.push((key.to_string(), why.to_string()));
        let extra = st.no_model.len().saturating_sub(KEEP);
        st.no_model.drain(..extra);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The faces kept for `key`; a hit becomes the newest entry, so the photo used least recently goes first.
    fn get(&self, key: &str) -> Option<Found> {
        let mut st = self.lock();
        let i = st.found.iter().position(|(k, _, _)| k == key)?;
        let hit = st.found.remove(i);
        let found = hit.2.clone();
        st.found.push(hit);
        Some(found)
    }

    /// The salt of photo `id`'s renders while its faces are unknown: [`UNKNOWN`], changed each time one of its
    /// searches ends (even, as [`UNKNOWN`] is, so it is never a fingerprint's; never 0).
    fn unknown_salt(&self, id: PhotoId) -> u64 {
        let ended = self.lock().ended.get(&id).copied().unwrap_or(0);
        UNKNOWN ^ (ended.wrapping_mul(0x9e37_79b9_7f4a_7c15) & !1 & !UNKNOWN)
    }

    /// Keep the end of a search of photo `id` under `key`: its faces replace every other entry of the photo.
    fn put(&self, key: &str, id: PhotoId, result: Result<Found, String>) {
        // (hashed here, on the search's worker, not by the first render that asks for the key)
        if let Ok(found) = &result {
            found.input.fingerprint();
        }
        let mut st = self.lock();
        st.busy.retain(|(k, _)| k != key);
        st.found.retain(|(k, p, _)| k != key && *p != id);
        st.failed.retain(|(k, _)| k != key);
        if st.ended.len() > 4 * KEEP && !st.ended.contains_key(&id) {
            st.ended.clear();
        }
        let n = st.ended.entry(id).or_default();
        *n = n.wrapping_add(1);
        match result {
            Ok(found) => {
                st.found.push((key.to_string(), id, found));
                trim(&mut st.found, KEEP_BYTES);
            }
            Err(why) => {
                st.failed.push((key.to_string(), why));
                let extra = st.failed.len().saturating_sub(KEEP);
                st.failed.drain(..extra);
            }
        }
    }

    /// Mark `key` (of photo `id`) as being searched; false when it already is, or another search of the photo is.
    fn claim(&self, key: &str, id: PhotoId) -> bool {
        let mut st = self.lock();
        if st.busy.iter().any(|(k, p)| k == key || *p == id) {
            return false;
        }
        st.busy.push((key.to_string(), id));
        true
    }

    fn release(&self, key: &str) {
        self.lock().busy.retain(|(k, _)| k != key);
    }

    /// Background searches started so far.
    #[cfg(test)]
    pub(crate) fn started(&self) -> usize {
        self.lock().started
    }

    /// The photos whose faces are kept, least recently used first.
    #[cfg(test)]
    pub(crate) fn kept(&self) -> Vec<(PhotoId, usize)> {
        self.lock().found.iter().map(|(_, id, f)| (*id, f.bytes())).collect()
    }
}

/// Drop the entries used least recently until the rest take at most `budget` bytes; the newest always stays.
fn trim(found: &mut Vec<(String, PhotoId, Found)>, budget: usize) {
    let mut total: usize = found.iter().map(|(_, _, f)| f.bytes()).fold(0, usize::saturating_add);
    while found.len() > 1 && total > budget {
        let gone = found.remove(0);
        total = total.saturating_sub(gone.2.bytes());
    }
}

/// Where a photo's faces stand, for the Portrait panel and `retouch.faces`.
#[derive(Clone, Default)]
pub struct FacesState {
    pub found: Option<Found>,
    /// A search is running.
    pub busy: bool,
    /// Why the last search failed.
    pub error: Option<String>,
}

/// The key of a photo's faces found by a provider: its content, and every setting that moves pixels before the crop
/// (the faces are in the photo's coordinates before the crop, after orientation, lens corrections and transform).
pub fn key_of(p: &Photo, s: &DevelopSettings) -> String {
    let o = &s.optics;
    format!(
        "{}|{:?}|{:?}|{}|{}|{}|{}",
        crate::media::content_key(p),
        s.orientation,
        s.geometry,
        o.lens_profile,
        o.profile_distortion,
        o.distortion,
        s.crop.geometry.angle
    )
}

/// What finds a photo's faces now: a provider (one installed, else YuNet) on the detect render, or the people parse.
enum Source {
    Provider(Arc<dyn FaceProvider>),
    Parts,
}

/// A face search set up on the calling thread, to run on a worker (or an export's): it renders and runs models.
enum Finder {
    Faces { provider: Arc<dyn FaceProvider>, job: crate::RenderJob, regions: Vec<(NormRect, String)> },
    Parts(parts::PartsFinder),
}

impl Finder {
    fn name(&self) -> String {
        match self {
            Finder::Faces { provider, .. } => provider.name().to_string(),
            Finder::Parts(_) => "People Parts".into(),
        }
    }

    fn run(self) -> std::result::Result<Found, String> {
        match self {
            Finder::Faces { provider, job, regions } => find(job, provider.as_ref(), &regions),
            Finder::Parts(p) => p.find(),
        }
    }
}

/// `key` (a render's cache key) with a retouched photo's faces mixed in (`salt` 0: unchanged).
pub fn salted(key: lightcraft_preview::Hash128, salt: u64) -> lightcraft_preview::Hash128 {
    if salt == 0 { key } else { lightcraft_preview::Hasher128::new().u64(key.0 as u64).u64((key.0 >> 64) as u64).u64(salt).finish() }
}

impl Session {
    /// What a render of `p` with settings `s` needs from retouching: the salt for its cache keys (0 while
    /// retouching is off: every other render keeps its key) and the faces for its `SourceInfo`. Starts a search
    /// when retouching is on and the faces are not known.
    pub(crate) fn retouch_for(&mut self, p: &Photo, s: &DevelopSettings) -> (u64, Option<Arc<Input>>) {
        if !lightcraft_develop::retouch::active(s) {
            return (0, None);
        }
        match self.retouch_faces.get(&self.retouch_key(p, s)) {
            Some(found) => (found.input.fingerprint() | 1, Some(found.input)),
            None => {
                self.retouch_search(p.id, false);
                (self.retouch_faces.unknown_salt(p.id), None)
            }
        }
    }

    /// [`Self::retouch_for`]'s salt alone, starting nothing (cache keys of renders made elsewhere).
    pub(crate) fn retouch_salt(&self, p: &Photo, s: &DevelopSettings) -> u64 {
        if !lightcraft_develop::retouch::active(s) {
            return 0;
        }
        match self.retouch_faces.get(&self.retouch_key(p, s)) {
            Some(found) => found.input.fingerprint() | 1,
            None => self.retouch_faces.unknown_salt(p.id),
        }
    }

    /// The key photo `p`'s faces are kept under: [`key_of`] from a provider. From the people parse: the content,
    /// the orientation and the parse's version only (another set, or the set downloaded later, finds them anew):
    /// the parse is made on the photo with default settings, and the step maps it through the lens corrections and
    /// transform, so dragging those, or a preset's thumbnail, finds the same faces.
    fn retouch_key(&self, p: &Photo, s: &DevelopSettings) -> String {
        match self.retouch_source_is_parts() {
            true => format!("{}|{:?}|parts:{}", crate::media::content_key(p), s.orientation, self.parsing.version()),
            false => key_of(p, s),
        }
    }

    fn retouch_source_is_parts(&self) -> bool {
        self.retouch_faces.provider.is_none() && self.parsing.ready()
    }

    /// Photo `id`'s lens corrections and transform as its settings are now, to match a face's own settings kept in
    /// the other frame than the faces' (`lightcraft_develop::retouch::KeyFrame`).
    pub fn retouch_key_map(&self, id: PhotoId) -> Option<lightcraft_pipeline::retouch_step::FrameKeyMap> {
        let d = self.develop_of(id)?;
        let frame = self.mask_frame(id, &d)?;
        Some(lightcraft_pipeline::retouch_step::FrameKeyMap::new(&frame, d.orientation))
    }

    /// Where photo `id`'s faces stand (no search is started).
    pub fn retouch_faces_of(&self, id: PhotoId) -> FacesState {
        let Some(p) = self.catalog.photo(id) else { return FacesState::default() };
        let key = self.retouch_key(p, &p.develop);
        let st = self.retouch_faces.lock();
        FacesState {
            found: st.found.iter().find(|(k, _, _)| *k == key).map(|(_, _, f)| f.clone()),
            busy: st.busy.iter().any(|(k, _)| *k == key),
            error: st.failed.iter().chain(&st.no_model).find(|(k, _)| *k == key).map(|(_, e)| e.clone()),
        }
    }

    /// Find photo `id`'s faces on a worker thread (once: nothing happens while they are known or being found, or
    /// while another search of the photo runs; `again` searches anew). Without a face model nothing starts and the
    /// reason comes back; it is not kept as the photo's failure: the reason is kept for the photo until a face model
    /// is installed or removed, so asking every frame reads no model again, and the search runs once a model is
    /// installed.
    pub fn retouch_search(&mut self, id: PhotoId, again: bool) -> Option<String> {
        let p = self.catalog.photo(id).cloned()?;
        let key = self.retouch_key(&p, &p.develop);
        if !again && (self.retouch_faces.get(&key).is_some() || self.retouch_faces.lock().failed.iter().any(|(k, _)| *k == key)) {
            return None;
        }
        if !again && let Some(why) = self.retouch_faces.no_model_for(&key) {
            return Some(why);
        }
        let source = match self.retouch_source() {
            Ok(source) => source,
            Err(why) => {
                self.retouch_faces.keep_no_model(&key, &why);
                return Some(why);
            }
        };
        if !self.retouch_faces.claim(&key, id) {
            return None;
        }
        let finder = match self.retouch_finder(&p, source) {
            Ok(finder) => finder,
            Err(why) => {
                self.retouch_faces.put(&key, id, Err(why));
                return None;
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.retouch_faces.lock().started += 1;
            let store = self.retouch_faces.clone();
            let guard = self.activity.start("retouch.faces", &format!("Finding Faces ({})", finder.name()), crate::activity::Cancel::No);
            let spawned = std::thread::Builder::new().name("retouch-faces".into()).spawn({
                let key = key.clone();
                let store = store.clone();
                move || {
                    let result = finder.run();
                    if let Err(e) = &result {
                        log::warn!("portrait: faces not found: {e}");
                    }
                    store.put(&key, id, result);
                    drop(guard);
                }
            });
            if let Err(e) = spawned {
                store.put(&key, id, Err(format!("could not start the face search: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = finder;
            self.retouch_faces.release(&key);
        }
        None
    }

    /// Photo `id`'s faces, found now when they are not known (exports and synchronous renders, `retouch.detect`).
    /// `again` searches anew.
    pub fn retouch_detect_now(&mut self, id: PhotoId, again: bool) -> crate::Result<Found> {
        let p = self.catalog.photo(id).cloned().ok_or_else(|| EngineError::Other("no such photo".into()))?;
        let key = self.retouch_key(&p, &p.develop);
        if !again && let Some(found) = self.retouch_faces.get(&key) {
            return Ok(found);
        }
        // (claimed first: the render below must not start a search of its own)
        let claimed = self.retouch_faces.claim(&key, id);
        let source = match self.retouch_source() {
            Ok(p) => p,
            Err(why) => {
                if claimed {
                    self.retouch_faces.release(&key);
                }
                return Err(match self.retouch_faces.provider.is_none().then(|| crate::cmd::face_detect::detector(self).err()).flatten() {
                    Some(missing) => missing.into_error("retouch.detect"),
                    None => EngineError::Other(why),
                });
            }
        };
        let result = self.retouch_finder(&p, source).and_then(Finder::run);
        self.retouch_faces.put(&key, id, result.clone());
        result.map_err(EngineError::Other)
    }

    /// Exports and synchronous renders: find a retouched photo's faces first, so the result matches the preview.
    /// A failure is logged and the photo renders without faces.
    pub(crate) fn retouch_before_export(&mut self, id: PhotoId) {
        let Some(p) = self.catalog.photo(id) else { return };
        if !lightcraft_develop::retouch::active(&p.develop) {
            return;
        }
        // (a prepared export finds them on its own worker)
        if self.retouch_faces.lock().deferred.contains(&self.retouch_key(p, &p.develop)) {
            return;
        }
        if let Err(e) = self.retouch_detect_now(id, false) {
            log::warn!("portrait: photo {} renders without faces: {e}", id.0);
        }
    }

    /// For an export set up now and run later (on a worker): when the photo is retouched and its faces are not known,
    /// the search the export runs first ([`ExportFaces::fill`]), so setting it up never waits for a model.
    pub(crate) fn retouch_export_faces(&mut self, id: PhotoId) -> Option<ExportFaces> {
        let p = self.catalog.photo(id)?.clone();
        if !lightcraft_develop::retouch::active(&p.develop) {
            return None;
        }
        let key = self.retouch_key(&p, &p.develop);
        if self.retouch_faces.get(&key).is_some() {
            return None;
        }
        let source = self.retouch_source().ok()?;
        let claimed = self.retouch_faces.claim(&key, id);
        self.retouch_faces.lock().deferred.push(key.clone());
        // (dropped unfilled when the render below can't be built: the claim goes again)
        let mut pending = ExportFaces { store: self.retouch_faces.clone(), key, id, finder: None, claimed };
        // (built after the claim: this render starts no search of its own)
        pending.finder = Some(self.retouch_finder(&p, source).ok()?);
        Some(pending)
    }

    /// What finds faces now: an installed provider; else the people parse while its set is downloaded; else YuNet.
    fn retouch_source(&self) -> std::result::Result<Source, String> {
        if let Some(p) = &self.retouch_faces.provider {
            return Ok(Source::Provider(p.clone()));
        }
        if self.parsing.ready() {
            return Ok(Source::Parts);
        }
        match crate::cmd::face_detect::detector(self) {
            Ok(d) => Ok(Source::Provider(Arc::new(Yunet(d)))),
            Err(missing) => Err(missing.into_error("retouch.detect").to_string()),
        }
    }

    /// The search `source` runs for photo `p`, set up here and run anywhere.
    fn retouch_finder(&mut self, p: &Photo, source: Source) -> std::result::Result<Finder, String> {
        let regions = named_regions(p);
        Ok(match source {
            Source::Provider(provider) => Finder::Faces { provider, job: self.retouch_detect_job(p.id)?, regions },
            Source::Parts => {
                let choice = self.parsing.choice();
                Finder::Parts(parts::PartsFinder {
                    // (the parse-ahead index, `fork/parse-ahead.md`: a photo parsed before is read, not run again)
                    known: crate::parse_ahead::known(self, p.id, &choice.version()),
                    // (YuNet is loaded on the search's worker, never here)
                    prepare: crate::cmd::parsing::prepare_ahead(self, p.id)?,
                    parsing: self.parsing.clone(),
                    choice,
                    orient: p.develop.orientation,
                    regions,
                    // (Stray Hair is searched on full-size crops of the original: `parts/crop.rs`)
                    cropper: self.retouch_cropper(p.id).map(Box::new),
                })
            }
        })
    }

    /// The render faces are found on: the photo before its crop, without retouching, at [`DETECT_EDGE`].
    fn retouch_detect_job(&mut self, id: PhotoId) -> std::result::Result<crate::RenderJob, String> {
        let mut job = self.render_job(id, DETECT_EDGE, DETECT_EDGE, false, false).ok_or("no such photo")?;
        let mut s = (*job.settings).clone();
        s.retouch = Default::default();
        job.settings = Arc::new(s);
        job.info.retouch = None;
        job.cache = None;
        job.view_cache = None;
        Ok(job)
    }
}

/// A face search an export runs before its render ([`Session::retouch_export_faces`]). Dropped unrun (an export
/// cancelled or failed), it gives the photo back to the ordinary search.
pub struct ExportFaces {
    store: FaceStore,
    key: String,
    id: PhotoId,
    finder: Option<Finder>,
    /// It holds the photo's search claim (else a background search had it first).
    claimed: bool,
}

impl ExportFaces {
    /// Find the faces (keeping them for the photo's previews) and hand them to `jobs`. A failure is logged and the
    /// photo renders without faces.
    pub fn fill(mut self, jobs: &mut [&mut crate::RenderJob]) {
        let Some(finder) = self.finder.take() else { return };
        let result = finder.run();
        // (`put` ends the claim; dropping `self` then ends the deferral)
        match &result {
            Ok(found) => {
                for j in jobs.iter_mut() {
                    j.info.retouch = Some(found.input.clone());
                }
            }
            Err(e) => log::warn!("portrait: the export renders without faces: {e}"),
        }
        self.store.put(&self.key, self.id, result);
    }
}

impl Drop for ExportFaces {
    fn drop(&mut self) {
        let mut st = self.store.lock();
        st.deferred.retain(|k| *k != self.key);
        // unfilled (`fill` puts the result, which ends the claim itself): the photo's ordinary search may run again
        if self.claimed {
            st.busy.retain(|(k, _)| *k != self.key);
        }
    }
}

/// The photo's named face regions in the frame faces are found in (after the photo's own rotation), with the
/// name trimmed and in lower case.
fn named_regions(p: &Photo) -> Vec<(NormRect, String)> {
    p.meta
        .regions
        .iter()
        .filter(|r| r.kind == lightcraft_meta::RegionKind::Face)
        .filter_map(|r| {
            let name = r.name.as_deref().map(str::trim).filter(|n| !n.is_empty())?.to_lowercase();
            let b = p.develop.orientation.map_norm_rect(r.rect);
            Some((NormRect::new(b.x0, b.y0, b.x1, b.y1), name))
        })
        .collect()
}

/// Run the render and the provider, name the faces and cut their thumbnails.
fn find(job: crate::RenderJob, provider: &dyn FaceProvider, regions: &[(NormRect, String)]) -> std::result::Result<Found, String> {
    let orient = job.settings.orientation;
    let img = job.run().rendered?.image;
    let mut faces = provider.faces(&img)?;
    // each face's box on the photo before the user's rotation, which per-face settings are kept by
    for f in &mut faces {
        let b = orient.inverse().map_norm_rect(lightcraft_geom::Rect { x0: f.bounds.x0, y0: f.bounds.y0, x1: f.bounds.x1, y1: f.bounds.y1 });
        f.upright = Some(NormRect::new(b.x0, b.y0, b.x1, b.y1));
    }
    arrange(&mut faces, regions);
    let thumbs = faces.iter().map(|f| thumb(&img, &f.bounds)).collect();
    Ok(Found { input: Arc::new(Input::new(faces)), thumbs: Arc::new(thumbs) })
}

/// The first [`lightcraft_develop::retouch::MAX_FACES`] faces (the largest, as the finder ranks them), left to right as
/// the face picker shows them (per-face settings never depend on the order), each named after its region.
fn arrange(faces: &mut Vec<Face>, regions: &[(NormRect, String)]) {
    faces.truncate(lightcraft_develop::retouch::MAX_FACES);
    faces.sort_by(|a, b| a.bounds.centre().0.total_cmp(&b.bounds.centre().0).then(a.bounds.centre().1.total_cmp(&b.bounds.centre().1)));
    name_faces(faces, regions);
}

/// Give each face the name of the region it overlaps most (at least a third of the smaller box, by intersection
/// over union), each name to one face.
pub fn name_faces(faces: &mut [Face], regions: &[(NormRect, String)]) {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (fi, f) in faces.iter().enumerate() {
        for (ri, (r, _)) in regions.iter().enumerate() {
            let iou = iou(&f.bounds, r);
            if iou >= 0.3 {
                pairs.push((iou, fi, ri));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2)));
    let (mut face_done, mut region_done) = (vec![false; faces.len()], vec![false; regions.len()]);
    for (_, fi, ri) in pairs {
        if face_done.get(fi).copied().unwrap_or(true) || region_done.get(ri).copied().unwrap_or(true) {
            continue;
        }
        if let (Some(f), Some((_, name))) = (faces.get_mut(fi), regions.get(ri)) {
            f.person = Some(name.clone());
        }
        if let Some(d) = face_done.get_mut(fi) {
            *d = true;
        }
        if let Some(d) = region_done.get_mut(ri) {
            *d = true;
        }
    }
}

fn iou(a: &NormRect, b: &NormRect) -> f64 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let inter = w * h;
    let union = a.width().abs() * a.height().abs() + b.width().abs() * b.height().abs() - inter;
    if union > 0.0 && inter.is_finite() { inter / union } else { 0.0 }
}

/// A [`THUMB`]-pixel square around `b` (room for hair and chin), cut from `img`.
fn thumb(img: &Rgba8, b: &NormRect) -> Rgba8 {
    let (w, h) = (img.width as f64, img.height as f64);
    let side = (b.width() * w).max(b.height() * h) * 1.5;
    let (cx, cy) = (b.centre().0 * w, b.centre().1 * h);
    let ok = side.is_finite() && cx.is_finite() && cy.is_finite() && side >= 1.0 && img.width > 0 && img.height > 0;
    Rgba8::from_fn(THUMB, THUMB, |x, y| {
        if !ok {
            return [0, 0, 0, 255];
        }
        let sx = cx - side / 2.0 + (x as f64 + 0.5) * side / THUMB as f64;
        let sy = cy - side / 2.0 + (y as f64 + 0.5) * side / THUMB as f64;
        if sx < 0.0 || sy < 0.0 || sx >= w || sy >= h {
            return [0, 0, 0, 255];
        }
        img.data.get(sy as usize * img.width + sx as usize).copied().unwrap_or([0, 0, 0, 255])
    })
}

#[cfg(test)]
pub(crate) mod tests;
