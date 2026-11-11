//! Loading a model set and parsing a photo with it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use candle_core::Device;

use crate::body;
use crate::coreml::{self, Runtime};
use crate::face::{self, FaceNet, Groups, Job, Nets};
use crate::facemesh::{FaceMesh, MeshOut};
use crate::image::Crop;
use crate::models::{FaceModel, ModelFile, ModelSet};
use crate::nn::{Layout, Params};
use crate::people;
use crate::sapiens::Sapiens2;
use crate::segformer::Segformer;
use crate::types::{FaceHint, NormRect, Parsing, Part, Person, Plane};
use crate::weights::Weights;
use crate::{Error, Result};

/// How a parse runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Long side of the picture the body model sees (a multiple of 32).
    pub body_side: usize,
    /// Most people kept (the largest faces first).
    pub max_people: usize,
    /// Faces smaller than this many photo pixels across get no face parse and no mesh (body parts only).
    pub min_face_px: f32,
    /// Faces per step on candle (Core ML takes up to [`coreml::FACE_BATCH`]); the mesh takes them together, and the
    /// face parser too except on the GPU, where it takes them one at a time ([`candle_face_calls`]).
    pub batch: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { body_side: 768, max_people: 64, min_face_px: 14.0, batch: 4 }
    }
}

/// Which engine runs the face parser, the clothes parser and the face mesh. Of the parsers only the SegFormer set's two
/// have a Core ML version; the others always run on candle. Every set has the same face mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Runtimes {
    pub face: Runtime,
    pub body: Runtime,
    pub mesh: Runtime,
}

/// Which engine runs a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    CoreMl,
    Candle,
}

impl Backend {
    pub fn id(self) -> &'static str {
        match self {
            Backend::CoreMl => "coreml",
            Backend::Candle => "candle",
        }
    }
}

pub(crate) type CoreMlResult<T> = std::result::Result<T, lightcraft_coreml::Error>;

/// One parser of a set: its Core ML version when it has one and it loaded, and its candle port, loaded at once when
/// it is the one that runs, else only when Core ML fails.
pub(crate) struct Engine<T> {
    label: &'static str,
    runtime: Runtime,
    coreml: Option<lightcraft_coreml::Model>,
    /// Set when Core ML failed on a photo: candle runs from then on.
    failed: AtomicBool,
    candle: OnceLock<std::result::Result<T, String>>,
    /// Make the next Core ML call fail (tests).
    #[cfg(test)]
    fail_next: AtomicBool,
}

impl<T> Engine<T> {
    /// `coreml`: the loader of `file`'s Core ML version, when it has one. `candle`: the candle port's loader.
    pub(crate) fn load(
        file: &ModelFile,
        runtime: Runtime,
        coreml: Option<&dyn Fn() -> CoreMlResult<lightcraft_coreml::Model>>,
        candle: impl FnOnce() -> Result<T>,
    ) -> Result<Self> {
        let loaded = match (runtime, coreml) {
            (Runtime::Candle, _) | (Runtime::Automatic, None) => None,
            (Runtime::CoreMl, None) => return Err(Error::Model(format!("the {} has no Core ML version: choose Automatic or Candle", file.label))),
            (Runtime::CoreMl, Some(load)) => Some(load().map_err(|e| Error::Model(format!("{} (Core ML): {e}", file.label)))?),
            (Runtime::Automatic, Some(load)) => match load() {
                Ok(m) => Some(m),
                Err(lightcraft_coreml::Error::Unavailable | lightcraft_coreml::Error::Missing(_)) => None,
                Err(e) => {
                    log::warn!("parsing: the Core ML {} does not load ({e}); using candle", file.label);
                    None
                }
            },
        };
        let me = Self {
            label: file.label,
            runtime,
            coreml: loaded,
            failed: AtomicBool::new(false),
            candle: OnceLock::new(),
            #[cfg(test)]
            fail_next: AtomicBool::new(false),
        };
        if me.coreml.is_none() {
            // loaded now, so a missing or damaged file is the error it always was
            let _ = me.candle.set(Ok(candle()?));
        }
        Ok(me)
    }

    pub(crate) fn backend(&self) -> Backend {
        if self.coreml.is_some() && !self.failed.load(Ordering::Relaxed) { Backend::CoreMl } else { Backend::Candle }
    }

