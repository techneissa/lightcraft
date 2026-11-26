//! SAM 3 through Apple's Core ML (issue #268, `fork/coreml.md`): seven converted parts on the GPU in float32, candle
//! staying the reference and the fallback.
//!
//! `tools/convert-coreml-sam3.py` converts them once from the weights in the `sam3` folder into the sibling folder
//! [`FOLDER`] (`<models>/sam3-coreml/sam3-<part>.mlpackage`); nothing is ever written into the `sam3` folder. Each
//! package compiles once for this Mac into `sam3-<part>.mlmodelc` beside it. SAM 3's licence is not on LightCraft's
//! allowlist: the packages are made on the Mac that runs them and never published.
//!
//! The parts take and give the tensors candle's port computes at the same steps, so either engine's features feed the
//! same selection code ([`crate::tracker::Heads`], [`crate::detector::merge_with`]):
//!
//! | Part | Runs | Inputs → outputs |
//! | --- | --- | --- |
//! | encoder | once per photo | pixels → backbone |
//! | clickfeat | once per photo | backbone → image, s0, s1 |
//! | clickdec | per click | image, s0, s1, points, labels → masks, iou, obj |
//! | textfeat | first prompt on a photo | backbone → l0, l1, l2 |
//! | textenc | per prompt without stored text features (Describe, a fixed phrase's first run) | ids, mask → text |
//! | textdet | per prompt | l2, text, mask → logits, presence, boxes, queries, memory |
//! | textmask | per prompt that matches | l0, l1, memory, text, mask, queries → masks, semantic |
//!
//! `textenc`, CLIP's text encoder, is most of the prompt model (about 1.4 GB, issue #437): it loads only when a prompt
//! has no stored text features ([`crate::stored`]), is never warmed up, and is let go after a fixed phrase's run. A
//! folder converted before #437 holds `textscore` (`textenc` and `textdet` in one part, giving `text` too) instead:
//! it still works, loading the encoder with every prompt, until `textenc` and `textdet` are converted beside it.
//!
//! Clicks go in [`SLOTS`] fixed slots (one input shape: Core ML plans a model on the GPU again for every new shape);
//! the decoder masks unused slots out of its attention, so a click's mask is the one of the clicks alone. A prompt
//! that matches nothing never runs `textmask`. Every output is checked for its size and for numbers that are not
//! finite: such an output is an error, and on Automatic candle takes the run over.
//!
//! Core ML plans each part on the GPU at its first run after loading (1.6–5.6 s once per launch); [`Parts::warm_next`]
//! runs one part not run yet on made-up inputs, off the thread that runs the user's clicks, never waiting for a run of
//! the user's (the caller runs it while the user is idle and stops between parts when a request comes).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

pub use lightcraft_coreml::ComputeUnits;
use lightcraft_coreml::{Error, Model, Output, Tensor};

use crate::tracker::{Click, Heads};
use crate::{INPUT, MASK_SIDE};

/// The Core ML folder: a sibling of the `sam3` model folder in the shared models folder.
pub const FOLDER: &str = "sam3-coreml";
/// Where the parts run unless a caller picks other units: the GPU (the Neural Engine never compiled the encoder,
/// #268). Issue #267 picks the units per Mac.
pub const DEFAULT_UNITS: ComputeUnits = ComputeUnits::CpuAndGpu;
/// Click slots of the decoder: 64 clicks and the "not a point" that closes the list.
pub const SLOTS: usize = 65;
/// Most clicks Core ML takes at once ([`SLOTS`] − 1); more go to candle.
pub const MAX_CLICKS: usize = SLOTS - 1;
const PAD: i32 = -10;
const NOT_A_POINT: i32 = -1;

const GRID: usize = 72;
const HIDDEN: usize = 256;
const QUERIES: usize = 200;
const TOKENS: usize = 32;
const CELLS: usize = MASK_SIDE * MASK_SIDE;
const BACKBONE: [usize; 4] = [1, 1024, GRID, GRID];

/// One of the converted parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Encoder,
    ClickFeat,
    ClickDec,
    TextFeat,
    TextEnc,
    TextDet,
    TextMask,
    /// `textenc` and `textdet` in one part, as converted before issue #437.
    TextScore,
}

