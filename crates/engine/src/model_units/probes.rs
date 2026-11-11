//! Each model's measurement (issue #267): its Core ML call ([`lightcraft_coreml::Sample`]), its candle run where candle
//! gives the same numbers from the same input, its unit of old, and how close an option's output must be to the
//! reference's. The worker that measures, and the `plan` every load asks, are in `worker.rs`.
//!
//! Tolerances, on the made-up input (`Sample::input_data`):
//! - Learn My Edits' image model (DINOv2): cosine of the features at least 0.995 (its tests hold Core ML to 0.999 of
//!   candle).
//! - People parts, face and clothes parsers: mean probability difference at most 0.005 (their tests' bound).
//! - The face mesh: points within 0.5 px (float16 moved them by up to 0.5 px, the Neural Engine by 3.5 px).
//! - Sapiens2's backbone: mean difference at most 2 % of the mean size of its tokens.
//! - BiRefNet and BiRefNet Matting: mean matte difference at most 0.005 after the sigmoid (their tests' bound).
//!
//! Candle is measured for DINOv2 and the two BiRefNets, whose candle ports take the same input as Core ML; the
//! people-parts parsers prepare their input in another way for candle (canvas, crops), so there candle stays the
//! unmeasured fallback, 5–8× slower than Core ML on the GPU when it was last timed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_coreml::{ComputeUnits, Sample};

use super::Unit;
use super::measure::{Probe, Runner, max_abs, mean_abs};
pub use super::worker::{attach, measure_again, measure_now, plan};

/// How close an option's output must be to the reference's.
#[derive(Clone, Copy, Debug)]
pub enum Tolerance {
    /// Cosine at least this.
    Cosine(f32),
    /// Mean |difference| at most this.
    MeanAbs(f32),
    /// Largest |difference| at most this.
    MaxAbs(f32),
    /// Mean |difference| at most this share of the reference's mean |value|.
    RelMean(f32),
    /// Mean |difference| after the sigmoid at most this (logits).
    SigmoidMean(f32),
}

impl Tolerance {
    pub fn matches(self, reference: &[f32], output: &[f32]) -> bool {
        if reference.len() != output.len() || reference.is_empty() {
            return false;
        }
        match self {
            Tolerance::Cosine(min) => {
                let dot: f32 = reference.iter().zip(output).map(|(a, b)| a * b).sum();
                let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
                let n = norm(reference) * norm(output);
                n > 0.0 && dot / n >= min
            }
            Tolerance::MeanAbs(max) => mean_abs(reference, output) <= max,
            Tolerance::MaxAbs(max) => max_abs(reference, output) <= max,
            Tolerance::RelMean(share) => {
                let size = reference.iter().map(|v| v.abs()).sum::<f32>() / reference.len() as f32;
                mean_abs(reference, output) <= share * size.max(f32::EPSILON)
            }
            Tolerance::SigmoidMean(max) => {
                let sig = |v: &[f32]| v.iter().map(|x| 1.0 / (1.0 + (-x).exp())).collect::<Vec<f32>>();
                mean_abs(&sig(reference), &sig(output)) <= max
            }
        }
    }
}

/// Loads candle's port from a model folder: a function from the made-up input to the output Core ML gives.
type CandleRun = Box<dyn FnMut(&[f32]) -> Result<Vec<f32>, String>>;
type CandleLoad = fn(&Path) -> Result<CandleRun, String>;

/// One model's measurement.
pub struct Spec {
    /// The model's id (`model_runtime::MODELS`).
    pub model: &'static str,
    pub sample: Sample,
    /// Its unit of old, the reference.
    pub default: ComputeUnits,
    pub tolerance: Tolerance,
    /// The folder candle loads from, and how.
    candle: Option<(PathBuf, CandleLoad)>,
}

impl Spec {
    /// Whether the Core ML version is here and this Mac can run it.
    pub fn coreml_ready(&self) -> bool {
        lightcraft_coreml::available() && (self.sample.package.is_dir() || self.sample.compiled.is_dir())
    }

