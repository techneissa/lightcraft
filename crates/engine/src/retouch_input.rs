//! Fork feature (`fork/retouch-foundation.md`): the faces portrait retouching works on, per photo.
//!
//! A [`FaceProvider`] finds the faces (and, once a people parser is installed, their part masks) on a render of the
//! photo before its crop, without retouching. The result is kept per photo in the session's [`FaceStore`] (in
//! memory, the last [`KEEP`] photos) and handed to every render of the photo in `SourceInfo::retouch`; its
//! fingerprint is part of the render keys ([`Session::retouch_for`]), so a preview made before the faces were known
//! is redrawn once they are.
//!
//! - Renders never wait: a photo whose retouching is on and whose faces are unknown starts a search on a worker
//!   thread, with a row in the activity stack, and renders without faces meanwhile (every feature then changes
//!   nothing). Exports and synchronous renders find them first ([`Session::retouch_detect_now`]).
//! - The default provider is YuNet (`crates/faces`, installed in Settings ▸ Faces): boxes and five landmarks, no
//!   masks. Another provider (the people parser, issue #81) is installed with [`FaceStore::set_provider`].
//! - A face's person is the name of the photo's named face region it overlaps most (People). Faces are listed left
//!   to right.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::{Photo, PhotoId};
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_raster::Rgba8;

use crate::{EngineError, Session};

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
                landmarks: f.landmarks.iter().map(|(x, y)| (f64::from(*x), f64::from(*y))).collect(),
                score: f.score,
                person: None,
                masks: Vec::new(),
            })
            .collect())
    }
}

/// Long edge of the render faces are found on.
pub const DETECT_EDGE: usize = 1600;
/// Photos whose faces are kept in memory.
pub const KEEP: usize = 64;
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

