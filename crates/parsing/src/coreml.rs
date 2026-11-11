//! The SegFormer set's two parsers and every set's face mesh through Apple's Core ML on the GPU (issue #105,
//! `fork/coreml.md`): the face parser (SegFormer-B0) and the clothes parser (SegFormer-B3), converted once by
//! `tools/convert-coreml-parsing.py`, and MediaPipe's face mesh, by `tools/convert-coreml-facemesh.py`.
//!
//! The converted models live next to the candle weights in the models folder: `<models>/<folder>/coreml/<folder>.mlpackage`,
//! compiled once for this Mac into `<folder>.mlmodelc` beside it (again when the package changes). candle stays the
//! reference and the fallback: [`Runtime::Automatic`] uses Core ML when its model is there and loads, and candle for
//! good once Core ML fails on a photo; [`Runtime::CoreMl`] makes either an error.
//!
//! The face parser takes up to [`FACE_BATCH`] crops per call and gives the ten face groups of `face.rs` at 512² (the
//! resize, softmax and group sums are inside the model). The clothes parser takes the photo's grid in the top-left of
//! one of [`BODY_CANVASES`] with a fourth channel marking where the photo is, and gives the class probabilities over
//! the canvas; the model ignores the padding, so the photo's part equals candle's run on the photo alone. The face mesh
//! takes up to [`FACE_BATCH`] crops of 256² and gives each one's 478 points and face score in one row, in float32.
//!
//! Each model is called with one input shape (issue #200): Core ML re-plans a model on the GPU whenever the shape of
//! its input changes from the call before, about 300 ms of wall and CPU time for the clothes parser and 40 ms a call
//! for the face parser, which is more than the run itself. So the clothes parser always gets the square canvas of its
//! long side ([`canvas`]: a portrait after a landscape, or a 4:3 photo after a 3:2 one, keeps the shape), the face
//! parser [`FACE_BATCH`] crops a call for each full batch and one crop a call for the rest (a photo of a few faces
//! never changes its shape), and the face mesh always [`FACE_BATCH`] crops (the unused ones blank: eight cost less
//! than one at a time).

use std::path::{Path, PathBuf};

pub use lightcraft_coreml::Runtime;
use lightcraft_coreml::{ComputeUnits, Error, Model};

use crate::facemesh::MeshOut;
use crate::models::{BODY_SEGFORMER_B3, FACE_MESH, FACE_SEGFORMER_B0, ModelFile};

/// The Core ML files' folder inside a model's folder.
pub const COREML_DIR: &str = "coreml";
const INPUT: &str = "pixels";
const FACE_OUTPUT: &str = "groups";
const BODY_OUTPUT: &str = "probs";
const MESH_OUTPUT: &str = "mesh";
/// Most face crops per call (the converted face parser and face mesh take 1 to 8).
pub const FACE_BATCH: usize = 8;

/// The canvases (height, width) the converted clothes parser takes, as `tools/convert-coreml-parsing.py`
/// `BODY_CANVASES` lists them: for each long side 768, 1024, 1280, 1536, the square and the short sides ¾, ⅔ and ½
/// of it (up to a multiple of 32), both ways round.
pub const BODY_CANVASES: [(usize, usize); 28] = [
    (768, 768),
    (576, 768),
    (768, 576),
    (512, 768),
    (768, 512),
    (384, 768),
    (768, 384),
    (1024, 1024),
    (768, 1024),
    (1024, 768),
    (704, 1024),
    (1024, 704),
    (512, 1024),
    (1024, 512),
    (1280, 1280),
    (960, 1280),
    (1280, 960),
    (864, 1280),
    (1280, 864),
    (640, 1280),
    (1280, 640),
    (1536, 1536),
    (1152, 1536),
    (1536, 1152),
    (1024, 1536),
    (1536, 1024),
    (768, 1536),
    (1536, 768),
];

/// The model files that have a Core ML version, with the id Settings and `models.runtime` use for each (its folder).
pub const CONVERTED: [ModelFile; 3] = [FACE_SEGFORMER_B0, BODY_SEGFORMER_B3, FACE_MESH];

/// Where `file`'s Core ML package and compiled model are under `models`.
pub fn paths(models: &Path, file: &ModelFile) -> (PathBuf, PathBuf) {
    let dir = models.join(file.folder).join(COREML_DIR);
    (dir.join(format!("{}.mlpackage", file.folder)), dir.join(format!("{}.mlmodelc", file.folder)))
}