impl Part {
    pub const ALL: [Part; 8] =
        [Part::Encoder, Part::ClickFeat, Part::ClickDec, Part::TextFeat, Part::TextEnc, Part::TextDet, Part::TextMask, Part::TextScore];
    /// The parts every conversion holds.
    const BASE: [Part; 5] = [Part::Encoder, Part::ClickFeat, Part::ClickDec, Part::TextFeat, Part::TextMask];

    /// Its name in the package's file name (`sam3-<name>.mlpackage`).
    pub fn name(self) -> &'static str {
        match self {
            Part::Encoder => "encoder",
            Part::ClickFeat => "clickfeat",
            Part::ClickDec => "clickdec",
            Part::TextFeat => "textfeat",
            Part::TextEnc => "textenc",
            Part::TextDet => "textdet",
            Part::TextMask => "textmask",
            Part::TextScore => "textscore",
        }
    }

    fn index(self) -> usize {
        Part::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// Its inputs: (name, shape, whole numbers).
    fn inputs(self) -> &'static [(&'static str, &'static [usize], bool)] {
        const LEVEL2: &[usize] = &[1, HIDDEN, GRID, GRID];
        const IDS: &[usize] = &[1, TOKENS];
        match self {
            Part::Encoder => &[("pixels", &[1, 3, INPUT, INPUT], false)],
            Part::ClickFeat | Part::TextFeat => &[("backbone", &BACKBONE, false)],
            Part::ClickDec => &[
                ("image", LEVEL2, false),
                ("s0", &[1, 32, MASK_SIDE, MASK_SIDE], false),
                ("s1", &[1, 64, MASK_SIDE / 2, MASK_SIDE / 2], false),
                ("points", &[1, 1, SLOTS, 2], false),
                ("labels", &[1, 1, SLOTS], true),
            ],
            Part::TextEnc => &[("ids", IDS, true), ("mask", IDS, true)],
            Part::TextDet => &[("l2", LEVEL2, false), ("text", &[1, TOKENS, HIDDEN], false), ("mask", IDS, true)],
            Part::TextScore => &[("l2", LEVEL2, false), ("ids", IDS, true), ("mask", IDS, true)],
            Part::TextMask => &[
                ("l0", &[1, HIDDEN, MASK_SIDE, MASK_SIDE], false),
                ("l1", &[1, HIDDEN, MASK_SIDE / 2, MASK_SIDE / 2], false),
                ("memory", &[1, GRID * GRID, HIDDEN], false),
                ("text", &[1, TOKENS, HIDDEN], false),
                ("mask", IDS, true),
                ("queries", &[1, QUERIES, HIDDEN], false),
            ],
        }
    }

    /// Its outputs: (name, how many numbers).
    fn outputs(self) -> &'static [(&'static str, usize)] {
        match self {
            Part::Encoder => &[("backbone", 1024 * GRID * GRID)],
            Part::ClickFeat => &[("image", HIDDEN * GRID * GRID), ("s0", 32 * CELLS), ("s1", 64 * CELLS / 4)],
            Part::ClickDec => &[("masks", 4 * CELLS), ("iou", 4), ("obj", 1)],
            Part::TextFeat => &[("l0", HIDDEN * CELLS), ("l1", HIDDEN * CELLS / 4), ("l2", HIDDEN * GRID * GRID)],
            Part::TextEnc => &[("text", TOKENS * HIDDEN)],
            Part::TextDet => {
                &[("logits", QUERIES), ("presence", 1), ("boxes", QUERIES * 4), ("queries", QUERIES * HIDDEN), ("memory", GRID * GRID * HIDDEN)]
            }
            Part::TextScore => &[
                ("logits", QUERIES),
                ("presence", 1),
                ("boxes", QUERIES * 4),
                ("queries", QUERIES * HIDDEN),
                ("memory", GRID * GRID * HIDDEN),
                ("text", TOKENS * HIDDEN),
            ],
            Part::TextMask => &[("masks", QUERIES * CELLS), ("semantic", CELLS)],
        }
    }
}

/// The Core ML folder of the model folder `model_dir` (its sibling [`FOLDER`]).
pub fn folder(model_dir: &Path) -> Option<PathBuf> {
    model_dir.parent().map(|p| p.join(FOLDER))
}

/// Where `part`'s package and compiled model are in the Core ML folder `dir`.
pub fn paths(dir: &Path, part: Part) -> (PathBuf, PathBuf) {
    (dir.join(format!("sam3-{}.mlpackage", part.name())), dir.join(format!("sam3-{}.mlmodelc", part.name())))
}

