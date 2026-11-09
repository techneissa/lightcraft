//! The SegFormer set's two parsers through Apple's Core ML on the GPU (issue #105, `fork/coreml.md`): the face parser
//! (SegFormer-B0) and the clothes parser (SegFormer-B3), converted once by `tools/convert-coreml-parsing.py`.
//!
//! The converted models live next to the candle weights in the models folder: `<models>/<folder>/coreml/<folder>.mlpackage`,
//! compiled once for this Mac into `<folder>.mlmodelc` beside it (again when the package changes). candle stays the
//! reference and the fallback: [`Runtime::Automatic`] uses Core ML when its model is there and loads, and candle for
//! good once Core ML fails on a photo; [`Runtime::CoreMl`] makes either an error.
//!
//! The face parser takes up to [`FACE_BATCH`] crops per call and gives the ten face groups of `face.rs` at 512² (the
//! resize, softmax and group sums are inside the model). The clothes parser takes the photo's grid in the top-left of
//! one of [`BODY_CANVASES`] with a fourth channel marking where the photo is, and gives the class probabilities over
//! the canvas; the model ignores the padding, so the photo's part equals candle's run on the photo alone.

use std::path::{Path, PathBuf};

pub use lightcraft_coreml::Runtime;
use lightcraft_coreml::{ComputeUnits, Error, Model};

use crate::models::{BODY_SEGFORMER_B3, FACE_SEGFORMER_B0, ModelFile};

/// The Core ML files' folder inside a model's folder.
pub const COREML_DIR: &str = "coreml";
const INPUT: &str = "pixels";
const FACE_OUTPUT: &str = "groups";
const BODY_OUTPUT: &str = "probs";
/// Most face crops per call (the converted face parser takes 1 to 8).
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
pub const CONVERTED: [ModelFile; 2] = [FACE_SEGFORMER_B0, BODY_SEGFORMER_B3];

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

/// The smallest canvas that holds a `w × h` grid.
pub(crate) fn canvas(w: usize, h: usize) -> Option<(usize, usize)> {
    BODY_CANVASES.iter().copied().filter(|&(ch, cw)| ch >= h && cw >= w).min_by_key(|&(ch, cw)| ch * cw)
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
    let (ch, cw) = canvas(w, h).ok_or_else(|| Error::Input(format!("no canvas holds a {w} × {h} grid")))?;
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
    fn every_body_grid_has_a_canvas_and_gets_the_smallest() {
        for long in (768..=1536).step_by(32) {
            for short in (32..=long).step_by(32) {
                for (w, h) in [(long, short), (short, long)] {
                    let (ch, cw) = canvas(w, h).unwrap_or_else(|| panic!("{w} × {h}"));
                    assert!(ch >= h && cw >= w && ch % 32 == 0 && cw % 32 == 0);
                }
            }
        }
        assert_eq!(canvas(768, 512), Some((512, 768)));
        assert_eq!(canvas(512, 768), Some((768, 512)));
        assert_eq!(canvas(1536, 1024), Some((1024, 1536)));
        assert_eq!(canvas(1024, 672), Some((704, 1024)));
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
