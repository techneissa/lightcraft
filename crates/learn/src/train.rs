//! Training: from examples (features and targets) to a model per slider group, and an honest accuracy.
//!
//! Each group picks its kernel (how much the encoder counts, linear or Gaussian) by leave-one-out error, and each
//! slider its ridge strength. The accuracy shown to the photographer comes from photos the model never saw: the
//! examples are split in up to five folds, and each fold is predicted by a model trained (with its own choices)
//! on the others. A slider that is no closer to the photographer's edits than leaving the photo at its defaults is
//! skipped (a colour wheel's two axes together), so one slider without a pattern never takes the others of its group
//! with it; a profile with no slider left is refused, so a weak profile can't pretend.

use serde::{Deserialize, Serialize};

use crate::ridge::{self, Kernel, Layout};
use crate::sliders::{Group, SLIDERS};
use crate::{Error, Result};

/// Fewest examples a profile trains on.
pub const MIN_EXAMPLES: usize = 10;
/// Most examples a profile holds.
pub const MAX_EXAMPLES: usize = 200;
/// A slider is learned when its held-out error is at most this share of the defaults' error: at least 20 % closer.
/// With 12 examples, 3 % of sliders moved at random pass it (10 % margin: 9 %).
const KEEP_RATIO: f64 = 0.8;
const FOLDS: usize = 5;

/// One photo the profile learns from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Example {
    /// Which photo it was (a library photo's id, or a file name), so adding it again replaces it.
    pub photo: String,
    pub features: Vec<f32>,
    /// One per [`SLIDERS`] entry.
    pub targets: Vec<f32>,
}

/// Feature means and spreads of the examples a model was trained on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Standardizer {
    pub mean: Vec<f32>,
    pub scale: Vec<f32>,
}

impl Standardizer {
    fn of(rows: &[&[f32]], dims: usize) -> Standardizer {
        let n = rows.len().max(1) as f64;
        let mut mean = vec![0f64; dims];
        for r in rows {
            for (m, v) in mean.iter_mut().zip(r.iter()) {
                *m += *v as f64 / n;
            }
        }
        let mut var = vec![0f64; dims];
        for r in rows {
            for ((s, v), m) in var.iter_mut().zip(r.iter()).zip(&mean) {
                *s += (*v as f64 - m).powi(2) / n;
            }
        }
        Standardizer { mean: mean.iter().map(|m| *m as f32).collect(), scale: var.iter().map(|v| v.sqrt().max(1e-3) as f32).collect() }
    }

    pub fn apply(&self, f: &[f32]) -> Vec<f32> {
        f.iter().zip(self.mean.iter().zip(&self.scale)).map(|(v, (m, s))| ((v - m) / s).clamp(-8.0, 8.0)).collect()
    }
}

/// The model of one group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupModel {
    pub group: Group,
    pub kernel: Kernel,
    /// Slider ids, in [`SLIDERS`] order; only sliders some example changed.
    pub sliders: Vec<String>,
    pub mean: Vec<f64>,
    /// `examples × sliders`, row-major.
    pub alpha: Vec<f64>,
}

/// A trained model: what [`crate::Profile::predict`] uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub layout: Layout,
    pub standardizer: Standardizer,
    pub groups: Vec<GroupModel>,
    /// How many examples it was trained on (the first ones of the profile).
    pub examples: usize,
}

/// Held-out accuracy of one slider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliderScore {
    pub id: String,
    /// Mean distance of the prediction from the photographer's value, in the slider's units.
    pub error: f64,
    /// The same for leaving the photo at its defaults.
    pub default_error: f64,
    /// Learned (closer than the defaults on held-out photos), or skipped. `None` in profiles saved before sliders
    /// were decided one by one: there the group's `kept` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<bool>,
}

impl SliderScore {
    /// Whether the slider is learned, for a report of either kind (`group_kept`: its group's decision).
    pub fn learned(&self, group_kept: bool) -> bool {
        self.kept.unwrap_or(group_kept)
    }
}