/// Whether `part` is in the Core ML folder `dir` (packaged or compiled).
fn present(dir: &Path, part: Part) -> bool {
    let (package, compiled) = paths(dir, part);
    package.is_dir() || compiled.is_dir()
}

/// Whether the folder `dir` holds the prompt model split in two (`textenc` and `textdet`, issue #437).
fn split_in(dir: &Path) -> bool {
    present(dir, Part::TextEnc) && present(dir, Part::TextDet)
}

/// The parts the folder `dir` is used with: the split prompt model when it is there, else the old `textscore`.
fn used(dir: &Path) -> Vec<Part> {
    let text: &[Part] = if split_in(dir) { &[Part::TextEnc, Part::TextDet] } else { &[Part::TextScore] };
    Part::BASE.iter().chain(text).copied().collect()
}

/// The parts the Core ML folder beside the model folder `model_dir` is used with (all of them, or some missing).
pub fn used_parts(model_dir: &Path) -> Vec<Part> {
    folder(model_dir).map(|dir| used(&dir)).unwrap_or_default()
}

/// Whether every part's Core ML version is beside the model folder `model_dir` (packaged or compiled, with the prompt
/// model split or in one part; whether they load is only known by loading them).
pub fn installed(model_dir: &Path) -> bool {
    folder(model_dir).is_some_and(|dir| used(&dir).iter().all(|p| present(&dir, *p)))
}

/// A part, loaded when first needed (and the text encoder let go again, [`Parts::release_text_encoder`]).
struct Slot {
    /// Serialises loading and runs, and says whether the part has run once (Core ML planned it on the GPU).
    state: Mutex<SlotState>,
}

#[derive(Default)]
struct SlotState {
    model: Option<Model>,
    warm: bool,
}

/// The parts on one set of compute units.
pub struct Parts {
    dir: PathBuf,
    units: ComputeUnits,
    /// The prompt model is split (`textenc` and `textdet`) rather than the old `textscore`.
    split: bool,
    /// The key of the text encoder's numbers ([`Parts::text_key`]).
    text_key: Option<String>,
    slots: [Slot; 8],
}

/// The click decoder's features of a photo (Core ML's or candle's numbers, on the CPU).
pub struct ClickFeatures {
    pub image: Vec<f32>,
    pub s0: Vec<f32>,
    pub s1: Vec<f32>,
}

/// The prompt features of a photo.
pub struct TextFeatures {
    pub l0: Vec<f32>,
    pub l1: Vec<f32>,
    pub l2: Vec<f32>,
}

/// What `textdet` (or the old `textscore`) gives for one prompt.
pub struct Scores {
    pub logits: Vec<f32>,
    pub presence: f32,
    queries: Vec<f32>,
    memory: Vec<f32>,
    text: Vec<f32>,
}

/// What `textmask` gives: 200 masks and the semantic mask, 288² each.
pub struct Masks {
    pub masks: Vec<f32>,
    pub semantic: Vec<f32>,
}

fn take(out: &mut std::vec::IntoIter<Output>) -> Result<Vec<f32>, Error> {
    out.next().map(|o| o.data).ok_or_else(|| Error::Output("an output is missing".into()))
}

/// The click slots of `clicks` (normalised to the photo): points in the 1008 px input and their labels.
pub fn slots(clicks: &[Click]) -> Result<(Vec<f32>, Vec<i32>), Error> {
    if clicks.is_empty() || clicks.len() > MAX_CLICKS {
        return Err(Error::Input(format!("Core ML takes 1 to {MAX_CLICKS} clicks, not {}", clicks.len())));
    }
    let mut points = vec![0f32; SLOTS * 2];
    let mut labels = vec![PAD; SLOTS];
    for (i, c) in clicks.iter().enumerate() {
        if let Some(p) = points.get_mut(2 * i..2 * i + 2) {
            p.copy_from_slice(&[c.x * INPUT as f32, c.y * INPUT as f32]);
        }
        if let Some(l) = labels.get_mut(i) {
            *l = i32::from(c.positive);
        }
    }
    if let Some(l) = labels.get_mut(clicks.len()) {
        *l = NOT_A_POINT;
    }
    Ok((points, labels))
}