/// Whether `file` has a Core ML version and it is under `models` (packaged or compiled; whether it loads is only known
/// by loading it).
pub fn installed(models: &Path, file: &ModelFile) -> bool {
    let (package, compiled) = paths(models, file);
    CONVERTED.contains(file) && (package.is_dir() || compiled.is_dir())
}

/// The canvas a `w × h` grid is run in: the smallest square that holds it, so photos of every shape with the same
/// long side share one input shape (a change of shape costs Core ML more than the run).
pub(crate) fn canvas(w: usize, h: usize) -> Option<(usize, usize)> {
    BODY_CANVASES.iter().copied().filter(|&(ch, cw)| ch == cw && ch >= h && cw >= w).min_by_key(|&(ch, cw)| ch * cw)
}

/// Load `file`'s Core ML model, checked to have the input and output the parser sends and reads.
fn load(models: &Path, file: &ModelFile, output: &str, units: ComputeUnits) -> Result<Model, Error> {
    if !lightcraft_coreml::available() {
        return Err(Error::Unavailable);
    }
    let (package, compiled) = paths(models, file);
    let model = lightcraft_coreml::load_package(&package, &compiled, units)?;
    if !model.inputs().iter().any(|n| n == INPUT) || !model.outputs().iter().any(|n| n == output) {
        return Err(Error::Load(format!(
            "{} is not the converted {} (it takes {:?} and gives {:?})",
            package.display(),
            file.label,
            model.inputs(),
            model.outputs()
        )));
    }
    Ok(model)
}

/// The converted face parser, on the GPU: as fast as the Neural Engine for it (about 11 ms a face on an M4 Pro) and
/// about ten times closer to float32 (the Neural Engine's float16 moved a made-up crop's person mask by 0.4–1.4 %).
pub(crate) fn load_face(models: &Path) -> Result<Model, Error> {
    load(models, &FACE_SEGFORMER_B0, FACE_OUTPUT, ComputeUnits::CpuAndGpu)
}

/// The converted clothes parser, on the GPU: the Neural Engine compiler takes over 15 minutes for one large canvas.
pub(crate) fn load_body(models: &Path) -> Result<Model, Error> {
    load(models, &BODY_SEGFORMER_B3, BODY_OUTPUT, ComputeUnits::CpuAndGpu)
}

/// The converted face mesh, on the GPU in float32: as fast as float16 for this small network and equal to float32
/// PyTorch to 0.001 px (float16 moved points by up to 0.5 px, the Neural Engine by 3.5 px).
pub(crate) fn load_mesh(models: &Path) -> Result<Model, Error> {
    load(models, &FACE_MESH, MESH_OUTPUT, ComputeUnits::CpuAndGpu)
}

/// The face mesh of `n` crops of [`crate::facemesh::SIDE`]² (`n × 3 × side × side`, values 0 to 1), `n` at most
/// [`FACE_BATCH`]. The call always holds [`FACE_BATCH`] crops, the ones past `n` blank, so the model keeps one input
/// shape (each crop gives what it gives alone).
pub(crate) fn meshes(model: &Model, px: &[f32], n: usize) -> Result<Vec<MeshOut>, Error> {
    let side = crate::facemesh::SIDE;
    let row = crate::facemesh::POINTS * 3 + 1;
    let crop = 3 * side * side;
    if n == 0 || n > FACE_BATCH || px.len() != n * crop {
        return Err(Error::Input(format!("{} numbers for {n} face mesh crops", px.len())));
    }
    let mut all = vec![0.0f32; FACE_BATCH * crop];
    if let Some(d) = all.get_mut(..px.len()) {
        d.copy_from_slice(px);
    }
    let out = model.predict(INPUT, &[FACE_BATCH, 3, side, side], &all, MESH_OUTPUT)?;
    if out.shape != [FACE_BATCH, row] || out.data.len() != FACE_BATCH * row {
        return Err(Error::Output(format!("the face mesh gave {:?}, not [{FACE_BATCH}, {row}]", out.shape)));
    }
    out.data.chunks_exact(row).take(n).map(|r| MeshOut::from_row(r).ok_or_else(|| Error::Output("a face mesh row of another size".into()))).collect()
}