/// Held-out accuracy of one group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupScore {
    pub group: Group,
    /// Some of its sliders are learned, or none.
    pub kept: bool,
    /// Held-out error of its learned sliders (of all, when none is) as a share of the defaults' error (lower is
    /// better; 1 = no better than doing nothing).
    pub ratio: f64,
    /// The same for applying the average edit to every photo, like a fixed preset.
    pub preset_ratio: f64,
    pub sliders: Vec<SliderScore>,
}

/// What training found, in words a photographer can act on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub examples: usize,
    /// Held-out photos where the profile came closer to the photographer's edit than the defaults did.
    pub beats_defaults: usize,
    /// …and closer than the average edit.
    pub beats_preset: usize,
    /// Held-out error of the kept groups as a share of the defaults' error.
    pub ratio: f64,
    pub groups: Vec<GroupScore>,
}

/// Smallest spread (root mean square change from the default) that counts as an edit, per slider.
fn floor_of(id: &str) -> f64 {
    match id {
        "light.exposure" => 0.03,
        "wb.temp" => 2.0,
        id if id.starts_with("curve.") && id.matches('.').count() == 2 => 0.5,
        _ => 0.75,
    }
}

/// Indices of the sliders in `groups` that some example changed, with the spread that scales their errors.
fn edited(examples: &[&Example], groups: &[Group]) -> Vec<(Group, Vec<(usize, f64)>)> {
    groups
        .iter()
        .map(|g| {
            let sliders = SLIDERS
                .iter()
                .enumerate()
                .filter(|(_, s)| s.group == *g)
                .filter_map(|(i, s)| {
                    let rms = (examples.iter().map(|e| (e.targets.get(i).copied().unwrap_or(0.0) as f64).powi(2)).sum::<f64>()
                        / examples.len().max(1) as f64)
                        .sqrt();
                    (rms >= floor_of(s.id)).then_some((i, rms))
                })
                .collect();
            (*g, sliders)
        })
        .filter(|(_, s): &(Group, Vec<(usize, f64)>)| !s.is_empty())
        .collect()
}

/// The kernels tried for each group, simplest first: the photo statistics alone, then more and more of the
/// encoder; each linear, then Gaussian.
fn candidates(layout: Layout) -> Vec<Kernel> {
    let weights: &[f64] = if layout.encoder == 0 { &[0.0] } else { &[0.0, 0.3, 1.0, 3.0] };
    weights.iter().flat_map(|w| [Kernel { encoder_weight: *w, rbf: None }, Kernel { encoder_weight: *w, rbf: Some(0.5) }]).collect()
}

/// A more complex kernel is chosen only when its leave-one-out error is this much lower than a simpler one's: the
/// choice is itself fitted to the examples, and with few of them small wins are mostly luck.
const SIMPLER_WITHIN: f64 = 1.03;

/// Fits one group on `x` (standardised) with the simplest kernel whose leave-one-out error is within
/// [`SIMPLER_WITHIN`] of the best.
fn fit_group(layout: Layout, x: &[Vec<f32>], y: &[Vec<f64>], scales: &[f64], cancel: &dyn Fn() -> bool) -> Result<(Kernel, ridge::Fit)> {
    let mut fits: Vec<(f64, Kernel, ridge::Fit)> = Vec::new();
    for kernel in candidates(layout) {
        if cancel() {
            return Err(Error::Cancelled);
        }
        let f = ridge::fit(&ridge::gram(&kernel, layout, x), y);
        let score: f64 = f.loo_mse.iter().zip(scales).map(|(m, s)| m / (s * s)).sum();
        fits.push((if score.is_finite() { score } else { f64::INFINITY }, kernel, f));
    }
    let best = fits.iter().map(|f| f.0).fold(f64::INFINITY, f64::min);
    let pick = fits.into_iter().find(|f| f.0 <= best * SIMPLER_WITHIN + 1e-12);
    pick.map(|(_, k, f)| (k, f)).ok_or_else(|| Error::Train("no kernel to try".into()))
}