impl Parts {
    /// The parts in the Core ML folder of `model_dir`, on `units`. The encoder and the click parts load now (compiled
    /// first when needed, a broken one is an error here); the prompt parts load when first needed or warmed up.
    pub fn load(model_dir: &Path, units: ComputeUnits) -> Result<Self, Error> {
        if !lightcraft_coreml::available() {
            return Err(Error::Unavailable);
        }
        let dir = folder(model_dir).ok_or_else(|| Error::Missing(model_dir.to_path_buf()))?;
        if let Some(missing) = used(&dir).iter().find(|p| !present(&dir, **p)) {
            // half the split prompt model and no old one: name the half that is missing
            let missing = match (*missing, present(&dir, Part::TextEnc), present(&dir, Part::TextDet)) {
                (Part::TextScore, true, false) => Part::TextDet,
                (Part::TextScore, false, true) => Part::TextEnc,
                (p, _, _) => p,
            };
            return Err(Error::Missing(paths(&dir, missing).0));
        }
        let split = split_in(&dir);
        if !split {
            log::info!(
                "SAM 3 on Core ML: the prompt model is the old single textscore part; tools/convert-coreml-sam3.py --parts textenc,textdet splits it"
            );
        }
        let (package, compiled) = paths(&dir, Part::TextEnc);
        let key = lightcraft_coreml::package_key(&package).ok().or_else(|| std::fs::read_to_string(format!("{}.key", compiled.display())).ok());
        let text_key = key.map(|k| format!("{units:?}-{k}"));
        let parts = Self { dir, units, split, text_key, slots: std::array::from_fn(|_| Slot { state: Mutex::new(SlotState::default()) }) };
        for part in [Part::Encoder, Part::ClickFeat, Part::ClickDec] {
            let mut held = parts.lock(part);
            parts.model(&mut held, part)?;
        }
        Ok(parts)
    }

    /// Whether the prompt model is split, so a prompt with stored text features runs without the text encoder.
    pub fn split(&self) -> bool {
        self.split
    }

    /// The key of the text encoder's numbers: its package (or compiled copy) and the units it runs on; `None` when
    /// neither can be read (its features are not stored then).
    pub fn text_key(&self) -> Option<String> {
        self.text_key.clone()
    }

    /// The compute units the parts run on.
    pub fn units(&self) -> ComputeUnits {
        self.units
    }

    fn slot(&self, part: Part) -> &Slot {
        let i = part.index();
        // (index() is always inside ALL, which `slots` matches)
        &self.slots[i.min(self.slots.len() - 1)]
    }

