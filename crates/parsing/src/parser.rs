//! Loading a model set and parsing a photo with it.

use std::path::Path;
use std::sync::Arc;

use candle_core::Device;

use crate::body;
use crate::face::{self, FaceNet, Job, Nets};
use crate::facemesh::FaceMesh;
use crate::models::{FaceModel, ModelFile, ModelSet};
use crate::nn::{Layout, Params};
use crate::people;
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
    /// Faces per model call.
    pub batch: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { body_side: 768, max_people: 64, min_face_px: 14.0, batch: 4 }
    }
}

/// A loaded model set.
pub struct Parser {
    set: ModelSet,
    face: FaceNet,
    face_model: FaceModel,
    body: Segformer,
    mesh: FaceMesh,
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

impl Parser {
    /// Load `set` from the models folder on the best device.
    pub fn load(set: ModelSet, models: &Path) -> Result<Self> {
        Self::load_on(set, models, best_device())
    }

    pub fn load_on(set: ModelSet, models: &Path, dev: Device) -> Result<Self> {
        let named = |f: &ModelFile, e: Error| match e {
            Error::Model(m) if !m.starts_with(f.label) => Error::Model(format!("{}: {m}", f.label)),
            other => other,
        };
        let face_file = set.face_file();
        let fw = open(&face_file, models)?;
        let face = match set.face_model() {
            FaceModel::Segformer => {
                FaceNet::Segformer(Segformer::load(&Params::new(fw, dev.clone(), Layout::Oihw)).map_err(|e| named(&face_file, e))?)
            }
            FaceModel::BiSeNet => FaceNet::BiSeNet(Box::new(
                crate::bisenet::BiSeNet::load(&Params::new(fw, dev.clone(), Layout::Ohwi)).map_err(|e| named(&face_file, e))?,
            )),
        };
        let body_file = set.body_file();
        let body = Segformer::load(&Params::new(open(&body_file, models)?, dev.clone(), Layout::Oihw)).map_err(|e| named(&body_file, e))?;
        if body.classes() != body::CLASSES {
            return Err(Error::Model(format!("{}: {} classes, not {}", body_file.label, body.classes(), body::CLASSES)));
        }
        let mesh_file = crate::models::FACE_MESH;
        let mesh = FaceMesh::load(open(&mesh_file, models)?.as_ref(), &dev).map_err(|e| named(&mesh_file, e))?;
        Ok(Self { set, face, face_model: set.face_model(), body, mesh, dev })
    }

    pub fn set(&self) -> ModelSet {
        self.set
    }

    pub fn device(&self) -> &Device {
        &self.dev
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
        let mut map = body::run(&self.body, img, body_side(&given, img, opts), &self.dev)?;
        prof.step("body");
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let faces = faces_of(&given, hints.is_empty(), &map, iw, opts);
        if hints.is_empty() {
            // the faces were found in the map: look again, closer, when they are small
            let side = body_side(&faces, img, opts);
            if side > map.w.max(map.h) {
                map = body::run(&self.body, img, side, &self.dev)?;
                prof.step("body again");
            }
        }
        let rects: Vec<NormRect> = faces.iter().map(|f| f.rect).collect();
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
        let nets = Nets { net: &self.face, model: self.face_model, mesh: &self.mesh, dev: &self.dev };
        let scene = face::Scene { claims: &claims, body_hat: &|x, y| map.at(body::HAT, x, y) };
        for f in face::run(&nets, img, &jobs, opts.batch, &scene, cancelled)? {
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
fn eye_angle(f: &FaceHint, iw: f32, ih: f32) -> f32 {
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
    let t = Plane { rect, width: nw, height: nh, data };
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
        let p = Plane { rect: NormRect::new(0.0, 0.0, 1.0, 1.0), width: 10, height: 10, data };
        let t = trim(p.clone()).unwrap();
        assert_eq!((t.width, t.height), (3, 3));
        assert!((t.sample(0.55, 0.55) - p.sample(0.55, 0.55)).abs() < 1e-5);
        assert!(trim(Plane { data: vec![0.01; 100], ..p.clone() }).is_none());
        assert!(trim(Plane { data: vec![1.0; 3], ..p }).is_none());
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