    /// Run on Core ML, or on candle (loaded by `load_candle` the first time it is needed).
    pub(crate) fn run<R>(
        &self,
        on_coreml: impl FnOnce(&lightcraft_coreml::Model) -> CoreMlResult<R>,
        on_candle: impl FnOnce(&T) -> Result<R>,
        load_candle: impl FnOnce() -> Result<T>,
    ) -> Result<R> {
        if let (Some(model), Backend::CoreMl) = (&self.coreml, self.backend()) {
            #[cfg(test)]
            if self.fail_next.swap(false, Ordering::Relaxed) {
                return self.after_coreml_failed(lightcraft_coreml::Error::Output("made to fail".into()), on_candle, load_candle);
            }
            match on_coreml(model) {
                Ok(r) => return Ok(r),
                Err(e) => return self.after_coreml_failed(e, on_candle, load_candle),
            }
        }
        self.on_candle(on_candle, load_candle)
    }

    /// Core ML failed with `e`: the error (Core ML chosen), else candle from now on.
    fn after_coreml_failed<R>(
        &self,
        e: lightcraft_coreml::Error,
        on_candle: impl FnOnce(&T) -> Result<R>,
        load_candle: impl FnOnce() -> Result<T>,
    ) -> Result<R> {
        if self.runtime == Runtime::CoreMl {
            return Err(Error::Compute(format!("{} (Core ML): {e}", self.label)));
        }
        log::warn!("parsing: Core ML failed on the {} ({e}); using candle from now on", self.label);
        self.failed.store(true, Ordering::Relaxed);
        self.on_candle(on_candle, load_candle)
    }

    fn on_candle<R>(&self, on_candle: impl FnOnce(&T) -> Result<R>, load_candle: impl FnOnce() -> Result<T>) -> Result<R> {
        let candle = self.candle.get_or_init(|| load_candle().map_err(|e| e.to_string())).as_ref().map_err(|e| Error::Model(e.clone()))?;
        on_candle(candle)
    }
}

/// Faces per candle face-parser call on `dev`, for `n` faces: one on the GPU, where candle 0.9.2 gives every face
/// after the first in a call other numbers than the same face alone (`fork/coreml.md`); all of them on the CPU.
pub(crate) fn candle_face_calls(dev: &Device, n: usize) -> usize {
    if dev.is_metal() { 1 } else { n.max(1) }
}

/// A loaded model set.
pub struct Parser {
    set: ModelSet,
    models: PathBuf,
    face: Engine<FaceNet>,
    face_model: FaceModel,
    body: Engine<Segformer>,
    /// The set's body-part parser (Sapiens2), which decides where skin, hair and clothes are.
    parts: Option<Sapiens2>,
    mesh: Engine<FaceMesh>,
    dev: Device,
}

/// The GPU on the Mac when there is one, else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

fn open(file: &ModelFile, models: &Path) -> Result<Arc<Weights>> {
    let path = file.path_in(models);
    file.verify(models)?;
    if !path.is_file() {
        return Err(Error::Missing(format!(
            "{} ({} in {})",
            file.label,
            file.name,
            path.parent().map_or(String::new(), |p| p.display().to_string())
        )));
    }
    Ok(Arc::new(Weights::open(&path).map_err(|e| Error::Model(format!("{}: {e}", file.label)))?))
}

/// A load error that names the file it came from.
fn named(f: &ModelFile, e: Error) -> Error {
    match e {
        Error::Model(m) if !m.starts_with(f.label) => Error::Model(format!("{}: {m}", f.label)),
        other => other,
    }
}

fn load_face_net(set: ModelSet, models: &Path, dev: &Device) -> Result<FaceNet> {
    let file = set.face_file();
    let fw = open(&file, models)?;
    Ok(match set.face_model() {
        FaceModel::Segformer => FaceNet::Segformer(Segformer::load(&Params::new(fw, dev.clone(), Layout::Oihw)).map_err(|e| named(&file, e))?),
        FaceModel::BiSeNet => {
            FaceNet::BiSeNet(Box::new(crate::bisenet::BiSeNet::load(&Params::new(fw, dev.clone(), Layout::Ohwi)).map_err(|e| named(&file, e))?))
        }
    })
}

pub(crate) fn load_mesh_net(models: &Path, dev: &Device) -> Result<FaceMesh> {
    let file = crate::models::FACE_MESH;
    FaceMesh::load(open(&file, models)?.as_ref(), dev).map_err(|e| named(&file, e))
}

