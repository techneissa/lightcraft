//! SAM 3 through Apple's Core ML (issue #268, `fork/coreml.md`): six converted parts on the GPU in float32, candle
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
//! | textscore | per prompt | l2, ids, mask → logits, presence, boxes, queries, memory, text |
//! | textmask | per prompt that matches | l0, l1, memory, text, mask, queries → masks, semantic |
//!
//! Clicks go in [`SLOTS`] fixed slots (one input shape: Core ML plans a model on the GPU again for every new shape);
//! the decoder masks unused slots out of its attention, so a click's mask is the one of the clicks alone. A prompt
//! that matches nothing never runs `textmask`. Every output is checked for its size and for numbers that are not
//! finite: such an output is an error, and on Automatic candle takes the run over.
//!
//! Core ML plans each part on the GPU at its first run after loading (1.6–5.6 s once per launch); [`Parts::warm_up`]
//! runs each part once on made-up inputs, off the thread that runs the user's clicks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

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
    TextScore,
    TextMask,
}

impl Part {
    pub const ALL: [Part; 6] = [Part::Encoder, Part::ClickFeat, Part::ClickDec, Part::TextFeat, Part::TextScore, Part::TextMask];

    /// Its name in the package's file name (`sam3-<name>.mlpackage`).
    pub fn name(self) -> &'static str {
        match self {
            Part::Encoder => "encoder",
            Part::ClickFeat => "clickfeat",
            Part::ClickDec => "clickdec",
            Part::TextFeat => "textfeat",
            Part::TextScore => "textscore",
            Part::TextMask => "textmask",
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

/// Whether every part's Core ML version is beside the model folder `model_dir` (packaged or compiled; whether they
/// load is only known by loading them).
pub fn installed(model_dir: &Path) -> bool {
    folder(model_dir).is_some_and(|dir| {
        Part::ALL.iter().all(|p| {
            let (package, compiled) = paths(&dir, *p);
            package.is_dir() || compiled.is_dir()
        })
    })
}

/// A part, loaded when first needed.
struct Slot {
    model: OnceLock<Model>,
    /// Serialises loading and runs, and says whether the part has run once (Core ML planned it on the GPU).
    warm: Mutex<bool>,
}

/// The six parts on one set of compute units.
pub struct Parts {
    dir: PathBuf,
    units: ComputeUnits,
    slots: [Slot; 6],
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

/// What `textscore` gives for one prompt.
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
        if let Some(missing) = Part::ALL.iter().map(|p| paths(&dir, *p)).find(|(package, compiled)| !package.is_dir() && !compiled.is_dir()) {
            return Err(Error::Missing(missing.0));
        }
        let parts = Self { dir, units, slots: std::array::from_fn(|_| Slot { model: OnceLock::new(), warm: Mutex::new(false) }) };
        for part in [Part::Encoder, Part::ClickFeat, Part::ClickDec] {
            let _held = parts.lock(part);
            parts.model(part)?;
        }
        Ok(parts)
    }

    fn slot(&self, part: Part) -> &Slot {
        let i = part.index();
        // (index() is always inside ALL, which `slots` matches)
        &self.slots[i.min(self.slots.len() - 1)]
    }

    fn lock(&self, part: Part) -> std::sync::MutexGuard<'_, bool> {
        self.slot(part).warm.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `part`'s model, loading it the first time (call with its lock held).
    fn model(&self, part: Part) -> Result<&Model, Error> {
        let slot = self.slot(part);
        if let Some(m) = slot.model.get() {
            return Ok(m);
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
        Ok(slot.model.get_or_init(|| model))
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
        let mut warm = self.lock(part);
        let out = self.model(part)?.predict_many(&inputs, &names)?;
        *warm = true;
        drop(warm);
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

    /// Score the 200 queries for a tokenized prompt (`ids` and `valid` as the tokenizer gives them, 32 each).
    pub fn text_score(&self, f: &TextFeatures, ids: &[u32], valid: &[bool]) -> Result<Scores, Error> {
        let (ids, mask) = tokens(ids, valid)?;
        let mut out = self.run(Part::TextScore, &[Data::F(&f.l2), Data::I(&ids), Data::I(&mask)])?;
        // (clamped as candle clamps them)
        let logits = take(&mut out)?.into_iter().map(|l| l.clamp(-12.0, 12.0)).collect();
        let presence = take(&mut out)?.first().copied().unwrap_or(-10.0).clamp(-10.0, 10.0);
        let _boxes = take(&mut out)?;
        Ok(Scores { logits, presence, queries: take(&mut out)?, memory: take(&mut out)?, text: take(&mut out)? })
    }

    /// The 200 masks and the semantic mask of a scored prompt.
    pub fn text_masks(&self, f: &TextFeatures, s: &Scores, valid: &[bool]) -> Result<Masks, Error> {
        let mask: Vec<i32> = valid.iter().map(|v| i32::from(*v)).collect();
        let data = [Data::F(&f.l0), Data::F(&f.l1), Data::F(&s.memory), Data::F(&s.text), Data::I(&mask), Data::F(&s.queries)];
        let mut out = self.run(Part::TextMask, &data)?;
        Ok(Masks { masks: take(&mut out)?, semantic: take(&mut out)? })
    }

    /// Run each part once on made-up inputs, so Core ML plans it on the GPU now and not in the user's first click or
    /// prompt: the click decoder first, then the encoder and the prompt parts. A part that already ran is skipped;
    /// `stop` is checked between parts. Returns the first error (the parts after it are not warmed).
    pub fn warm_up(&self, stop: &AtomicBool) -> Result<(), Error> {
        let order = [Part::ClickDec, Part::Encoder, Part::ClickFeat, Part::TextFeat, Part::TextScore, Part::TextMask];
        for part in order {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let mut warm = self.lock(part);
            if *warm {
                continue;
            }
            let model = self.model(part)?;
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
            *warm = true;
        }
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

/// The token ids and mask as `textscore` takes them (32 of each).
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

    /// The Core ML folder is the `sam3` folder's sibling, and it counts as installed only with all six parts.
    #[test]
    fn the_packages_live_beside_the_model_folder() {
        let root = std::env::temp_dir().join(format!("lc-sam3-coreml-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let model = root.join("sam3");
        std::fs::create_dir_all(&model).unwrap();
        assert_eq!(folder(&model), Some(root.join(FOLDER)));
        assert!(!installed(&model));
        for (i, p) in Part::ALL.iter().enumerate() {
            let (package, compiled) = paths(&root.join(FOLDER), *p);
            std::fs::create_dir_all(if i % 2 == 0 { package } else { compiled }).unwrap();
            assert_eq!(installed(&model), i + 1 == Part::ALL.len());
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
