//! Fork feature (`fork/ai-culling.md`): Assisted Culling's eyes. For each photo, on a worker thread: the faces and
//! their face meshes, then whether the main faces' eyes are closed and in focus
//! ([`lightcraft_pipeline::cull::eyes`]).
//!
//! The faces come from the photo's people parse when it has one (no model runs); otherwise only the face detector
//! (YuNet, Settings ▸ Faces) and the face mesh run, not the whole parse. Either way the picture is the one the parse
//! reads (`cmd::parsing::prepare_plain`: upright, uncropped, default settings, `PARSE_EDGE` long), so the parse's
//! landmarks fall on it.
//!
//! [`EyeJob`] is set up on the calling thread (no file is read); [`Tools::measure`] runs anywhere, without the session.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::{Eyes, PhotoId};
use lightcraft_parsing::{NormRect, Parsing, RgbImage};
use lightcraft_pipeline::cull::eyes::{self as math, FaceEyes, FaceIn, Luma};

use crate::Session;
use crate::cmd::parsing::Prepare;
use crate::parse_ahead::Known;
use crate::people_parsing::PeopleParsing;

/// What one photo's eyes need, taken on the calling thread.
pub struct EyeJob {
    pub id: PhotoId,
    known: Option<Known>,
    prepare: Prepare,
}

/// One face as measured.
#[derive(Clone, Debug)]
pub struct Face {
    pub rect: NormRect,
    pub eyes: FaceEyes,
    /// How sure the face mesh was that it saw a face (0 to 1; 0: no mesh, the face too small to mesh).
    pub mesh_score: f32,
}

/// One photo's eyes.
#[derive(Clone, Debug)]
pub struct PhotoEyes {
    pub faces: Vec<Face>,
    pub summary: Eyes,
    /// The faces came from the photo's people parse (no model ran).
    pub from_parse: bool,
}

/// The models the measurements share for a run (loaded on a worker, the first time a photo needs them; one photo
/// at a time runs them, the renders go on meanwhile).
pub struct Tools {
    parsing: PeopleParsing,
    detector: Option<PathBuf>,
    models: Option<PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    runtime: lightcraft_parsing::coreml::Runtime,
    #[cfg(not(target_arch = "wasm32"))]
    loaded: std::sync::Mutex<Loaded>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct Loaded {
    yunet: Option<Result<std::sync::Arc<lightcraft_faces::yunet::Detector>, String>>,
    mesher: Option<Result<lightcraft_parsing::mesher::Mesher, String>>,
}

impl Tools {
    /// The session's face detector, models folder and mesh engine; nothing is loaded yet.
    pub fn new(s: &Session) -> Self {
        Self {
            parsing: s.parsing.clone(),
            detector: crate::cmd::face_detect::detector_file(s).map(|(p, _)| p),
            models: s.parsing.models_dir(),
            #[cfg(not(target_arch = "wasm32"))]
            runtime: s.parsing.runtimes().mesh,
            #[cfg(not(target_arch = "wasm32"))]
            loaded: std::sync::Mutex::default(),
        }
    }

    /// Why no eyes can be read for a photo that has no people parse (`None`: they can).
    pub fn missing(&self) -> Option<String> {
        if cfg!(target_arch = "wasm32") {
            return Some("eyes are not read in the web version".into());
        }
        if self.detector.is_none() {
            return Some("no face detector: download it in Settings ▸ Faces".into());
        }
        let mesh = lightcraft_parsing::models::FACE_MESH;
        if !self.models.as_deref().is_some_and(|m| mesh.installed_in(m)) {
            return Some("the face mesh is not downloaded: download a People Parsing model set in Settings".into());
        }
        None
    }

