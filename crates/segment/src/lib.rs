//! SAM 3 (Segment Anything with Concepts, Meta, 2025) inference in pure Rust on candle.
//!
//! - **Point prompts** ([`Sam3::segment_clicks`]): positive and negative clicks select one
//!   object, via SAM 3's interactive (SAM 2 style) prompt encoder and mask decoder.
//! - **Text prompts** ([`Sam3::segment_text`]): a noun phrase ("the red car", "sky") selects
//!   every instance of that concept, via the CLIP text encoder and the DETR detector.
//!
//! The expensive part, the 32-layer ViT over the 1008 × 1008 input, runs once per image
//! ([`Sam3::encode`]); each click or prompt then reuses the [`Encoded`] features.
//!
//! On macOS the model runs on the GPU through Metal; elsewhere (for now) on the CPU. On a Mac
//! with its Core ML conversion installed it runs through Core ML ([`coreml`], issue #268), candle
//! staying the reference and the fallback ([`Sam3::load_with`]). The weights are not part of LightCraft: they are the `facebook/sam3` checkpoint (SAM License),
//! read from a directory the user downloads them to ([`Sam3::load`]).
//!
//! Ported from the Hugging Face `transformers` implementation (Apache-2.0); see `NOTICE`.
//! This crate is therefore licensed under the Apache License 2.0 only (not the MIT option of
//! the rest of LightCraft). No UI dependencies (L3).
//!
//! [`fetch`] downloads the model, only when the user asks for it (from configurable mirrors,
//! verified before use). Nothing in LightCraft requires the model: without it, AI masks report
//! that it isn't installed and everything else works.
//!
//! Modified work (Apache License 2.0, §4(b)): ported by the LightCraft contributors in 2026 from
//! the Python/PyTorch SAM 3 code of Hugging Face Transformers (`src/transformers/models/sam3` and `sam3_tracker`), Copyright The HuggingFace
//! Team and Meta Platforms, Inc.; translated to Rust on candle and restructured. See NOTICE.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
mod clip;
#[cfg(not(target_arch = "wasm32"))]
pub mod coreml;
#[cfg(not(target_arch = "wasm32"))]
mod detector;
#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
pub mod mask;
#[cfg(not(target_arch = "wasm32"))]
mod neck;
#[cfg(not(target_arch = "wasm32"))]
mod nn;
#[cfg(not(target_arch = "wasm32"))]
pub mod remote;
pub mod tokenizer;
#[cfg(not(target_arch = "wasm32"))]
mod tracker;
#[cfg(not(target_arch = "wasm32"))]
mod vit;
#[cfg(not(target_arch = "wasm32"))]
mod weights;
pub mod whole;

use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
pub use candle_core::Device;
#[cfg(not(target_arch = "wasm32"))]
use candle_core::{DType, Tensor};
#[cfg(not(target_arch = "wasm32"))]
use candle_nn::VarBuilder;