    fn lock(&self, part: Part) -> std::sync::MutexGuard<'_, SlotState> {
        self.slot(part).state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `part`'s model, loading it the first time (`state` is its slot, locked).
    fn model<'a>(&self, state: &'a mut SlotState, part: Part) -> Result<&'a Model, Error> {
        if state.model.is_some() {
            return state.model.as_ref().ok_or(Error::Unavailable);
        }
        let (package, compiled) = paths(&self.dir, part);
        let model = lightcraft_coreml::load_package(&package, &compiled, self.units)?;
        let ins = part.inputs().iter().all(|(n, _, _)| model.inputs().iter().any(|m| m == n));
        let outs = part.outputs().iter().all(|(n, _)| model.outputs().iter().any(|m| m == n));
        if !ins || !outs || model.inputs().len() != part.inputs().len() {
            return Err(Error::Load(format!(
                "{} is not the converted SAM 3 {} (it takes {:?} and gives {:?})",
                package.display(),
                part.name(),
                model.inputs(),
                model.outputs()
            )));
        }
        Ok(state.model.insert(model))
    }

    /// Let the text encoder go (after a fixed phrase's features are stored); it loads again for the next prompt that
    /// needs it.
    pub fn release_text_encoder(&self) {
        *self.lock(Part::TextEnc) = SlotState::default();
    }

    /// Whether the text encoder is in memory (tests).
    pub fn text_encoder_loaded(&self) -> bool {
        self.lock(Part::TextEnc).model.is_some()
    }

    /// Run `part` on `data` (its inputs in order) and read its outputs, each checked for its size and for numbers that
    /// are not finite.
    fn run(&self, part: Part, data: &[Data<'_>]) -> Result<std::vec::IntoIter<Output>, Error> {
        let specs = part.inputs();
        if data.len() != specs.len() {
            return Err(Error::Input(format!("{} takes {} inputs", part.name(), specs.len())));
        }
        let inputs: Vec<Tensor<'_>> = specs
            .iter()
            .zip(data)
            .map(|((name, shape, _), d)| match d {
                Data::F(v) => Tensor::f32(name, shape, v),
                Data::I(v) => Tensor::i32(name, shape, v),
            })
            .collect();
        let names: Vec<&str> = part.outputs().iter().map(|(n, _)| *n).collect();
        let mut state = self.lock(part);
        let out = self.model(&mut state, part)?.predict_many(&inputs, &names)?;
        state.warm = true;
        drop(state);
        check(part, &out)?;
        Ok(out.into_iter())
    }

    /// The backbone `[1, 1024, 72, 72]` of preprocessed pixels `[1, 3, 1008, 1008]` (−1..1).
    pub fn encode(&self, pixels: &[f32]) -> Result<Vec<f32>, Error> {
        take(&mut self.run(Part::Encoder, &[Data::F(pixels)])?)
    }

    /// The click decoder's features of a backbone.
    pub fn click_features(&self, backbone: &[f32]) -> Result<ClickFeatures, Error> {
        let mut out = self.run(Part::ClickFeat, &[Data::F(backbone)])?;
        Ok(ClickFeatures { image: take(&mut out)?, s0: take(&mut out)?, s1: take(&mut out)? })
    }

    /// The decoder's four masks, their IoUs and the object score for `clicks` (1 to [`MAX_CLICKS`]).
    pub fn click_heads(&self, f: &ClickFeatures, clicks: &[Click]) -> Result<Heads, Error> {
        let (points, labels) = slots(clicks)?;
        let mut out = self.run(Part::ClickDec, &[Data::F(&f.image), Data::F(&f.s0), Data::F(&f.s1), Data::F(&points), Data::I(&labels)])?;
        let masks = take(&mut out)?;
        let iou = take(&mut out)?;
        let object_score = take(&mut out)?.first().copied().unwrap_or(0.0);
        Ok(Heads { masks, iou, object_score })
    }

    /// The prompt features of a backbone.
    pub fn text_features(&self, backbone: &[f32]) -> Result<TextFeatures, Error> {
        let mut out = self.run(Part::TextFeat, &[Data::F(backbone)])?;
        Ok(TextFeatures { l0: take(&mut out)?, l1: take(&mut out)?, l2: take(&mut out)? })
    }

    /// The text features `[1, 32, 256]` of a tokenized prompt (`ids` and `valid` as the tokenizer gives them, 32
    /// each), from the split prompt model's text encoder.
    pub fn text_encode(&self, ids: &[u32], valid: &[bool]) -> Result<Vec<f32>, Error> {
        let (ids, mask) = tokens(ids, valid)?;
        take(&mut self.run(Part::TextEnc, &[Data::I(&ids), Data::I(&mask)])?)
    }

    /// Score the 200 queries for a prompt's text features (`valid` marks its real tokens), on the split prompt model.
    pub fn text_detect(&self, f: &TextFeatures, text: Vec<f32>, valid: &[bool]) -> Result<Scores, Error> {
        let mask: Vec<i32> = valid.iter().map(|v| i32::from(*v)).collect();
        let mut out = self.run(Part::TextDet, &[Data::F(&f.l2), Data::F(&text), Data::I(&mask)])?;
        let (logits, presence) = scores(&mut out)?;
        Ok(Scores { logits, presence, queries: take(&mut out)?, memory: take(&mut out)?, text })
    }

    /// Score the 200 queries for a tokenized prompt on the old single `textscore` part (text encoder included).
    pub fn text_score(&self, f: &TextFeatures, ids: &[u32], valid: &[bool]) -> Result<Scores, Error> {
        let (ids, mask) = tokens(ids, valid)?;
        let mut out = self.run(Part::TextScore, &[Data::F(&f.l2), Data::I(&ids), Data::I(&mask)])?;
        let (logits, presence) = scores(&mut out)?;
        Ok(Scores { logits, presence, queries: take(&mut out)?, memory: take(&mut out)?, text: take(&mut out)? })
    }

    /// The 200 masks and the semantic mask of a scored prompt.
    pub fn text_masks(&self, f: &TextFeatures, s: &Scores, valid: &[bool]) -> Result<Masks, Error> {
        let mask: Vec<i32> = valid.iter().map(|v| i32::from(*v)).collect();
        let data = [Data::F(&f.l0), Data::F(&f.l1), Data::F(&s.memory), Data::F(&s.text), Data::I(&mask), Data::F(&s.queries)];
        let mut out = self.run(Part::TextMask, &data)?;
        Ok(Masks { masks: take(&mut out)?, semantic: take(&mut out)? })
    }

    /// The order the parts are warmed up in: the click decoder first (the user's next step after a photo is ready),
    /// then the prompt parts; the encoder and the click features run when a photo is analysed. The split prompt
    /// model's text encoder is never warmed: it loads only for a prompt without stored text features.
    fn warm_order(&self) -> [Part; 6] {
        let score = if self.split { Part::TextDet } else { Part::TextScore };
        [Part::ClickDec, Part::TextFeat, score, Part::TextMask, Part::Encoder, Part::ClickFeat]
    }

    /// Whether every part warmed up has run once since loading. A part running right now counts as not run yet.
    pub fn warmed(&self) -> bool {
        self.warm_order().iter().all(|p| self.slot(*p).state.try_lock().is_ok_and(|s| s.warm))
    }

    /// Run the next part that has not run yet on made-up inputs, so Core ML plans it on the GPU now and not in the
    /// user's first click or prompt. Never waits for a user's run: a part running right now is skipped (that run
    /// plans it), and so is a part that has run. One prediction at most; `Ok(false)` when no part was left to run.
    pub fn warm_next(&self) -> Result<bool, Error> {
        for part in self.warm_order() {
            let mut state = match self.slot(part).state.try_lock() {
                Ok(w) => w,
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => continue,
            };
            if state.warm {
                continue;
            }
            let model = self.model(&mut state, part)?;
            let made_up: Vec<(Vec<f32>, Vec<i32>)> = part
                .inputs()
                .iter()
                .map(|(name, shape, whole)| {
                    let n: usize = shape.iter().product();
                    match (*whole, *name) {
                        (true, "labels") => (Vec::new(), (0..n).map(|i| [1, NOT_A_POINT].get(i).copied().unwrap_or(PAD)).collect()),
                        (true, "ids") => (Vec::new(), (0..n).map(|i| if i == 0 { 49406 } else { 49407 }).collect()),
                        (true, _) => (Vec::new(), (0..n).map(|i| i32::from(i < 2)).collect()),
                        (false, _) => (vec![0.0; n], Vec::new()),
                    }
                })
                .collect();
            let inputs: Vec<Tensor<'_>> = part
                .inputs()
                .iter()
                .zip(&made_up)
                .map(|((name, shape, whole), (f, i))| if *whole { Tensor::i32(name, shape, i) } else { Tensor::f32(name, shape, f) })
                .collect();
            let names: Vec<&str> = part.outputs().iter().map(|(n, _)| *n).collect();
            model.predict_many(&inputs, &names)?;
            state.warm = true;
            return Ok(true);
        }
        Ok(false)
    }

    /// [`Parts::warm_next`] until every part has run or `stop` is raised (checked between parts).
    pub fn warm_up(&self, stop: &AtomicBool) -> Result<(), Error> {
        while !stop.load(Ordering::Relaxed) && self.warm_next()? {}
        Ok(())
    }
}