fn load_body_net(set: ModelSet, models: &Path, dev: &Device) -> Result<Segformer> {
    let file = set.body_file();
    let body = Segformer::load(&Params::new(open(&file, models)?, dev.clone(), Layout::Oihw)).map_err(|e| named(&file, e))?;
    if body.classes() != body::CLASSES {
        return Err(Error::Model(format!("{}: {} classes, not {}", file.label, body.classes(), body::CLASSES)));
    }
    Ok(body)
}

impl Parser {
    /// Load `set` from the models folder on the best device, each parser on Core ML when it can (Automatic).
    pub fn load(set: ModelSet, models: &Path) -> Result<Self> {
        Self::load_with(set, models, best_device(), Runtimes::default())
    }

    pub fn load_on(set: ModelSet, models: &Path, dev: Device) -> Result<Self> {
        Self::load_with(set, models, dev, Runtimes::default())
    }

    /// Load `set` with its face and clothes parsers and the face mesh on the engines `runtimes` asks for; Sapiens2, and
    /// candle's models, run on `dev`.
    pub fn load_with(set: ModelSet, models: &Path, dev: Device, runtimes: Runtimes) -> Result<Self> {
        let (face_file, body_file) = (set.face_file(), set.body_file());
        let face_coreml = || coreml::load_face(models);
        let body_coreml = || coreml::load_body(models);
        let has = |f: &ModelFile| coreml::CONVERTED.contains(f);
        let face_loader: Option<&dyn Fn() -> CoreMlResult<lightcraft_coreml::Model>> = has(&face_file).then_some(&face_coreml);
        let body_loader: Option<&dyn Fn() -> CoreMlResult<lightcraft_coreml::Model>> = has(&body_file).then_some(&body_coreml);
        crate::profile::mark("load start");
        let face = Engine::load(&face_file, runtimes.face, face_loader, || load_face_net(set, models, &dev))?;
        crate::profile::mark("load face");
        let body = Engine::load(&body_file, runtimes.body, body_loader, || load_body_net(set, models, &dev))?;
        crate::profile::mark("load body");
        let parts = match set.parts_file() {
            Some(f) => Some(Sapiens2::load(&Params::new(open(&f, models)?, dev.clone(), Layout::Ohwi)).map_err(|e| named(&f, e))?),
            None => None,
        };
        let mesh_coreml = || coreml::load_mesh(models);
        let mesh_loader: &dyn Fn() -> CoreMlResult<lightcraft_coreml::Model> = &mesh_coreml;
        crate::profile::mark("load parts");
        let mesh = Engine::load(&crate::models::FACE_MESH, runtimes.mesh, Some(mesh_loader), || load_mesh_net(models, &dev))?;
        crate::profile::mark("load mesh");
        Ok(Self { set, models: models.to_path_buf(), face, face_model: set.face_model(), body, parts, mesh, dev })
    }

    pub fn set(&self) -> ModelSet {
        self.set
    }

    pub fn device(&self) -> &Device {
        &self.dev
    }

    /// The engines the next photo's face and clothes parsers run on.
    pub fn backends(&self) -> (Backend, Backend) {
        (self.face.backend(), self.body.backend())
    }

    /// The engine the next photo's face mesh runs on.
    pub fn mesh_backend(&self) -> Backend {
        self.mesh.backend()
    }

    /// The face mesh of `crops` (of [`crate::facemesh::SIDE`]²).
    pub(crate) fn meshes(&self, img: &crate::RgbImage, crops: &[Crop]) -> Result<Vec<MeshOut>> {
        self.mesh.run(
            |m| {
                let mut out = Vec::with_capacity(crops.len());
                for chunk in crops.chunks(coreml::FACE_BATCH) {
                    out.extend(coreml::meshes(m, &face::batch_pixels(img, chunk, false), chunk.len())?);
                }
                Ok(out)
            },
            |net| face::mesh_batch(net, img, crops, &self.dev),
            || load_mesh_net(&self.models, &self.dev),
        )
    }

