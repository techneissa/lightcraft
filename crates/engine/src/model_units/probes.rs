//! Each model's measurement (issue #267): its Core ML call ([`lightcraft_coreml::Sample`]), its candle run where candle
//! gives the same numbers from the same input, its unit of old, and how close an option's output must be to the
//! reference's. Also the background worker that measures, and [`plan`], which every load asks.
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
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};

use lightcraft_coreml::{ComputeUnits, Runtime, Sample};

use super::measure::{self, Probe, Runner, Wall, max_abs, mean_abs};
use super::{LOAD_CAP, Plan, Unit, shared};
use crate::activity::{Activity, Cancel};
use crate::prep::Foreground;

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
    let mut net =
        lightcraft_birefnet::BiRefNet::load_with(dir, variant, Runtime::Candle, lightcraft_birefnet::best_device()).map_err(|e| e.to_string())?;
    Ok(Box::new(move |x: &[f32]| net.matte_pixels(x.to_vec()).map(|m| m.logits).map_err(|e| e.to_string())))
}

/// A [`Spec`] as the measurement runs it, with its made-up input.
struct ModelProbe {
    spec: Spec,
    input: Vec<f32>,
}

/// The probe, shared with the runners it makes (each lives on the measuring thread).
struct SharedProbe(Arc<ModelProbe>);

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
    fn options(&self) -> Vec<Unit> {
        let spec = &self.0.spec;
        let mut v = vec![Unit::coreml(spec.default)];
        v.extend(ComputeUnits::MEASURED.into_iter().filter(|u| *u != spec.default).map(Unit::coreml));
        if spec.candle.is_some() && has_metal() {
            v.push(Unit::CandleGpu);
        }
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

/// A model to measure for `key`.
struct Job {
    spec: Spec,
    key: String,
}

/// The thread that measures, one model at a time.
pub(crate) struct Worker {
    tx: mpsc::Sender<Job>,
}

/// Measure from now on, with rows in `activity` and waiting while `foreground` says the photo on screen is being made
/// (the app and MCP hosts do this; a bare session, as in tests, never measures). Once per process.
pub fn attach(activity: Activity, foreground: Foreground) {
    let mut worker = super::lock(&shared().worker);
    if worker.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new().name("model-units".into()).spawn(move || {
        for job in rx {
            let model = job.spec.model;
            run(job, &activity, &foreground);
            shared().end(model);
        }
    });
    match spawned {
        Ok(_) => *worker = Some(Worker { tx }),
        Err(e) => log::warn!("model units: no measuring thread ({e})"),
    }
}

fn run(job: Job, activity: &Activity, foreground: &Foreground) {
    let model = job.spec.model;
    let feature = crate::model_runtime::MODELS.iter().find(|(id, _)| *id == model).map_or(model, |(_, f)| f);
    let cancel = Arc::new(AtomicBool::new(false));
    let _row = activity.start_background("modelUnits", &format!("Finding the fastest unit for {feature}"), Cancel::Flag(cancel.clone()));
    let wait = || foreground.wait(&cancel) || lightcraft_gpu::shutting_down();
    let t = web_time::Instant::now();
    match measure_now(job.spec, &wait) {
        Ok(Some(options)) => {
            log::info!("model units: {model} measured in {:?}: {options:?}", t.elapsed());
            shared().store(model, &job.key, options);
        }
        Ok(None) => log::info!("model units: measuring {model} was stopped"),
        Err(e) => log::warn!("model units: {model}: {e}"),
    }
}

/// Measure `spec`'s model now, on this thread, with the real clock ([`measure::measure`]; `wait` as there). Nothing
/// is filed (the `model_units` example prints it).
pub fn measure_now(spec: Spec, wait: &dyn Fn() -> bool) -> Result<Option<Vec<super::Timing>>, String> {
    let input = spec.sample.input_data().map_err(|e| e.to_string())?;
    let probe: Arc<dyn Probe> = Arc::new(SharedProbe(Arc::new(ModelProbe { spec, input })));
    Ok(measure::measure(probe, Arc::new(Wall::default()), LOAD_CAP, wait))
}

/// Queue `spec` for measurement unless it is queued already or nothing measures.
fn request(spec: Spec, key: String) -> bool {
    let units = shared();
    let model = spec.model;
    if !units.begin(model) {
        return true;
    }
    let sent = super::lock(&units.worker).as_ref().is_some_and(|w| w.tx.send(Job { spec, key }).is_ok());
    if !sent {
        units.end(model);
    }
    sent
}

/// How a load of `spec`'s model on `runtime` (the Settings choice) goes: the measured order, or the unit of old while
/// the model has not been measured for its files on this Mac, which queues its measurement.
pub fn plan(spec: Spec, runtime: Runtime) -> Plan {
    let old = Plan::default_for(spec.default);
    if runtime == Runtime::Candle || !spec.coreml_ready() {
        return old;
    }
    let key = spec.key();
    let units = shared();
    let plan = Plan::from_order(&units.order(spec.model, &key, Unit::coreml(spec.default)), spec.default);
    if !units.measured(spec.model, &key) {
        request(spec, key);
    }
    // (Core ML only, as Settings asked: no candle first)
    if runtime == Runtime::CoreMl { Plan { candle_first: false, ..plan } } else { plan }
}

/// Measure `spec`'s model again (Settings' Measure Again, `models.measure`).
pub fn measure_again(spec: Spec) -> Result<(), String> {
    if !spec.coreml_ready() {
        return Err("this model's Core ML version is not installed".into());
    }
    if super::lock(&shared().worker).is_none() {
        return Err("models are measured only in the app".into());
    }
    let key = spec.key();
    shared().forget(spec.model);
    if request(spec, key) { Ok(()) } else { Err("the measuring thread stopped".into()) }
}