fn targets_of(examples: &[&Example], sliders: &[(usize, f64)]) -> Vec<Vec<f64>> {
    examples.iter().map(|e| sliders.iter().map(|(i, _)| e.targets.get(*i).copied().unwrap_or(0.0) as f64).collect()).collect()
}

/// Fits every group on `examples`.
/// Fits every group of `plan` on `examples`: the model and the examples' standardised features.
fn fit_all(layout: Layout, examples: &[&Example], plan: &[(Group, Vec<(usize, f64)>)], cancel: &dyn Fn() -> bool) -> Result<(Model, Vec<Vec<f32>>)> {
    let rows: Vec<&[f32]> = examples.iter().map(|e| e.features.as_slice()).collect();
    let standardizer = Standardizer::of(&rows, layout.dims());
    let x: Vec<Vec<f32>> = rows.iter().map(|r| standardizer.apply(r)).collect();
    let mut groups = Vec::with_capacity(plan.len());
    for (group, sliders) in plan {
        let y = targets_of(examples, sliders);
        let scales: Vec<f64> = sliders.iter().map(|(_, s)| *s).collect();
        let (kernel, f) = fit_group(layout, &x, &y, &scales, cancel)?;
        groups.push(GroupModel {
            group: *group,
            kernel,
            sliders: sliders.iter().filter_map(|(i, _)| SLIDERS.get(*i).map(|s| s.id.to_string())).collect(),
            mean: f.mean,
            alpha: f.alpha,
        });
    }
    Ok((Model { layout, standardizer, groups, examples: examples.len() }, x))
}

/// The model's predictions for raw features `f`: `(slider index, target)`.
pub fn predict(model: &Model, x: &[Vec<f32>], f: &[f32]) -> Vec<(usize, f32)> {
    let q = model.standardizer.apply(f);
    let mut out = Vec::new();
    for g in &model.groups {
        let p = ridge::predict(&g.kernel, model.layout, x, &g.mean, &g.alpha, &q);
        for (id, v) in g.sliders.iter().zip(p) {
            if let Some(i) = SLIDERS.iter().position(|s| s.id == id) {
                out.push((i, v as f32));
            }
        }
    }
    out
}

/// Per group of a plan, per example, per slider: a prediction (0, the default, where none was made).
type Held = Vec<Vec<Vec<f64>>>;
type Plan = Vec<(Group, Vec<(usize, f64)>)>;

/// Folds of the inner held-out run that decides what a model learns.
const INNER_FOLDS: usize = 3;

/// The part of `chosen` that is in `plan`.
fn restrict(chosen: Plan, plan: &[(Group, Vec<(usize, f64)>)]) -> Plan {
    chosen
        .into_iter()
        .filter_map(|(g, sl)| {
            let (_, own) = plan.iter().find(|(pg, _)| *pg == g)?;
            let sl: Vec<(usize, f64)> = sl.into_iter().filter(|(i, _)| own.iter().any(|(j, _)| j == i)).collect();
            (!sl.is_empty()).then_some((g, sl))
        })
        .collect()
}

/// Where slider `idx` of `group` sits in `plan`.
fn place(plan: &[(Group, Vec<(usize, f64)>)], group: Group, idx: usize) -> Option<(usize, usize)> {
    let gi = plan.iter().position(|(g, _)| *g == group)?;
    let si = plan.get(gi)?.1.iter().position(|(i, _)| *i == idx)?;
    Some((gi, si))
}