/// `part`'s outputs, each of its size and with finite numbers only (float32 should never overflow, but a damaged or
/// foreign package could give anything): else an error, and on Automatic candle takes the run over.
fn check(part: Part, out: &[Output]) -> Result<(), Error> {
    if out.len() != part.outputs().len() {
        return Err(Error::Output(format!("SAM 3 {}: {} outputs, not {}", part.name(), out.len(), part.outputs().len())));
    }
    for ((name, len), o) in part.outputs().iter().zip(out) {
        if o.data.len() != *len {
            return Err(Error::Output(format!("SAM 3 {}: `{name}` holds {} numbers, not {len}", part.name(), o.data.len())));
        }
        if let Some(i) = o.data.iter().position(|v| !v.is_finite()) {
            return Err(Error::Output(format!("SAM 3 {}: `{name}` has a number that is not finite at {i}", part.name())));
        }
    }
    Ok(())
}

/// The logits and the presence score, clamped as candle clamps them, and the boxes skipped.
fn scores(out: &mut std::vec::IntoIter<Output>) -> Result<(Vec<f32>, f32), Error> {
    let logits = take(out)?.into_iter().map(|l| l.clamp(-12.0, 12.0)).collect();
    let presence = take(out)?.first().copied().unwrap_or(-10.0).clamp(-10.0, 10.0);
    let _boxes = take(out)?;
    Ok((logits, presence))
}