    /// The model files' identity: the package's key (its manifest and its files' sizes and times), else the compiled
    /// copy's, and candle's weights file's size and time when candle is measured.
    fn fingerprint(&self) -> String {
        let coreml = lightcraft_coreml::package_key(&self.sample.package).ok().unwrap_or_else(|| {
            let mut key = self.sample.compiled.as_os_str().to_owned();
            key.push(".key");
            std::fs::read_to_string(PathBuf::from(key)).unwrap_or_else(|_| "compiled".into())
        });
        let candle = self.candle.as_ref().and_then(|(dir, _)| weights_stamp(dir)).unwrap_or_default();
        format!("{coreml}{candle}")
    }

    fn shape(&self) -> String {
        let dims: Vec<String> = self.sample.shape.iter().map(usize::to_string).collect();
        format!("{}{}", dims.join("x"), self.sample.function.as_deref().map(|f| format!(":{f}")).unwrap_or_default())
    }

    /// What a measurement of these files on this Mac is filed under.
    pub fn key(&self) -> String {
        super::key(&self.fingerprint(), &self.shape())
    }
}

fn weights_stamp(dir: &Path) -> Option<String> {
    let meta = std::fs::metadata(dir.join("model.safetensors")).ok()?;
    let time = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(format!("+{}-{time}", meta.len()))
}

fn has_metal() -> bool {
    static METAL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *METAL.get_or_init(|| lightcraft_parsing::best_device().is_metal())
}

/// Learn My Edits' image model in its folder `dir`.
pub fn learn(dir: &Path) -> Spec {
    let candle: CandleLoad = |dir| {
        let enc = lightcraft_learn::encoder::Encoder::load(&dir.join(lightcraft_learn::ENCODER_FILE)).map_err(|e| e.to_string())?;
        Ok(Box::new(move |x: &[f32]| enc.run(x.to_vec()).map_err(|e| e.to_string())))
    };
    Spec {
        model: lightcraft_learn::ENCODER_ID,
        sample: lightcraft_learn::image_encoder::coreml_sample(dir),
        default: lightcraft_learn::image_encoder::DEFAULT_UNITS,
        tolerance: Tolerance::Cosine(0.995),
        candle: Some((dir.to_path_buf(), candle)),
    }
}

/// The people-parts model `model` (its folder's name) in the models folder `models`.
pub fn parsing(models: &Path, model: &str) -> Option<Spec> {
    use lightcraft_parsing::models::{BODY_SAPIENS2, FACE_MESH, FACE_SEGFORMER_B0};
    let file = lightcraft_parsing::coreml::CONVERTED.into_iter().find(|f| f.folder == model)?;
    let tolerance = match file {
        f if f == FACE_MESH => Tolerance::MaxAbs(0.5),
        f if f == BODY_SAPIENS2 => Tolerance::RelMean(0.02),
        f if f == FACE_SEGFORMER_B0 => Tolerance::MeanAbs(0.005),
        _ => Tolerance::MeanAbs(0.005),
    };
    let id = crate::people_parsing::COREML_MODELS.into_iter().find(|m| *m == model)?;
    Some(Spec {
        model: id,
        sample: lightcraft_parsing::coreml::sample(models, &file)?,
        default: lightcraft_parsing::coreml::DEFAULT_UNITS,
        tolerance,
        candle: None,
    })
}

/// BiRefNet (`matting` false) or BiRefNet Matting in its folder `dir`.
#[cfg(feature = "sam")]
pub fn birefnet(dir: &Path, matting: bool) -> Spec {
    use lightcraft_birefnet::Variant;
    let (variant, model) =
        if matting { (Variant::Matting, crate::hair_edges::MODEL_DIR) } else { (Variant::General, crate::model_runtime::BIREFNET) };
    let general: CandleLoad = |dir| candle_birefnet(dir, Variant::General);
    let matte: CandleLoad = |dir| candle_birefnet(dir, Variant::Matting);
    Spec {
        model,
        sample: lightcraft_birefnet::coreml::sample(dir, variant),
        default: lightcraft_birefnet::coreml::DEFAULT_UNITS,
        tolerance: Tolerance::SigmoidMean(0.005),
        candle: Some((dir.to_path_buf(), if matting { matte } else { general })),
    }
}