    /// The engine the face mesh ran on (`coreml` or `candle`), once it has run.
    pub fn mesh_engine(&self) -> Option<&'static str> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let loaded = self.loaded.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loaded.mesher.as_ref().and_then(|m| m.as_ref().ok()).map(|m| m.backend().id())
        }
        #[cfg(target_arch = "wasm32")]
        None
    }

    /// Measure one photo's eyes (never panics: a failure, a panic inside a model or the render, and a stop are
    /// errors).
    pub fn measure(&self, job: EyeJob, cancel: &AtomicBool) -> Result<PhotoEyes, String> {
        crate::guard::catch("Assisted Culling", || self.measure_inner(job, cancel))?
    }

    fn measure_inner(&self, job: EyeJob, cancel: &AtomicBool) -> Result<PhotoEyes, String> {
        // (waits for the parse-ahead index when it is still being read, so a parsed photo never runs a model just
        // because the run started first; a parse made before landmarks existed, or whose faces were never meshed,
        // has no eyes to read: the models run, or the photo stays "not measured" and is tried again next time)
        let parse = job.known.as_ref().and_then(|k| k.read_when_loaded(&self.parsing, cancel)).filter(|p| has_landmarks(p));
        if parse.is_none()
            && let Some(why) = self.missing()
        {
            return Err(why);
        }
        let (img, regions) = crate::memory::in_background(|| job.prepare.render())?.into_parts();
        // the picture and what the eyes make of it, counted against the decode gate (the render itself is counted by
        // its decode): culling's lanes together never hold more than the gate allows, whatever their number
        let _held = crate::memory::work_gate().acquire(held_bytes(img.width, img.height));
        if cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        let (rects, meshes, scores) = match &parse {
            Some(p) => faces_of_parse(p),
            None => self.faces_by_mesh(&img, &regions, cancel)?,
        };
        let luma = math::luma_of_rgb8(&img.data);
        let picture = Luma { data: &luma, width: img.width, height: img.height };
        let faces_in: Vec<FaceIn> = rects.iter().zip(&meshes).map(|(r, m)| FaceIn { rect: [r.x0, r.y0, r.x1, r.y1], landmarks: m }).collect();
        let read = math::read(&picture, &faces_in);
        let s = math::summary(&read);
        Ok(PhotoEyes {
            faces: rects.into_iter().zip(read).zip(scores).map(|((rect, eyes), mesh_score)| Face { rect, eyes, mesh_score }).collect(),
            summary: Eyes { faces: s.faces, main: s.main, closed: s.closed, focus: s.focus.map(|f| (f * 10.0).round() / 10.0) },
            from_parse: parse.is_some(),
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn faces_by_mesh(&self, _: &RgbImage, _: &[lightcraft_meta::Region], _: &AtomicBool) -> Result<FacesFound, String> {
        Err("eyes are not read in the web version".into())
    }

    /// The fast path: YuNet's faces and the face mesh of each, no parse.
    #[cfg(not(target_arch = "wasm32"))]
    fn faces_by_mesh(&self, img: &RgbImage, regions: &[lightcraft_meta::Region], cancel: &AtomicBool) -> Result<FacesFound, String> {
        let path = self.detector.clone().ok_or("no face detector")?;
        let mut loaded = self.loaded.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let detector = loaded.yunet.get_or_insert_with(|| crate::cmd::face_detect::detector_at(&path)).clone()?;
        let hints: Vec<lightcraft_parsing::FaceHint> = crate::cmd::parsing::hints_for(Some(&detector), img, regions)?;
        if hints.is_empty() {
            return Ok((Vec::new(), Vec::new(), Vec::new()));
        }
        let (models, runtime) = (self.models.clone().ok_or("no models folder")?, self.runtime);
        let mesher = loaded
            .mesher
            .get_or_insert_with(|| {
                // (the units measured fastest for the face mesh on this Mac, issue #267)
                let runtimes = lightcraft_parsing::Runtimes { mesh: runtime, ..Default::default() };
                let (runtimes, units) = crate::people_parsing::measured_units(&models, runtimes, &[lightcraft_parsing::models::FACE_MESH.folder]);
                lightcraft_parsing::mesher::Mesher::load_on_units(&models, lightcraft_parsing::best_device(), runtimes.mesh, units.mesh)
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let found = mesher.meshes(img, &hints, &|| cancel.load(Ordering::Relaxed)).map_err(|e| e.to_string())?;
        let mut meshes = vec![Vec::new(); hints.len()];
        let mut scores = vec![0.0; hints.len()];
        for m in found {
            if let (Some(slot), Some(score)) = (meshes.get_mut(m.face), scores.get_mut(m.face)) {
                *slot = m.landmarks.iter().map(|l| [l.x, l.y, l.z]).collect();
                *score = m.score;
            }
        }
        Ok((hints.into_iter().map(|h| h.rect).collect(), meshes, scores))
    }
}

/// Faces found: their boxes, meshes (empty without one) and mesh scores, in one order.
type FacesFound = (Vec<NormRect>, Vec<Vec<[f32; 3]>>, Vec<f32>);

/// The faces of a people parse: each person's face box, mesh and mesh score.
fn faces_of_parse(p: &Parsing) -> FacesFound {
    let mut out: FacesFound = Default::default();
    for q in &p.people {
        let Some(face) = q.face else { continue };
        out.0.push(face);
        out.1.push(q.landmarks.iter().map(|l| [l.x, l.y, l.z]).collect());
        out.2.push(q.landmark_score);
    }
    out
}

/// Bytes one photo's eye reading holds once its picture is rendered: the 8-bit RGB picture (3 per pixel), its
/// luminance (4) and the detail maps at full scale (3 × 4). A 2048 × 1365 picture holds about 53 MB, so the four
/// lanes of a run hold about 210 MB at most, inside the decode gate's quarter of the budget (384 MiB by default).
fn held_bytes(width: usize, height: usize) -> usize {
    width.saturating_mul(height).saturating_mul(3 + 4 + 12)
}

/// Whether a parse has the face mesh of any of its faces (or has no face to mesh): a parse that has faces but no
/// mesh for any of them was made before landmarks were kept, or never meshed them.
fn has_landmarks(p: &Parsing) -> bool {
    let faces: Vec<_> = p.people.iter().filter(|q| q.face.is_some()).collect();
    faces.is_empty() || faces.iter().any(|q| !q.landmarks.is_empty() || q.landmark_score > 0.0)
}

/// Set up the eyes of photo `id` (no file is read).
pub fn job(s: &mut Session, id: PhotoId) -> Option<EyeJob> {
    let version = s.parsing.ahead_version();
    let known = crate::parse_ahead::known(s, id, &version);
    let prepare = crate::cmd::parsing::prepare_plain(s, id).ok()?;
    Some(EyeJob { id, known, prepare })
}