/// The face groups of `n` crops of [`crate::face::SIDE`]² (`n × 3 × side × side` normalised pixels), `n` at most
/// [`FACE_BATCH`].
pub(crate) fn face_groups(model: &Model, px: &[f32], n: usize) -> Result<crate::face::Groups, Error> {
    let side = crate::face::SIDE;
    let plane = side * side;
    let groups = crate::face::GROUP_COUNT;
    let out = model.predict(INPUT, &[n, 3, side, side], px, FACE_OUTPUT)?;
    if out.shape != [n, groups, side, side] || out.data.len() != n * groups * plane {
        return Err(Error::Output(format!("the face parser gave {:?}, not [{n}, {groups}, {side}, {side}]", out.shape)));
    }
    Ok(out.data.chunks_exact(groups * plane).map(|face| face.chunks_exact(plane).map(<[f32]>::to_vec).collect()).collect())
}

/// The clothes parser's class probabilities (`CLASSES × h × w`) for the grid `px` (three `w × h` planes).
pub(crate) fn body_probs(model: &Model, px: &[f32], w: usize, h: usize) -> Result<Vec<f32>, Error> {
    let at = canvas(w, h).ok_or_else(|| Error::Input(format!("no canvas holds a {w} × {h} grid")))?;
    body_probs_in(model, px, w, h, at)
}

/// [`body_probs`] in the canvas `(ch, cw)` (one of [`BODY_CANVASES`] that holds the grid).
pub(crate) fn body_probs_in(model: &Model, px: &[f32], w: usize, h: usize, (ch, cw): (usize, usize)) -> Result<Vec<f32>, Error> {
    if ch < h || cw < w || !BODY_CANVASES.contains(&(ch, cw)) {
        return Err(Error::Input(format!("a {w} × {h} grid does not go in a {cw} × {ch} canvas")));
    }
    let (n, grid) = (ch * cw, w * h);
    if px.len() != 3 * grid {
        return Err(Error::Input(format!("{} numbers for a {w} × {h} grid", px.len())));
    }
    let mut x = vec![0.0f32; 4 * n];
    for y in 0..h {
        for k in 0..3 {
            let (src, dst) = (px.get(k * grid + y * w..k * grid + (y + 1) * w), x.get_mut(k * n + y * cw..k * n + y * cw + w));
            if let (Some(s), Some(d)) = (src, dst) {
                d.copy_from_slice(s);
            }
        }
        if let Some(d) = x.get_mut(3 * n + y * cw..3 * n + y * cw + w) {
            d.fill(1.0);
        }
    }
    let classes = crate::body::CLASSES;
    let out = model.predict(INPUT, &[1, 4, ch, cw], &x, BODY_OUTPUT)?;
    if out.shape != [1, classes, ch, cw] || out.data.len() != classes * n {
        return Err(Error::Output(format!("the clothes parser gave {:?}, not [1, {classes}, {ch}, {cw}]", out.shape)));
    }
    let mut probs = Vec::with_capacity(classes * grid);
    for c in 0..classes {
        for y in 0..h {
            probs.extend_from_slice(out.data.get(c * n + y * cw..c * n + y * cw + w).unwrap_or(&[]));
        }
    }
    Ok(probs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_body_grid_has_a_canvas_and_gets_the_square_of_its_long_side() {
        for long in (768..=1536).step_by(32) {
            for short in (32..=long).step_by(32) {
                for (w, h) in [(long, short), (short, long)] {
                    let (ch, cw) = canvas(w, h).unwrap_or_else(|| panic!("{w} × {h}"));
                    assert!(ch >= h && cw >= w && ch % 32 == 0 && cw % 32 == 0);
                }
            }
        }
        // (one square per long side: landscape, portrait and every aspect share the input shape)
        assert_eq!(canvas(768, 512), Some((768, 768)));
        assert_eq!(canvas(512, 768), Some((768, 768)));
        assert_eq!(canvas(768, 576), Some((768, 768)));
        assert_eq!(canvas(1536, 1024), Some((1536, 1536)));
        assert_eq!(canvas(1024, 672), Some((1024, 1024)));
        assert_eq!(canvas(896, 600), Some((1024, 1024)));
        assert_eq!(canvas(1568, 32), None);
    }

    #[test]
    fn only_converted_files_count_as_installed() {
        let dir = crate::tests::scratch("coreml-installed");
        assert!(!installed(&dir, &FACE_SEGFORMER_B0));
        let (package, _) = paths(&dir, &FACE_SEGFORMER_B0);
        std::fs::create_dir_all(&package).unwrap();
        assert!(installed(&dir, &FACE_SEGFORMER_B0));
        let other = crate::models::FACE_BISENET;
        let (package, _) = paths(&dir, &other);
        std::fs::create_dir_all(&package).unwrap();
        assert!(!installed(&dir, &other));
    }
}