    /// The clothes parser's map of the photo at long side `side`.
    pub(crate) fn body_map(&self, img: &crate::RgbImage, side: usize) -> Result<body::BodyMap> {
        let (w, h, px) = body::pixels(img, side);
        crate::profile::mark("body resize");
        let map = self.body.run(
            |m| coreml::body_probs(m, &px, w, h).map(|probs| body::BodyMap { w, h, probs }),
            |net| body::run(net, (w, h, px.clone()), &self.dev),
            || load_body_net(self.set, &self.models, &self.dev),
        );
        crate::profile::mark("body model");
        map
    }

    /// The face parser's groups for `crops`.
    pub(crate) fn face_groups(&self, img: &crate::RgbImage, crops: &[Crop]) -> Result<Groups> {
        self.face.run(
            |m| {
                // (a full batch in one call, any other number one crop a call: the model sees two input shapes only,
                // and a photo of a few faces never changes it, `coreml`)
                let per_call = if crops.len().is_multiple_of(coreml::FACE_BATCH) { coreml::FACE_BATCH } else { 1 };
                let mut out = Vec::with_capacity(crops.len());
                for chunk in crops.chunks(per_call) {
                    out.extend(coreml::face_groups(m, &face::batch_pixels(img, chunk, true), chunk.len())?);
                }
                Ok(out)
            },
            |net| {
                // (also when Core ML failed on these crops: they come here as one chunk of up to `FACE_BATCH`)
                let mut out = Vec::with_capacity(crops.len());
                for chunk in crops.chunks(candle_face_calls(&self.dev, crops.len())) {
                    out.extend(face::parse_batch(net, self.face_model, img, chunk, &self.dev)?);
                }
                Ok(out)
            },
            || load_face_net(self.set, &self.models, &self.dev),
        )
    }

    /// Make the face parser's next Core ML call fail, as a GPU error on a photo would.
    #[cfg(test)]
    pub(crate) fn fail_next_face_call(&self) {
        self.face.fail_next.store(true, Ordering::Relaxed);
    }

    /// Parse a photo. `hints`: faces a detector found (none: the body model's faces are used). `cancelled` is asked
    /// between steps; a cancelled parse is [`Error::Cancelled`].
    pub fn parse(&self, img: &crate::RgbImage, hints: &[FaceHint], opts: &Options, cancelled: &dyn Fn() -> bool) -> Result<Parsing> {
        let mut out =
            Parsing { model_set: self.set.id().into(), model_version: self.set.version(), width: img.width, height: img.height, people: Vec::new() };
        if img.width.max(img.height) < crate::MIN_SIDE {
            return Ok(out);
        }
        let mut prof = Profile::start();
        let (iw, ih) = (img.width as f32, img.height as f32);
        let given: Vec<FaceHint> = hints.iter().filter_map(|h| h.rect.clamped().map(|rect| FaceHint { rect, ..h.clone() })).collect();
        let mut map = self.body_map(img, body_side(&given, img, opts))?;
        prof.step("body");
        if cancelled() {
            return Err(Error::Cancelled);
        }
        // without hints the faces come from the clothes parser's map (Sapiens2 sees faces where there are none)
        let faces = faces_of(&given, hints.is_empty(), &map, iw, opts);
        if hints.is_empty() {
            // the faces were found in the map: look again, closer, when they are small
            let side = body_side(&faces, img, opts);
            if side > map.w.max(map.h) {
                map = self.body_map(img, side)?;
                prof.step("body again");
            }
        }
        let rects: Vec<NormRect> = faces.iter().map(|f| f.rect).collect();
        let map = match &self.parts {
            Some(net) => {
                let m = crate::crops::parts_map(&|x| net.forward(x), map, img, &rects, &self.dev, cancelled)?;
                prof.step("people up close");
                crate::profile::mark("sapiens2");
                m
            }
            None => map,
        };
        let owners = people::assign(&map.person(), &map.skin_or_hair(), map.w, map.h, &rects);
        let count = owners.people.min(opts.max_people);
        let mut people: Vec<Person> = (0..count)
            .map(|k| match faces.get(k) {
                Some(f) => Person { face: Some(f.rect), face_score: f.score, person_id: f.person_id.clone(), ..Person::default() },
                None => Person::default(),
            })
            .collect();
        let jobs: Vec<Job> = faces
            .iter()
            .enumerate()
            .filter(|(_, f)| (f.rect.width() * iw).max(f.rect.height() * ih) >= opts.min_face_px)
            .map(|(person, f)| Job { person, rect: f.rect, angle: eye_angle(f, iw, ih) })
            .collect();
        let claims = |p: usize, x: f32, y: f32| {
            let o = map.owner_at(&owners, x, y);
            o == 0 || usize::from(o) == p + 1
        };
        for (k, p) in people.iter_mut().enumerate() {
            let facial = !jobs.iter().any(|j| j.person == k);
            for (part, plane) in body::planes(&map, &owners, k, facial) {
                add(p, part, plane);
            }
        }
        crate::profile::mark("people split");
        let parse = |img: &crate::RgbImage, crops: &[Crop]| self.face_groups(img, crops);
        let mesh = |img: &crate::RgbImage, crops: &[Crop]| self.meshes(img, crops);
        let nets = Nets { parse: &parse, mesh: &mesh };
        let scene = face::Scene { claims: &claims, body_hat: &|x, y| map.at(body::HAT, x, y) };
        let on_coreml = self.face.backend() == Backend::CoreMl || self.mesh.backend() == Backend::CoreMl;
        let batch = if on_coreml { coreml::FACE_BATCH } else { opts.batch };
        for f in face::run(&nets, img, &jobs, batch, &scene, cancelled)? {
            if let Some(p) = people.get_mut(f.person) {
                for (part, plane) in f.planes {
                    add(p, part, plane);
                }
                p.landmarks = f.landmarks;
                p.landmark_score = f.landmark_score;
            }
        }
        prof.step("faces");
        for p in &mut people {
            p.bounds = p.parts.get(&Part::EntirePerson).and_then(|v| v.iter().map(|q| q.rect).reduce(|a, b| a.union(&b)));
        }
        people.retain(|p| p.face.is_some() || !p.parts.is_empty());
        people.sort_by(|a, b| b.area(Part::EntirePerson).total_cmp(&a.area(Part::EntirePerson)));
        out.people = people;
        crate::profile::mark("finish");
        Ok(out)
    }
}