#[cfg(feature = "sam")]
fn candle_birefnet(dir: &Path, variant: lightcraft_birefnet::Variant) -> Result<CandleRun, String> {
    let mut net = lightcraft_birefnet::BiRefNet::load_with(dir, variant, lightcraft_coreml::Runtime::Candle, lightcraft_birefnet::best_device())
        .map_err(|e| e.to_string())?;
    Ok(Box::new(move |x: &[f32]| net.matte_pixels(x.to_vec()).map(|m| m.logits).map_err(|e| e.to_string())))
}

/// A [`Spec`] as the measurement runs it, with its made-up input.
pub(super) struct ModelProbe {
    pub(super) spec: Spec,
    input: Vec<f32>,
}

impl ModelProbe {
    pub(super) fn new(spec: Spec) -> Result<Arc<Self>, String> {
        let input = spec.sample.input_data().map_err(|e| e.to_string())?;
        Ok(Arc::new(Self { spec, input }))
    }

    /// Whether its Core ML package is a large one ([`super::LARGE_PACKAGE`]).
    pub(super) fn large(&self) -> bool {
        dir_bytes(&self.spec.sample.package, 0) >= super::LARGE_PACKAGE
    }
}

/// The bytes of the files under `path` (a package: a few folders deep, a few files).
fn dir_bytes(path: &Path, depth: usize) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    if depth > 8 {
        return 0;
    }
    std::fs::read_dir(path)
        .map(|entries| entries.flatten().take(1024).map(|e| dir_bytes(&e.path(), depth + 1)).fold(0u64, u64::saturating_add))
        .unwrap_or(0)
}

/// The probe, shared with the runners it makes (each lives on the measuring thread).
pub(super) struct SharedProbe(pub(super) Arc<ModelProbe>);

struct CoreMlRunner {
    model: lightcraft_coreml::Model,
    probe: Arc<ModelProbe>,
}

impl Runner for CoreMlRunner {
    fn run(&mut self) -> Result<Vec<f32>, String> {
        let _gpu = crate::gpu_run::turn_for("measuring a model");
        self.probe.spec.sample.run(&self.model, &self.probe.input).map_err(|e| e.to_string())
    }
}

struct CandleRunner {
    run: CandleRun,
    probe: Arc<ModelProbe>,
}

impl Runner for CandleRunner {
    fn run(&mut self) -> Result<Vec<f32>, String> {
        let _gpu = crate::gpu_run::turn_for("measuring a model");
        (self.run)(&self.probe.input)
    }
}

impl Probe for SharedProbe {
    /// The reference, then the others that load quickly (the GPU, candle), then the Neural Engine ones (which may
    /// wait for later).
    fn options(&self) -> Vec<Unit> {
        let spec = &self.0.spec;
        let reference = Unit::coreml(spec.default);
        let others: Vec<Unit> = ComputeUnits::MEASURED.into_iter().map(Unit::coreml).filter(|u| *u != reference).collect();
        let mut v = vec![reference];
        v.extend(others.iter().copied().filter(|u| !u.neural_engine()));
        if spec.candle.is_some() && has_metal() {
            v.push(Unit::CandleGpu);
        }
        v.extend(others.iter().copied().filter(|u| u.neural_engine()));
        v
    }

    fn load(&self, unit: Unit) -> Result<Box<dyn Runner>, String> {
        let probe = self.0.clone();
        Ok(match unit.compute_units() {
            Some(units) => Box::new(CoreMlRunner { model: probe.spec.sample.load(units).map_err(|e| e.to_string())?, probe }),
            None => {
                let (dir, load) = probe.spec.candle.as_ref().ok_or("no candle port to measure")?;
                let run = load(dir)?;
                Box::new(CandleRunner { run, probe })
            }
        })
    }

    fn matches(&self, reference: &[f32], output: &[f32]) -> bool {
        self.0.spec.tolerance.matches(reference, output)
    }
}