#[cfg(not(target_arch = "wasm32"))]
pub use lightcraft_coreml::Runtime;
#[cfg(not(target_arch = "wasm32"))]
pub use tracker::Click;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SAM 3 model: {0}")]
    Model(String),
    #[error("SAM 3 model files not found in {0} (expected model.safetensors, vocab.json, merges.txt)")]
    Missing(PathBuf),
    #[error("SAM 3 cancelled")]
    Cancelled,
    /// Core ML failed and the runtime is Core ML only (on Automatic candle takes over instead).
    #[error("SAM 3 on Core ML: {0}")]
    CoreMl(String),
    #[cfg(not(target_arch = "wasm32"))]
    #[error(transparent)]
    Candle(#[from] candle_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Side of the square model input.
pub const INPUT: usize = 1008;
/// Side of the low-resolution masks the model predicts.
pub const MASK_SIDE: usize = 288;
/// Most clicks one Object selection may carry (the engine's `MAX_CLICKS`).
pub const MAX_CLICKS: usize = 64;
/// (fork: the Core ML decoder's click slots hold every click the engine allows, so a selection never falls back to
/// candle, or fails, for its number of clicks)
#[cfg(not(target_arch = "wasm32"))]
const _: () = assert!(coreml::MAX_CLICKS >= MAX_CLICKS);
#[cfg(not(target_arch = "wasm32"))]
const _: () = assert!(MASK_SIDE == tracker::LOW_RES && INPUT == vit::IMAGE);

/// What a photo's subject usually is, as concept prompts ([`Sam3::segment_subject`]); anything else
/// (an object, a landscape's feature) is selected at the focus point instead.
pub const SUBJECT_CONCEPTS: &[&str] = &["person", "animal"];

/// The files of a model directory.
pub const WEIGHTS_FILE: &str = "model.safetensors";

/// Whether `dir` holds a usable checkpoint.
pub fn is_model_dir(dir: &Path) -> bool {
    dir.join(WEIGHTS_FILE).is_file() && dir.join("vocab.json").is_file() && dir.join("merges.txt").is_file()
}

#[cfg(not(target_arch = "wasm32"))]
/// Whether [`best_device`] can be a GPU in this build (Metal on macOS). Where it can, a caller that
/// sees the model fail there can load it again with [`Sam3::load_cpu`].
pub const GPU_BUILD: bool = cfg!(target_os = "macos");

#[cfg(not(target_arch = "wasm32"))]
/// The best device here: Metal on macOS (when a GPU is available), else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

#[cfg(not(target_arch = "wasm32"))]
/// A loaded model. On candle the vision backbone loads with it and the point and text heads on
/// first use; on Core ML candle's parts load only when Core ML fails and candle takes over.
pub struct Sam3 {
    dir: PathBuf,
    device: Device,
    weights: Arc<weights::Weights>,
    backbone: Option<vit::Vit>,
    tracker_neck: Option<neck::Neck>,
    tracker: Option<tracker::Tracker>,
    detector: Option<detector::Detector>,
    tokenizer: Option<tokenizer::Tokenizer>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Core ML's parts (issue #268); `None`: candle runs everything.
    coreml: Option<Arc<coreml::Parts>>,
    runtime: Runtime,
}

#[cfg(not(target_arch = "wasm32"))]
/// An image run through the backbone, with the head-specific features computed on demand.
pub struct Encoded {
    /// candle's backbone (made from Core ML's numbers when candle takes over).
    backbone: Option<Tensor>,
    tracker: Option<tracker::Features>,
    detector: Option<detector::Features>,
    /// Core ML's numbers: the backbone, then the click and prompt features.
    coreml: Option<CoreMlFeatures>,
}

#[cfg(not(target_arch = "wasm32"))]
struct CoreMlFeatures {
    backbone: Vec<f32>,
    click: Option<coreml::ClickFeatures>,
    text: Option<coreml::TextFeatures>,
}

#[cfg(not(target_arch = "wasm32"))]
/// Runs each Core ML part once so its first real run is fast ([`Sam3::warm_up`]); `Send` and `Sync`, for another
/// thread. Cheap to clone.
#[derive(Clone)]
pub struct WarmUp(Arc<coreml::Parts>);

#[cfg(not(target_arch = "wasm32"))]
impl WarmUp {
    /// Plan the next part not run yet on the GPU (one prediction at most; a part the user is running is skipped);
    /// `false` when none was left.
    pub fn step(&self) -> Result<bool> {
        self.0.warm_next().map_err(|e| Error::CoreMl(e.to_string()))
    }

    /// Whether every part has run once.
    pub fn done(&self) -> bool {
        self.0.warmed()
    }

    /// [`WarmUp::step`] until every part has run or `stop` is raised (checked between parts).
    pub fn run(&self, stop: &std::sync::atomic::AtomicBool) -> Result<()> {
        self.0.warm_up(stop).map_err(|e| Error::CoreMl(e.to_string()))
    }
}

/// A predicted mask: `MASK_SIDE²` logits over the model's square input (which the image was
/// stretched to), and a confidence 0..1.
#[derive(Clone, Debug)]
pub struct Prediction {
    pub logits: Vec<f32>,
    pub score: f32,
}

#[cfg(not(target_arch = "wasm32"))]
impl Sam3 {
    /// Load from a directory holding `model.safetensors` (the `facebook/sam3` checkpoint in
    /// Hugging Face format), `vocab.json` and `merges.txt`.
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_on(dir, best_device())
    }

    /// [`Sam3::load`] on the CPU, whatever GPU there is.
    pub fn load_cpu(dir: &Path) -> Result<Self> {
        Self::load_on(dir, Device::Cpu)
    }

    /// [`Sam3::load`] on candle on `device`.
    pub fn load_on(dir: &Path, device: Device) -> Result<Self> {
        Self::load_with(dir, device, Runtime::Candle)
    }

    /// Load on `runtime`: Automatic is Core ML when its packages are beside `dir` ([`coreml::FOLDER`]) and load, else
    /// candle on `device`; Core ML is an error when they can't load ([`Error::CoreMl`]); Candle is candle on `device`.
    /// candle's weights are always needed: candle takes over when Core ML fails on Automatic.
    pub fn load_with(dir: &Path, device: Device, runtime: Runtime) -> Result<Self> {
        Self::load_with_units(dir, device, runtime, coreml::DEFAULT_UNITS)
    }

    /// [`Sam3::load_with`] with Core ML on `units` (the GPU by default; issue #267 picks them per Mac).
    pub fn load_with_units(dir: &Path, device: Device, runtime: Runtime, units: coreml::ComputeUnits) -> Result<Self> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let weights = weights::Weights::open(&dir.join(WEIGHTS_FILE))?;
        hugging_face_format(&weights)?;
        let coreml = match runtime {
            Runtime::Candle => None,
            _ => match coreml::Parts::load(dir, units) {
                Ok(parts) => Some(Arc::new(parts)),
                Err(e) if runtime == Runtime::CoreMl => return Err(Error::CoreMl(e.to_string())),
                Err(lightcraft_coreml::Error::Missing(_) | lightcraft_coreml::Error::Unavailable) => None,
                Err(e) => {
                    log::warn!("SAM 3 on Core ML did not load ({e}); using candle");
                    None
                }
            },
        };
        let mut model = Self {
            dir: dir.to_path_buf(),
            device,
            weights,
            backbone: None,
            tracker_neck: None,
            tracker: None,
            detector: None,
            tokenizer: None,
            cancel: None,
            coreml,
            runtime,
        };
        if model.coreml.is_none() {
            model.vit()?;
        }
        Ok(model)
    }

    /// The engine the model runs on: `coreml` or `candle`.
    pub fn backend(&self) -> &'static str {
        if self.coreml.is_some() { "coreml" } else { "candle" }
    }

    /// What plans each Core ML part on the GPU before its first real run (1.6–5.6 s each, once per load), to run on
    /// another thread; `None` on candle.
    pub fn warm_up(&self) -> Option<WarmUp> {
        self.coreml.clone().map(WarmUp)
    }

    /// A Core ML failure in `what`: on Core ML only, an error; on Automatic, candle takes this and every later run.
    fn coreml_failed(&mut self, what: &str, e: lightcraft_coreml::Error) -> Result<()> {
        if self.runtime == Runtime::CoreMl {
            return Err(Error::CoreMl(format!("{what}: {e}")));
        }
        log::warn!("SAM 3 on Core ML failed ({what}: {e}); using candle from now on");
        self.coreml = None;
        Ok(())
    }

    /// candle's ViT, loaded when first needed.
    fn vit(&mut self) -> Result<&vit::Vit> {
        if self.backbone.is_none() {
            self.backbone = Some(vit::Vit::new(Self::vb(&self.weights, &self.device).pp("detector_model.vision_encoder.backbone"))?);
        }
        self.backbone.as_ref().ok_or_else(|| Error::Model("the backbone did not load".into()))
    }

    /// candle's backbone of `enc`, made from Core ML's numbers when Core ML encoded it.
    fn candle_backbone(&self, enc: &mut Encoded) -> Result<Tensor> {
        if let Some(b) = &enc.backbone {
            return Ok(b.clone());
        }
        let cm = enc.coreml.as_ref().ok_or_else(|| Error::Model("the photo has no backbone".into()))?;
        let t = Tensor::from_vec(cm.backbone.clone(), (1, 1024, vit::GRID, vit::GRID), &self.device)?;
        enc.backbone = Some(t.clone());
        Ok(t)
    }

    /// Install an optional cancellation flag checked between ViT and DETR layers.
    pub fn set_cancel(&mut self, cancel: Option<Arc<std::sync::atomic::AtomicBool>>) {
        self.cancel = cancel;
    }

    fn check_cancel(&self) -> Result<()> {
        if self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    fn vb<'a>(w: &Arc<weights::Weights>, device: &Device) -> VarBuilder<'a> {
        VarBuilder::from_backend(Box::new(weights::Backend(w.clone())), DType::F32, device.clone())
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Run the backbone on an RGB image (8-bit, row-major, `w × h`), stretched to the square
    /// input like the reference preprocessing.
    pub fn encode(&mut self, rgb: &[u8], w: usize, h: usize) -> Result<Encoded> {
        self.encode_values(mask::preprocess(rgb, w, h, INPUT)?)
    }

    /// Run the backbone on preprocessed pixels `[1, 3, 1008, 1008]` (−1..1, row-major): on Core ML when it is loaded,
    /// else (or when Core ML fails on Automatic) on candle.
    pub fn encode_values(&mut self, pixels: Vec<f32>) -> Result<Encoded> {
        if pixels.len() != 3 * INPUT * INPUT {
            return Err(Error::Model(format!("{} pixel values, not 3 × {INPUT}²", pixels.len())));
        }
        if let Some(parts) = self.coreml.clone() {
            self.check_cancel()?;
            match parts.encode(&pixels) {
                Ok(backbone) => {
                    self.check_cancel()?;
                    let coreml = Some(CoreMlFeatures { backbone, click: None, text: None });
                    return Ok(Encoded { backbone: None, tracker: None, detector: None, coreml });
                }
                Err(e) => self.coreml_failed("encoder", e)?,
            }
        }
        let pixels = Tensor::from_vec(pixels, (1, 3, INPUT, INPUT), &self.device)?;
        self.encode_pixels(&pixels)
    }

    /// Run candle's backbone on preprocessed pixels `[1, 3, 1008, 1008]` (−1..1).
    pub fn encode_pixels(&mut self, pixels: &Tensor) -> Result<Encoded> {
        self.check_cancel()?;
        let pixels = pixels.to_device(&self.device)?;
        let cancel = self.cancel.clone();
        let backbone = self.vit()?.forward_cancellable(&pixels, cancel.as_deref())?;
        Ok(Encoded { backbone: Some(backbone), tracker: None, detector: None, coreml: None })
    }

    fn tracker_parts(&mut self) -> Result<(&neck::Neck, &tracker::Tracker)> {
        if self.tracker_neck.is_none() {
            self.tracker_neck = Some(neck::Neck::new(Self::vb(&self.weights, &self.device).pp("tracker_neck"))?);
        }
        if self.tracker.is_none() {
            self.tracker = Some(tracker::Tracker::new(Self::vb(&self.weights, &self.device).pp("tracker_model"))?);
        }
        match (&self.tracker_neck, &self.tracker) {
            (Some(n), Some(t)) => Ok((n, t)),
            _ => Err(Error::Model("tracker failed to load".into())),
        }
    }

    /// candle's click decoder and its features of `enc`.
    fn candle_tracker<'a>(&'a mut self, enc: &'a mut Encoded) -> Result<(&'a tracker::Tracker, &'a tracker::Features)> {
        let backbone = if enc.tracker.is_none() { Some(self.candle_backbone(enc)?) } else { None };
        let (neck, tracker) = self.tracker_parts()?;
        if let Some(backbone) = backbone {
            let levels = neck.forward(&backbone, 3)?;
            enc.tracker = Some(tracker.features(&levels)?);
        }
        let f = enc.tracker.as_ref().ok_or_else(|| Error::Model("tracker features".into()))?;
        Ok((tracker, f))
    }

    /// Core ML's click features of `enc`, computed when missing; `None` when candle runs the clicks.
    fn coreml_clicks(&mut self, enc: &mut Encoded) -> Result<Option<Arc<coreml::Parts>>> {
        let (Some(parts), Some(cm)) = (self.coreml.clone(), enc.coreml.as_mut()) else { return Ok(None) };
        if cm.click.is_none() {
            self.check_cancel()?;
            match parts.click_features(&cm.backbone) {
                Ok(f) => cm.click = Some(f),
                Err(e) => {
                    self.coreml_failed("click features", e)?;
                    return Ok(None);
                }
            }
        }
        Ok(Some(parts))
    }

    /// The decoder's outputs for `clicks` on Core ML; `None` when candle runs them.
    fn coreml_heads(&mut self, enc: &mut Encoded, clicks: &[Click]) -> Result<Option<tracker::Heads>> {
        if clicks.len() > coreml::MAX_CLICKS && self.coreml.is_some() {
            if self.runtime == Runtime::CoreMl {
                return Err(Error::CoreMl(format!("at most {} clicks at once", coreml::MAX_CLICKS)));
            }
            return Ok(None);
        }
        let Some(parts) = self.coreml_clicks(enc)? else { return Ok(None) };
        let f = enc.coreml.as_ref().and_then(|c| c.click.as_ref()).ok_or_else(|| Error::Model("click features".into()))?;
        self.check_cancel()?;
        match parts.click_heads(f, clicks) {
            Ok(h) => Ok(Some(h)),
            Err(e) => self.coreml_failed("clicks", e).map(|()| None),
        }
    }

    /// Compute the click decoder's features for `enc` now (so the first click is fast).
    pub fn prepare_clicks(&mut self, enc: &mut Encoded) -> Result<()> {
        if self.coreml_clicks(enc)?.is_none() {
            self.candle_tracker(enc)?;
        }
        Ok(())
    }

    /// Segment one object from clicks (positions normalized to the image, 0..1).
    pub fn segment_clicks(&mut self, enc: &mut Encoded, clicks: &[Click]) -> Result<Prediction> {
        if clicks.is_empty() {
            return Err(Error::Model("no clicks".into()));
        }
        // the prompt grows with every click: the editor takes at most this many (the engine's `MAX_CLICKS`), on
        // candle and on Core ML alike (its slots hold at least as many)
        if clicks.len() > MAX_CLICKS {
            return Err(Error::Model(format!("at most {MAX_CLICKS} clicks, not {}", clicks.len())));
        }
        // one click: SAM's three alternatives, best by predicted IoU; more: the single mask
        let multimask = clicks.len() == 1;
        let p = match self.coreml_heads(enc, clicks)? {
            Some(heads) => heads.pick(multimask)?,
            None => {
                let (tracker, f) = self.candle_tracker(enc)?;
                tracker.predict(f, clicks, multimask)?
            }
        };
        Ok(Prediction { logits: p.logits, score: p.iou * sigmoid(p.object_score) })
    }

    /// Segment one object to remove it (fork, `fork/object-remove.md`): one include click takes the alternative that
    /// covers the whole object ([`whole::whole_object`]), not the best scoring part; anything else as
    /// [`Sam3::segment_clicks`] (an exclude click narrows as it always did).
    pub fn segment_object(&mut self, enc: &mut Encoded, clicks: &[Click]) -> Result<Prediction> {
        if !(clicks.len() == 1 && clicks.iter().all(|c| c.positive)) {
            return self.segment_clicks(enc, clicks);
        }
        let p = match self.coreml_heads(enc, clicks)? {
            Some(heads) => heads.whole()?,
            None => {
                let (tracker, f) = self.candle_tracker(enc)?;
                tracker.predict_whole(f, clicks)?
            }
        };
        Ok(Prediction { logits: p.logits, score: p.iou * sigmoid(p.object_score) })
    }

    /// The prompt's token ids and which are real tokens.
    fn tokens(&mut self, text: &str) -> Result<(Vec<u32>, Vec<bool>)> {
        if self.tokenizer.is_none() {
            self.tokenizer = Some(tokenizer::Tokenizer::load(&self.dir)?);
        }
        self.tokenizer.as_ref().map(|t| t.encode(text)).ok_or_else(|| Error::Model("the tokenizer did not load".into()))
    }

    /// The prompt `text` on Core ML: `Some(None)` when nothing scores above `threshold` (no masks are made); `None`
    /// when candle runs it.
    fn coreml_text(&mut self, enc: &mut Encoded, text: &str, threshold: f32, main_only: bool) -> Result<Option<Option<Prediction>>> {
        if self.coreml.is_none() || enc.coreml.is_none() {
            return Ok(None);
        }
        let (ids, valid) = self.tokens(text)?;
        let (Some(parts), Some(cm)) = (self.coreml.clone(), enc.coreml.as_mut()) else { return Ok(None) };
        if cm.text.is_none() {
            self.check_cancel()?;
            match parts.text_features(&cm.backbone) {
                Ok(f) => cm.text = Some(f),
                Err(e) => return self.coreml_failed("prompt features", e).map(|()| None),
            }
        }
        let f = cm.text.as_ref().ok_or_else(|| Error::Model("prompt features".into()))?;
        self.check_cancel()?;
        let scores = match parts.text_score(f, &ids, &valid) {
            Ok(s) => s,
            Err(e) => return self.coreml_failed("prompt", e).map(|()| None),
        };
        if !detector::any_above(&scores.logits, scores.presence, threshold) {
            return Ok(Some(None));
        }
        self.check_cancel()?;
        let masks = match parts.text_masks(f, &scores, &valid) {
            Ok(m) => m,
            Err(e) => return self.coreml_failed("prompt masks", e).map(|()| None),
        };
        let n = MASK_SIDE * MASK_SIDE;
        let one = |i: usize| masks.masks.get(i * n..(i + 1) * n).map(<[f32]>::to_vec);
        Ok(Some(detector::merge_with(&scores.logits, scores.presence, &masks.semantic, threshold, main_only, one)))
    }

    fn detector_parts(&mut self) -> Result<(&detector::Detector, &tokenizer::Tokenizer)> {
        if self.detector.is_none() {
            self.detector = Some(detector::Detector::new(Self::vb(&self.weights, &self.device).pp("detector_model"))?);
        }
        if self.tokenizer.is_none() {
            self.tokenizer = Some(tokenizer::Tokenizer::load(&self.dir)?);
        }
        match (&self.detector, &self.tokenizer) {
            (Some(d), Some(t)) => Ok((d, t)),
            _ => Err(Error::Model("detector failed to load".into())),
        }
    }

    /// Segment every instance of the concept `text` names. Instances scoring above
    /// `threshold` (0..1, 0.5 is the reference default) are merged into one mask; `None` when
    /// nothing matches.
    pub fn segment_text(&mut self, enc: &mut Encoded, text: &str, threshold: f32) -> Result<Option<Prediction>> {
        self.segment_text_with(enc, text, threshold, false)
    }

    /// [`Sam3::segment_text`]; with `main_only`, only the main subject's instances (the largest and any of at
    /// least a third of its area, [`detector::main_instances`]) are merged.
    pub fn segment_text_with(&mut self, enc: &mut Encoded, text: &str, threshold: f32, main_only: bool) -> Result<Option<Prediction>> {
        self.check_cancel()?;
        if let Some(done) = self.coreml_text(enc, text, threshold, main_only)? {
            return Ok(done);
        }
        let cancel = self.cancel.clone();
        let backbone = if enc.detector.is_none() { Some(self.candle_backbone(enc)?) } else { None };
        let (det, tok) = self.detector_parts()?;
        if let Some(backbone) = backbone {
            enc.detector = Some(det.features(&backbone)?);
        }
        let f = enc.detector.as_ref().ok_or_else(|| Error::Model("detector features".into()))?;
        let (ids, valid) = tok.encode(text);
        let Some(out) = det.predict_cancellable(f, &ids, &valid, Some(threshold), cancel.as_deref())? else {
            return Ok(None);
        };
        Ok(detector::merge(&out, threshold, main_only))
    }

    /// The photo's subject among its people and animals ([`detector::subject`]): the ones the
    /// camera's focus (`focus`, normalized; `None` when the file doesn't record it) falls on, or
    /// without it the most prominent, with whoever is about as large. `None` when the subject is
    /// neither (a focus on something else, or nobody in the photo): select what is at the point.
    pub fn segment_subject(&mut self, enc: &mut Encoded, focus: Option<(f32, f32)>, threshold: f32) -> Result<Option<Prediction>> {
        let outs = SUBJECT_CONCEPTS.iter().map(|c| self.detect(enc, c)).collect::<Result<Vec<_>>>()?;
        Ok(detector::subject(&outs, threshold, focus))
    }

    /// Raw detector outputs for `text` (tests and tools).
    pub fn detect(&mut self, enc: &mut Encoded, text: &str) -> Result<detector::Output> {
        let backbone = if enc.detector.is_none() { Some(self.candle_backbone(enc)?) } else { None };
        let (det, tok) = self.detector_parts()?;
        if let Some(backbone) = backbone {
            enc.detector = Some(det.features(&backbone)?);
        }
        let f = enc.detector.as_ref().ok_or_else(|| Error::Model("detector features".into()))?;
        let (ids, valid) = tok.encode(text);
        det.predict(f, &ids, &valid)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Encoded {
    /// candle's backbone output `[1, 1024, 72, 72]` (tests); `None` when Core ML encoded the photo.
    pub fn backbone(&self) -> Option<&Tensor> {
        self.backbone.as_ref()
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Meta's original checkpoint (`sam3.pt`, or a `sam3.safetensors` conversion of it as on community
/// mirrors) names its tensors differently: say so, rather than name the first tensor it lacks (#3378).
fn hugging_face_format(weights: &weights::Weights) -> Result<()> {
    let first = "detector_model.vision_encoder.backbone.embeddings.patch_embeddings.projection.weight";
    if !weights.contains(first) && (weights.has_prefix("detector.backbone.") || weights.has_prefix("tracker.")) {
        return Err(Error::Model(format!(
            "{WEIGHTS_FILE} is Meta's original checkpoint (sam3.pt, or a sam3.safetensors conversion of it as on community mirrors); \
             LightCraft needs the Hugging Face format: {WEIGHTS_FILE} from facebook/sam3"
        )));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(not(target_arch = "wasm32"))]
pub use detector::Output as DetectorOutput;

#[cfg(not(target_arch = "wasm32"))]
/// Wait for queued GPU work to finish (benchmarks).
pub fn sync(model: &Sam3) -> Result<()> {
    Ok(model.device.synchronize()?)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn meta_original_checkpoint_is_named_as_such() {
        // #3378: a community mirror's `sam3.safetensors` (Meta's key names, C64 RoPE tables) saved as model.safetensors
        let dir = std::env::temp_dir().join(format!("lc-sam3-original-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let header = r#"{"detector.backbone.vision_backbone.trunk.blocks.0.attn.freqs_cis":{"dtype":"C64","shape":[2],"data_offsets":[0,16]},"tracker.sam_mask_decoder.iou_token.weight":{"dtype":"F32","shape":[2],"data_offsets":[16,24]}}"#;
        std::fs::rename(weights::tests::write("original", header, 24), dir.join(WEIGHTS_FILE)).unwrap();
        std::fs::write(dir.join("vocab.json"), "{}").unwrap();
        std::fs::write(dir.join("merges.txt"), "").unwrap();
        let err = Sam3::load_cpu(&dir).err().unwrap().to_string();
        assert!(err.contains("Meta's original checkpoint") && err.contains("facebook/sam3"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Core ML that is missing or does not load: Automatic goes on to candle (here candle's own error about the short
    /// checkpoint, not Core ML's), Core ML only is a Core ML error. No panic; runs everywhere (off the Mac Core ML is
    /// unavailable).
    #[test]
    fn missing_or_damaged_core_ml_falls_back_to_candle() {
        let root = std::env::temp_dir().join(format!("lc-sam3-coreml-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("sam3");
        std::fs::create_dir_all(&dir).unwrap();
        let header = r#"{"detector_model.vision_encoder.backbone.embeddings.patch_embeddings.projection.weight":{"dtype":"F32","shape":[2],"data_offsets":[0,8]}}"#;
        std::fs::rename(weights::tests::write("coreml-fallback", header, 8), dir.join(WEIGHTS_FILE)).unwrap();
        std::fs::write(dir.join("vocab.json"), "{}").unwrap();
        std::fs::write(dir.join("merges.txt"), "").unwrap();
        let check = |what: &str| {
            let e = Sam3::load_with(&dir, Device::Cpu, Runtime::Automatic).err().unwrap();
            assert!(!matches!(e, Error::CoreMl(_)), "{what}, Automatic: candle's error, not {e}");
            let e = Sam3::load_with(&dir, Device::Cpu, Runtime::CoreMl).err().unwrap();
            assert!(matches!(e, Error::CoreMl(_)), "{what}, Core ML only: {e}");
        };
        check("no packages");
        // six folders that are not models
        for part in coreml::Part::ALL {
            let (package, _) = coreml::paths(&root.join(coreml::FOLDER), part);
            std::fs::create_dir_all(package.join("Data/com.apple.CoreML")).unwrap();
            std::fs::write(package.join("Manifest.json"), b"not a manifest").unwrap();
        }
        assert!(coreml::installed(&dir));
        check("damaged packages");
        let _ = std::fs::remove_dir_all(root);
    }
}