/// The body model should see a typical face at least this many pixels tall.
const BODY_FACE_PX: f32 = 24.0;
/// The body model's input never grows past this long side (about four times the work of the default).
const MAX_BODY_SIDE: usize = 1536;

/// The body model's input side: the default, larger when the faces are small (so people in a group photo are more
/// than a few pixels in the map), never more than the photo has.
fn body_side(faces: &[FaceHint], img: &crate::RgbImage, opts: &Options) -> usize {
    let long = img.width.max(img.height) as f32;
    let mut heights: Vec<f32> = faces.iter().map(|f| f.rect.height() * img.height as f32 / long).filter(|h| h.is_finite() && *h > 0.0).collect();
    heights.sort_by(f32::total_cmp);
    let Some(&median) = heights.get(heights.len() / 2) else { return opts.body_side };
    let want = (BODY_FACE_PX / median).ceil() as usize;
    let cap = MAX_BODY_SIDE.max(opts.body_side).min((long as usize).div_ceil(32) * 32).max(opts.body_side);
    want.clamp(opts.body_side, cap)
}

/// The faces to parse: the given ones, or (`find`) the body model's faces; largest first, at most `max_people`.
fn faces_of(given: &[FaceHint], find: bool, map: &body::BodyMap, iw: f32, opts: &Options) -> Vec<FaceHint> {
    let mut faces = given.to_vec();
    if find {
        let min = (opts.min_face_px * map.w as f32 / iw).ceil().max(2.0) as usize;
        faces = body::faces(map, min).into_iter().map(|rect| FaceHint { rect, landmarks: None, score: 0.5, person_id: None }).collect();
    }
    faces.sort_by(|a, b| (b.rect.width() * b.rect.height()).total_cmp(&(a.rect.width() * a.rect.height())));
    faces.truncate(opts.max_people);
    faces
}

/// Step times on stderr with `LIGHTCRAFT_PROFILE=1`, as the pipeline prints its own.
struct Profile {
    at: Option<std::time::Instant>,
}

impl Profile {
    fn start() -> Self {
        let on = std::env::var_os("LIGHTCRAFT_PROFILE").is_some_and(|v| v == "1");
        Self { at: on.then(std::time::Instant::now) }
    }