/// The token ids and mask as `textenc` and `textscore` take them (32 of each).
fn tokens(ids: &[u32], valid: &[bool]) -> Result<(Vec<i32>, Vec<i32>), Error> {
    if ids.len() != TOKENS || valid.len() != TOKENS {
        return Err(Error::Input(format!("a prompt is {TOKENS} tokens, not {}", ids.len())));
    }
    let ids = ids.iter().map(|i| i32::try_from(*i).map_err(|_| Error::Input(format!("token {i}")))).collect::<Result<_, _>>()?;
    Ok((ids, valid.iter().map(|v| i32::from(*v)).collect()))
}

/// The numbers of an input.
enum Data<'a> {
    F(&'a [f32]),
    I(&'a [i32]),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clicks fill the first slots, the "not a point" closes them and the rest are unused; no click or too many is an
    /// error (candle takes those).
    #[test]
    fn clicks_fill_the_slots_in_order() {
        let clicks = [Click { x: 0.5, y: 0.25, positive: true }, Click { x: 0.0, y: 1.0, positive: false }];
        let (points, labels) = slots(&clicks).unwrap();
        assert_eq!((points.len(), labels.len()), (SLOTS * 2, SLOTS));
        assert_eq!(points[..4], [504.0, 252.0, 0.0, 1008.0]);
        assert_eq!(labels[..4], [1, 0, NOT_A_POINT, PAD]);
        assert!(labels[3..].iter().all(|l| *l == PAD));
        assert!(slots(&[]).is_err());
        assert!(slots(&vec![clicks[0]; MAX_CLICKS]).is_ok());
        assert!(slots(&vec![clicks[0]; MAX_CLICKS + 1]).is_err());
    }

    /// An output of the wrong size or with a number that is not finite is an error (on Automatic, candle takes over).
    #[test]
    fn a_wrong_or_non_finite_output_is_an_error() {
        let out = |iou: Vec<f32>| {
            vec![
                Output { shape: vec![1, 4, MASK_SIDE, MASK_SIDE], data: vec![0.5; 4 * CELLS] },
                Output { shape: vec![1, iou.len()], data: iou },
                Output { shape: vec![1, 1], data: vec![2.0] },
            ]
        };
        assert_eq!(check(Part::ClickDec, &out(vec![0.1; 4])), Ok(()));
        assert!(matches!(check(Part::ClickDec, &out(vec![0.1; 3])), Err(Error::Output(_))));
        assert!(matches!(check(Part::ClickDec, &out(vec![0.1, f32::NAN, 0.2, 0.3])), Err(Error::Output(_))));
        assert!(matches!(check(Part::ClickDec, &out(vec![0.1, 0.2, f32::INFINITY, 0.3])), Err(Error::Output(_))));
        assert!(matches!(check(Part::ClickDec, &out(vec![0.1; 4])[..2]), Err(Error::Output(_))));
    }

    /// The Core ML folder is the `sam3` folder's sibling, and it counts as installed only with every part, the
    /// prompt model split in two (issue #437) or in the old single `textscore` (a folder converted before #437).
    #[test]
    fn the_packages_live_beside_the_model_folder() {
        let root = std::env::temp_dir().join(format!("lc-sam3-coreml-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let model = root.join("sam3");
        std::fs::create_dir_all(&model).unwrap();
        let dir = root.join(FOLDER);
        assert_eq!(folder(&model), Some(dir.clone()));
        assert!(!installed(&model));
        for (i, p) in Part::BASE.iter().enumerate() {
            let (package, compiled) = paths(&dir, *p);
            std::fs::create_dir_all(if i % 2 == 0 { package } else { compiled }).unwrap();
            assert!(!installed(&model));
        }
        std::fs::create_dir_all(paths(&dir, Part::TextScore).0).unwrap();
        assert!(installed(&model) && !split_in(&dir), "an old conversion");
        std::fs::create_dir_all(paths(&dir, Part::TextEnc).0).unwrap();
        assert!(!split_in(&dir), "half the split prompt model: the old one is used");
        std::fs::create_dir_all(paths(&dir, Part::TextDet).1).unwrap();
        std::fs::remove_dir_all(paths(&dir, Part::TextScore).0).unwrap();
        assert!(installed(&model) && split_in(&dir), "a new conversion");
        let _ = std::fs::remove_dir_all(&root);
    }
}