#[derive(Default)]
struct Store {
    /// (key, faces), oldest first.
    found: Vec<(String, Found)>,
    /// Keys being searched now.
    busy: Vec<String>,
    /// The last search's failure, per key.
    failed: Vec<(String, String)>,
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
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, key: &str) -> Option<Found> {
        self.lock().found.iter().find(|(k, _)| k == key).map(|(_, f)| f.clone())
    }

    fn put(&self, key: &str, result: Result<Found, String>) {
        let mut st = self.lock();
        st.busy.retain(|k| k != key);
        st.found.retain(|(k, _)| k != key);
        st.failed.retain(|(k, _)| k != key);
        match result {
            Ok(found) => {
                st.found.push((key.to_string(), found));
                let extra = st.found.len().saturating_sub(KEEP);
                st.found.drain(..extra);
            }
            Err(why) => {
                st.failed.push((key.to_string(), why));
                let extra = st.failed.len().saturating_sub(KEEP);
                st.failed.drain(..extra);
            }
        }
    }

    /// Mark `key` as being searched; false when it already is.
    fn claim(&self, key: &str) -> bool {
        let mut st = self.lock();
        if st.busy.iter().any(|k| k == key) {
            return false;
        }
        st.busy.push(key.to_string());
        true
    }

    fn release(&self, key: &str) {
        self.lock().busy.retain(|k| k != key);
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

/// The key of a photo's faces: its content, and every setting that moves pixels before the crop (the faces are in
/// the photo's coordinates before the crop, after orientation, lens corrections and transform).
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
        match self.retouch_faces.get(&key_of(p, s)) {
            Some(found) => (found.input.fingerprint() | 1, Some(found.input)),
            None => {
                self.retouch_search(p.id, false);
                (UNKNOWN, None)
            }
        }
    }

    /// [`Self::retouch_for`]'s salt alone, starting nothing (cache keys of renders made elsewhere).
    pub(crate) fn retouch_salt(&self, p: &Photo, s: &DevelopSettings) -> u64 {
        if !lightcraft_develop::retouch::active(s) {
            return 0;
        }
        self.retouch_faces.get(&key_of(p, s)).map_or(UNKNOWN, |found| found.input.fingerprint() | 1)
    }

    /// Where photo `id`'s faces stand (no search is started).
    pub fn retouch_faces_of(&self, id: PhotoId) -> FacesState {
        let Some(p) = self.catalog.photo(id) else { return FacesState::default() };
        let key = key_of(p, &p.develop);
        let st = self.retouch_faces.lock();
        FacesState {
            found: st.found.iter().find(|(k, _)| *k == key).map(|(_, f)| f.clone()),
            busy: st.busy.contains(&key),
            error: st.failed.iter().find(|(k, _)| *k == key).map(|(_, e)| e.clone()),
        }
    }

    /// Find photo `id`'s faces on a worker thread (once: nothing happens while they are known or being found, and
    /// `again` searches anew). Without a face model nothing starts; the failure says why ([`FacesState::error`]).
    pub fn retouch_search(&mut self, id: PhotoId, again: bool) {
        let Some(p) = self.catalog.photo(id).cloned() else { return };
        let key = key_of(&p, &p.develop);
        if !again && (self.retouch_faces.get(&key).is_some() || self.retouch_faces.lock().failed.iter().any(|(k, _)| *k == key)) {
            return;
        }
        if !self.retouch_faces.claim(&key) {
            return;
        }
        let job = match self.retouch_provider().and_then(|provider| Ok((provider, self.retouch_detect_job(id)?))) {
            Ok(job) => job,
            Err(why) => {
                self.retouch_faces.put(&key, Err(why));
                return;
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (provider, job) = job;
            let store = self.retouch_faces.clone();
            let regions = named_regions(&p);
            let guard = self.activity.start("retouch.faces", &format!("Finding Faces ({})", provider.name()), crate::activity::Cancel::No);
            let spawned = std::thread::Builder::new().name("retouch-faces".into()).spawn({
                let key = key.clone();
                let store = store.clone();
                move || {
                    let result = find(job, provider.as_ref(), &regions);
                    if let Err(e) = &result {
                        log::warn!("portrait: faces not found: {e}");
                    }
                    store.put(&key, result);
                    drop(guard);
                }
            });
            if let Err(e) = spawned {
                store.put(&key, Err(format!("could not start the face search: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = job;
            self.retouch_faces.release(&key);
        }
    }

    /// Photo `id`'s faces, found now when they are not known (exports and synchronous renders, `retouch.detect`).
    /// `again` searches anew.
    pub fn retouch_detect_now(&mut self, id: PhotoId, again: bool) -> crate::Result<Found> {
        let p = self.catalog.photo(id).cloned().ok_or_else(|| EngineError::Other("no such photo".into()))?;
        let key = key_of(&p, &p.develop);
        if !again && let Some(found) = self.retouch_faces.get(&key) {
            return Ok(found);
        }
        // (claimed first: the render below must not start a search of its own)
        let claimed = self.retouch_faces.claim(&key);
        let provider = match self.retouch_provider() {
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
        let job = self.retouch_detect_job(id);
        let result = job.and_then(|job| find(job, provider.as_ref(), &named_regions(&p)));
        self.retouch_faces.put(&key, result.clone());
        result.map_err(EngineError::Other)
    }

    /// Exports and synchronous renders: find a retouched photo's faces first, so the result matches the preview.
    /// A failure is logged and the photo renders without faces.
    pub(crate) fn retouch_before_export(&mut self, id: PhotoId) {
        let Some(p) = self.catalog.photo(id) else { return };
        if !lightcraft_develop::retouch::active(&p.develop) {
            return;
        }
        if let Err(e) = self.retouch_detect_now(id, false) {
            log::warn!("portrait: photo {} renders without faces: {e}", id.0);
        }
    }

    fn retouch_provider(&self) -> std::result::Result<Arc<dyn FaceProvider>, String> {
        if let Some(p) = &self.retouch_faces.provider {
            return Ok(p.clone());
        }
        match crate::cmd::face_detect::detector(self) {
            Ok(d) => Ok(Arc::new(Yunet(d))),
            Err(missing) => Err(missing.into_error("retouch.detect").to_string()),
        }
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
    let img = job.run().rendered?.image;
    let mut faces = provider.faces(&img)?;
    faces.truncate(lightcraft_develop::retouch::MAX_FACES);
    // left to right, as the face picker shows them (per-face settings never depend on the order)
    faces.sort_by(|a, b| a.bounds.centre().0.total_cmp(&b.bounds.centre().0).then(a.bounds.centre().1.total_cmp(&b.bounds.centre().1)));
    name_faces(&mut faces, regions);
    let thumbs = faces.iter().map(|f| thumb(&img, &f.bounds)).collect();
    Ok(Found { input: Arc::new(Input::new(faces)), thumbs: Arc::new(thumbs) })
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
