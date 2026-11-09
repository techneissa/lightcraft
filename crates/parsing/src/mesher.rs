//! Fork feature (`fork/ai-culling.md`): the face mesh alone, for faces a detector already found. Assisted Culling
//! reads each face's eyes from its 478 points; it needs no part masks, so it loads and runs only the mesh (Core ML
//! when its converted model is there, else candle), not a whole model set.

use std::path::{Path, PathBuf};

use candle_core::Device;

use crate::coreml::{self, Runtime};
use crate::face::{self, Job};
use crate::facemesh::{FaceMesh, MeshOut};
use crate::image::Crop;
use crate::parser::{Backend, CoreMlResult, Engine, eye_angle, load_mesh_net};
use crate::types::{FaceHint, Landmark};
use crate::{Error, Result};

/// Faces smaller than this many photo pixels across get no mesh (as a parse's default `min_face_px`).
pub const MIN_FACE_PX: f32 = 14.0;
/// Most faces meshed per photo (the largest first).
pub const MAX_FACES: usize = 64;

/// One face's mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    /// The index of the face in the hints handed in.
    pub face: usize,
    /// 478 points in fractions of the photo; empty when the mesh was not sure it saw a face.
    pub landmarks: Vec<Landmark>,
    /// How sure the mesh was that it saw a face (0 to 1).
    pub score: f32,
}

/// The face mesh, loaded.
pub struct Mesher {
    mesh: Engine<FaceMesh>,
    models: PathBuf,
    dev: Device,
}

impl Mesher {
    /// Load the face mesh from the models folder: Core ML as `runtime` asks, candle on `dev` otherwise.
    pub fn load(models: &Path, dev: Device, runtime: Runtime) -> Result<Self> {
        let mesh_coreml = || coreml::load_mesh(models);
        let loader: &dyn Fn() -> CoreMlResult<lightcraft_coreml::Model> = &mesh_coreml;
        let mesh = Engine::load(&crate::models::FACE_MESH, runtime, Some(loader), || load_mesh_net(models, &dev))?;
        Ok(Self { mesh, models: models.to_path_buf(), dev })
    }

    /// The engine the next face runs on.
    pub fn backend(&self) -> Backend {
        self.mesh.backend()
    }

    /// The mesh of each face in `hints` large enough to mesh (at most [`MAX_FACES`], the largest first). `cancelled`
    /// is asked between batches.
    pub fn meshes(&self, img: &crate::RgbImage, hints: &[FaceHint], cancelled: &dyn Fn() -> bool) -> Result<Vec<Mesh>> {
        if img.width.max(img.height) < crate::MIN_SIDE {
            return Ok(Vec::new());
        }
        let (iw, ih) = (img.width as f32, img.height as f32);
        let mut jobs: Vec<Job> = hints
            .iter()
            .enumerate()
            .filter_map(|(k, h)| {
                let rect = h.rect.clamped()?;
                let big = (rect.width() * iw).max(rect.height() * ih) >= MIN_FACE_PX;
                big.then(|| Job { person: k, rect, angle: eye_angle(h, iw, ih) })
            })
            .collect();
        jobs.sort_by(|a, b| (b.rect.width() * b.rect.height()).total_cmp(&(a.rect.width() * a.rect.height())));
        jobs.truncate(MAX_FACES);
        let mut out = Vec::with_capacity(jobs.len());
        for chunk in jobs.chunks(coreml::FACE_BATCH) {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let crops: Vec<Crop> = chunk.iter().map(|j| j.mesh_crop(img)).collect();
            let meshes = self.run(img, &crops)?;
            for ((job, crop), m) in chunk.iter().zip(&crops).zip(&meshes) {
                out.push(Mesh { face: job.person, landmarks: face::landmarks_of(m, crop, img), score: m.presence });
            }
        }
        Ok(out)
    }

    fn run(&self, img: &crate::RgbImage, crops: &[Crop]) -> Result<Vec<MeshOut>> {
        self.mesh.run(
            |m| coreml::meshes(m, &face::batch_pixels(img, crops, false), crops.len()),
            |net| face::mesh_batch(net, img, crops, &self.dev),
            || load_mesh_net(&self.models, &self.dev),
        )
    }
}