/// Out-of-fold predictions of `examples` for the sliders of `plan`, and of the fixed preset (each fold's average
/// edit). Each fold is predicted by a model trained on the others that learns only what its own training photos
/// changed and, with `choose`, only what an inner held-out run on those photos finds worth learning ([`decide`]), so
/// the error of the whole procedure is what is measured.
fn held_out(
    layout: Layout,
    examples: &[&Example],
    plan: &[(Group, Vec<(usize, f64)>)],
    groups: &[Group],
    folds: usize,
    choose: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(Held, Held)> {
    let n = examples.len();
    let mut held: Held = plan.iter().map(|(_, s)| vec![vec![0.0; s.len()]; n]).collect();
    let mut preset = held.clone();
    for (fold, fold_plan) in fold_plans(examples, folds, groups).into_iter().enumerate() {
        if cancel() {
            return Err(Error::Cancelled);
        }
        let train: Vec<&Example> = examples.iter().enumerate().filter(|(i, _)| i % folds != fold).map(|(_, e)| *e).collect();
        let fold_plan = restrict(fold_plan, plan);
        let out: Vec<usize> = (0..n).filter(|i| i % folds == fold).collect();
        for (g, sl) in &fold_plan {
            for (idx, _) in sl {
                let mean = train.iter().map(|e| e.targets.get(*idx).copied().unwrap_or(0.0) as f64).sum::<f64>() / train.len().max(1) as f64;
                let Some((gi, si)) = place(plan, *g, *idx) else { continue };
                for i in &out {
                    if let Some(v) = preset.get_mut(gi).and_then(|h| h.get_mut(*i)).and_then(|r| r.get_mut(si)) {
                        *v = mean;
                    }
                }
            }
        }
        let chosen = if choose { decide(layout, &train, &fold_plan, groups, cancel)? } else { fold_plan };
        if chosen.is_empty() {
            continue;
        }
        let (model, x) = fit_all(layout, &train, &chosen, cancel)?;
        for i in &out {
            let Some(e) = examples.get(*i) else { continue };
            let q = model.standardizer.apply(&e.features);
            for g in &model.groups {
                let p = ridge::predict(&g.kernel, layout, &x, &g.mean, &g.alpha, &q);
                for (id, v) in g.sliders.iter().zip(&p) {
                    let Some(idx) = SLIDERS.iter().position(|s| s.id == id) else { continue };
                    let Some((gi, si)) = place(plan, g.group, idx) else { continue };
                    if let Some(h) = held.get_mut(gi).and_then(|h| h.get_mut(*i)).and_then(|r| r.get_mut(si)) {
                        *h = *v;
                    }
                }
            }
        }
    }
    Ok((held, preset))
}

/// The sliders of `plan` worth learning, judged on `examples` alone (the photos a model is trained on), by an inner
/// held-out run: a slider is kept when its error on photos the inner models had not seen is at most [`KEEP_RATIO`]
/// of leaving them at their defaults. A colour wheel's two axes (one label) are judged together.
fn decide(layout: Layout, examples: &[&Example], plan: &[(Group, Vec<(usize, f64)>)], groups: &[Group], cancel: &dyn Fn() -> bool) -> Result<Plan> {
    let folds = INNER_FOLDS.min(examples.len());
    if folds < 2 || plan.is_empty() {
        return Ok(Vec::new());
    }
    let (held, _) = held_out(layout, examples, plan, groups, folds, false, cancel)?;
    let mut out = Vec::new();
    for ((group, sliders), held) in plan.iter().zip(&held) {
        let label = |si: usize| sliders.get(si).and_then(|(idx, _)| SLIDERS.get(*idx)).map_or("", |s| s.label);
        // per slider: (held-out, defaults) mean absolute error, scaled
        let errs: Vec<(f64, f64)> = sliders
            .iter()
            .enumerate()
            .map(|(si, (idx, scale))| {
                examples.iter().zip(held).fold((0.0, 0.0), |a, (e, h)| {
                    let y = e.targets.get(*idx).copied().unwrap_or(0.0) as f64;
                    let p = h.get(si).copied().unwrap_or(0.0);
                    (a.0 + (p - y).abs() / scale, a.1 + y.abs() / scale)
                })
            })
            .collect();
        let kept: Vec<(usize, f64)> = sliders
            .iter()
            .enumerate()
            .filter(|(si, _)| {
                let (e, d) = (0..sliders.len())
                    .filter(|sj| label(*sj) == label(*si))
                    .filter_map(|sj| errs.get(sj))
                    .fold((0.0, 0.0), |a, t| (a.0 + t.0, a.1 + t.1));
                d > 1e-9 && e <= KEEP_RATIO * d
            })
            .map(|(_, s)| *s)
            .collect();
        if !kept.is_empty() {
            out.push((*group, kept));
        }
    }
    Ok(out)
}

/// What each held-out fold learns: the sliders its own training photos changed, never ones only its held-out photos
/// changed (that would leak them into the accuracy shown). Example `i` is held out in fold `i % folds`.
pub(crate) fn fold_plans(all: &[&Example], folds: usize, groups: &[Group]) -> Vec<Vec<(Group, Vec<(usize, f64)>)>> {
    (0..folds)
        .map(|fold| {
            let train: Vec<&Example> = all.iter().enumerate().filter(|(i, _)| i % folds != fold).map(|(_, e)| *e).collect();
            edited(&train, groups)
        })
        .collect()
}

/// Check what can be trained before spending time on it.
pub fn check(examples: &[Example], layout: Layout, groups: &[Group]) -> Result<()> {
    if examples.len() < MIN_EXAMPLES {
        return Err(Error::Train(format!("a profile needs at least {MIN_EXAMPLES} edited examples; this one has {}", examples.len())));
    }
    if examples.len() > MAX_EXAMPLES {
        return Err(Error::Train(format!("a profile holds at most {MAX_EXAMPLES} examples; this one has {}", examples.len())));
    }
    if let Some(e) = examples.iter().find(|e| e.features.len() != layout.dims() || e.targets.len() != SLIDERS.len()) {
        return Err(Error::Train(format!("example {} was made by another version; add it again", e.photo)));
    }
    if groups.is_empty() {
        return Err(Error::Train("choose at least one group of sliders to learn".into()));
    }
    let all: Vec<&Example> = examples.iter().collect();
    if edited(&all, groups).is_empty() {
        let names: Vec<&str> = groups.iter().map(|g| g.label()).collect();
        return Err(Error::Train(format!("none of the examples changed {} from its defaults: there is nothing to learn", names.join(", "))));
    }
    Ok(())
}

/// Train on `examples`, learning `groups`. `cancel` is asked between steps; `progress(done, total)` reports them.
pub fn train(
    examples: &[Example],
    layout: Layout,
    groups: &[Group],
    cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<(Model, Report)> {
    check(examples, layout, groups)?;
    let all: Vec<&Example> = examples.iter().collect();
    let plan = edited(&all, groups);
    let n = examples.len();
    let folds = FOLDS.min(n);
    // held-out predictions of every example (each fold choosing what it learns from its own training photos)
    progress(0, 3);
    let (held, preset) = held_out(layout, &all, &plan, groups, folds, true, cancel)?;
    // what the profile learns: decided on all the examples, the way each fold decided on its own
    progress(1, 3);
    let kept_plan = decide(layout, &all, &plan, groups, cancel)?;
    progress(2, 3);
    // score each slider on the held-out predictions; a wheel's two axes (one label) are kept or dropped together
    let mut scores = Vec::with_capacity(plan.len());
    let mut per_photo = vec![(0.0f64, 0.0f64, 0.0f64); n];
    let mut all_ratios = Vec::with_capacity(plan.len());
    for (gi, (group, sliders)) in plan.iter().enumerate() {
        // per slider: summed errors in its own units, and per photo in shared (scaled) units
        let mut sums = vec![(0.0f64, 0.0f64); sliders.len()];
        let mut photo_errs = vec![vec![(0.0f64, 0.0f64, 0.0f64); n]; sliders.len()];
        for (i, e) in all.iter().enumerate() {
            for (si, (idx, scale)) in sliders.iter().enumerate() {
                let y = e.targets.get(*idx).copied().unwrap_or(0.0) as f64;
                let p = held.get(gi).and_then(|h| h.get(i)).and_then(|r| r.get(si)).copied().unwrap_or(0.0);
                let m = preset.get(gi).and_then(|h| h.get(i)).and_then(|r| r.get(si)).copied().unwrap_or(0.0);
                let (de, dd, dm) = ((p - y).abs(), y.abs(), (m - y).abs());
                if let Some(s) = sums.get_mut(si) {
                    s.0 += de;
                    s.1 += dd;
                }
                if let Some(pe) = photo_errs.get_mut(si).and_then(|v| v.get_mut(i)) {
                    *pe = (de / scale, dd / scale, dm / scale);
                }
            }
        }
        let total =
            |si: usize| photo_errs.get(si).map_or((0.0, 0.0, 0.0), |v| v.iter().fold((0.0, 0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1, a.2 + p.2)));
        let kept: Vec<bool> = sliders.iter().map(|(idx, _)| kept_plan.iter().any(|(g, sl)| g == group && sl.iter().any(|(j, _)| j == idx))).collect();
        let ratio_of = |which: &dyn Fn(usize) -> bool| {
            let (e, d, p) = (0..sliders.len()).filter(|si| which(*si)).map(total).fold((0.0, 0.0, 0.0), |a, t| (a.0 + t.0, a.1 + t.1, a.2 + t.2));
            if d > 1e-9 { (e / d, p / d) } else { (1.0, 1.0) }
        };
        let any = kept.iter().any(|k| *k);
        all_ratios.push(ratio_of(&|_| true).0);
        let (ratio, preset_ratio) = if any { ratio_of(&|si| kept.get(si).copied().unwrap_or(false)) } else { ratio_of(&|_| true) };
        for (si, k) in kept.iter().enumerate() {
            if *k && let Some(v) = photo_errs.get(si) {
                for (acc, pe) in per_photo.iter_mut().zip(v) {
                    acc.0 += pe.0;
                    acc.1 += pe.1;
                    acc.2 += pe.2;
                }
            }
        }
        let slider_scores = sliders
            .iter()
            .zip(&sums)
            .zip(&kept)
            .filter_map(|(((idx, _), (e, d)), k)| {
                SLIDERS.get(*idx).map(|s| SliderScore { id: s.id.to_string(), error: e / n as f64, default_error: d / n as f64, kept: Some(*k) })
            })
            .collect();
        scores.push(GroupScore { group: *group, kept: any, ratio, preset_ratio, sliders: slider_scores });
    }
    if kept_plan.is_empty() {
        let worst = scores.iter().zip(&all_ratios).map(|(s, r)| format!("{} {:.0} %", s.group.label(), r * 100.0)).collect::<Vec<_>>().join(", ");
        return Err(Error::Train(format!(
            "these examples don't share a pattern the profile can learn: on photos it had not seen, it was no closer to your edits than leaving them at their defaults (error against the defaults': {worst}). Add more examples, or split them into profiles by kind of photo"
        )));
    }
    let (err, def): (f64, f64) = per_photo.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let report = Report {
        examples: n,
        beats_defaults: per_photo.iter().filter(|p| p.1 > 1e-9 && p.0 < p.1).count(),
        beats_preset: per_photo.iter().filter(|p| p.0 < p.2).count(),
        ratio: if def > 1e-9 { err / def } else { 1.0 },
        groups: scores,
    };
    let (model, _) = fit_all(layout, &all, &kept_plan, cancel)?;
    progress(3, 3);
    Ok((model, report))
}