    fn step(&mut self, name: &str) {
        if let Some(t) = self.at {
            eprintln!("parsing: {name} {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
            self.at = Some(std::time::Instant::now());
        }
    }
}

/// The eye line's angle in photo pixels (0 without landmarks or for a face turned more than 60°).
pub(crate) fn eye_angle(f: &FaceHint, iw: f32, ih: f32) -> f32 {
    let Some(l) = f.landmarks else { return 0.0 };
    // the subject's right eye is on the left of the picture
    let ((x0, y0), (x1, y1)) = (l[0], l[1]);
    let a = ((y1 - y0) * ih).atan2((x1 - x0) * iw);
    if a.is_finite() && a.abs() < 1.05 { a } else { 0.0 }
}

/// Keep a plane that holds something, trimmed to where it does.
fn add(p: &mut Person, part: Part, plane: Plane) {
    if let Some(t) = trim(plane) {
        p.parts.entry(part).or_default().push(t);
    }
}

/// The plane cut to the box of its values above 1/255 (with a pixel around), or `None` when it is (nearly) empty.
pub fn trim(p: Plane) -> Option<Plane> {
    if !p.is_valid() || p.peak() < 0.05 {
        return None;
    }
    let (w, h) = (p.width, p.height);
    let mut b = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, &v) in p.data.iter().enumerate() {
        if v > 1.0 / 255.0 {
            let (x, y) = (i % w, i / w);
            b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1));
        }
    }
    let (x0, y0) = (b.0.saturating_sub(1), b.1.saturating_sub(1));
    let (x1, y1) = ((b.2 + 1).min(w), (b.3 + 1).min(h));
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    if (x0, y0, x1, y1) == (0, 0, w, h) {
        return Some(p);
    }
    let (nw, nh) = (x1 - x0, y1 - y0);
    let mut data = Vec::with_capacity(nw * nh);
    for y in y0..y1 {
        data.extend_from_slice(p.data.get(y * w + x0..y * w + x1).unwrap_or(&[]));
    }
    let r = p.rect;
    let fx = |x: usize| r.x0 + x as f32 / w as f32 * r.width();
    let fy = |y: usize| r.y0 + y as f32 / h as f32 * r.height();
    let rect = NormRect::new(fx(x0), fy(y0), fx(x1), fy(y1));
    let t = Plane { rect, width: nw, height: nh, data, split: p.split };
    t.is_valid().then_some(t)
}

/// Each person's parts and their areas (for test messages).
#[cfg(test)]
pub fn summary(p: &Parsing) -> Vec<std::collections::BTreeMap<Part, f32>> {
    p.people.iter().map(|q| q.parts.keys().map(|&k| (k, q.area(k))).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimming_keeps_the_values_and_their_place() {
        let mut data = vec![0.0; 100];
        data[5 * 10 + 5] = 1.0;
        let p = Plane { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), width: 10, height: 10, data, split: false };
        let t = trim(p.clone()).unwrap();
        assert_eq!((t.width, t.height), (3, 3));
        assert!((t.sample(0.55, 0.55) - p.sample(0.55, 0.55)).abs() < 1e-5);
        assert!(trim(Plane { data: vec![0.01; 100], ..p.clone() }).is_none());
        assert!(trim(Plane { data: vec![1.0; 3], ..p }).is_none());
    }

    #[test]
    fn candle_parses_faces_one_a_call_on_the_gpu_only() {
        assert_eq!(candle_face_calls(&Device::Cpu, 4), 4);
        assert_eq!(candle_face_calls(&Device::Cpu, 0), 1);
        #[cfg(target_os = "macos")]
        if let Ok(gpu) = Device::new_metal(0) {
            assert_eq!(candle_face_calls(&gpu, 8), 1);
        }
    }

    #[test]
    fn the_eye_angle_follows_the_eyes_and_ignores_nonsense() {
        let hint = |l: Option<[(f32, f32); 5]>| FaceHint { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), landmarks: l, score: 1.0, person_id: None };
        assert_eq!(eye_angle(&hint(None), 100.0, 100.0), 0.0);
        let tilted = hint(Some([(0.4, 0.4), (0.6, 0.5), (0.5, 0.6), (0.4, 0.7), (0.6, 0.7)]));
        assert!((eye_angle(&tilted, 100.0, 100.0) - 0.4636).abs() < 1e-3);
        let nan = hint(Some([(f32::NAN, 0.4), (0.6, 0.5), (0.5, 0.6), (0.4, 0.7), (0.6, 0.7)]));
        assert_eq!(eye_angle(&nan, 100.0, 100.0), 0.0);
    }
}
