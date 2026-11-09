//! The Refine brush (fork, `fork/refine-brush.md`, issue #99): strokes painted per feature that set where it works
//! and how strongly.
//!
//! - **Reduce** weakens the feature where it is painted, down to none at Strength 100: the pixels there are what
//!   they were before this feature ran (not the photo's: features that ran earlier stay), bit for bit.
//! - **Restore** takes Reduce and Add back where it is painted (the automatic result again).
//! - **Add** paints the feature where its automatic mask missed: it widens the masks the feature declares
//!   ([`crate::Feature::ADD_PARTS`]) for the face it works on: the face the stroke was painted for (a picked face's
//!   own strokes belong to it), else the face nearest it, and only on or near that face ([`ADD_REACH`]).
//!
//! Strokes are kept per feature ([`Refine`]): for every face in [`crate::Retouch::refine`], and for a face with its
//! own settings in [`crate::FaceOverride::refine`]. Points are normalised like Remove spots (the photo before its
//! crop, after orientation, lens corrections and transform: [`crate::View::to_edit`]); sizes are fractions of the
//! photo's long edge, as the Masking brush keeps them, so a preview, the zoomed loupe and an export sample the same
//! strokes. Each feature gets this without code of its own: the step ([`crate::run`]) wraps every feature run.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::input::Fnv;

mod blend;
mod raster;

pub(crate) use blend::run_refined;
pub use raster::{FEATURE_WORK_CAP, TOTAL_WORK_CAP, grids_made, work};

/// Most strokes one feature keeps (per face); more are not read from settings and not added.
pub const MAX_STROKES: usize = 2000;
/// Most points one stroke keeps.
pub const MAX_POINTS: usize = 20_000;

/// What a stroke does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefineMode {
    /// Weaken the feature, to none at Strength 100.
    #[default]
    Reduce,
    /// Bring the automatic result back (undo Reduce and Add).
    Restore,
    /// Apply the feature where its mask missed.
    Add,
}

impl RefineMode {
    pub const ALL: [RefineMode; 3] = [RefineMode::Reduce, RefineMode::Restore, RefineMode::Add];

    /// The name commands take (`reduce`, `restore`, `add`).
    pub fn name(self) -> &'static str {
        match self {
            RefineMode::Reduce => "reduce",
            RefineMode::Restore => "restore",
            RefineMode::Add => "add",
        }
    }

    pub fn from_name(s: &str) -> Option<RefineMode> {
        RefineMode::ALL.into_iter().find(|m| m.name() == s)
    }

    /// Title Case, as the panel shows it.
    pub fn label(self) -> &'static str {
        match self {
            RefineMode::Reduce => "Reduce",
            RefineMode::Restore => "Restore",
            RefineMode::Add => "Add",
        }
    }
}

/// One stroke of the Refine brush.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RefineStroke {
    pub mode: RefineMode,
    /// Normalised points (see the module docs).
    pub points: Vec<(f64, f64)>,
    /// The brush's radius, a fraction of the photo's long edge.
    pub size: f64,
    /// 0…100.
    pub feather: f64,
    /// 1…100: how much each pass lays down.
    pub flow: f64,
    /// 0…100: how far the stroke goes (Reduce 100 takes the feature away; Add 100 applies it fully).
    pub strength: f64,
    /// Pen pressure (0…1) per point; empty is full pressure. It scales each dab's radius and flow.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pressure: Vec<f32>,
}

impl Default for RefineStroke {
    fn default() -> Self {
        RefineStroke { mode: RefineMode::Reduce, points: Vec::new(), size: 0.04, feather: 50.0, flow: 100.0, strength: 100.0, pressure: Vec::new() }
    }
}

impl RefineStroke {
    /// The stroke with every value in its range, points that are not numbers dropped and the lists capped; `None`
    /// when no point is left.
    pub fn sanitized(mut self) -> Option<RefineStroke> {
        let c = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let d = RefineStroke::default();
        self.size = c(self.size, 0.001, 0.5, d.size);
        self.feather = c(self.feather, 0.0, 100.0, d.feather);
        self.flow = c(self.flow, 1.0, 100.0, d.flow);
        self.strength = c(self.strength, 0.0, 100.0, d.strength);
        // (a point far off the photo paints nothing and would only stretch the stroke's grid)
        self.points.retain(|(x, y)| x.is_finite() && y.is_finite() && (-1.0..=2.0).contains(x) && (-1.0..=2.0).contains(y));
        self.points.truncate(MAX_POINTS);
        self.pressure.truncate(self.points.len());
        for p in &mut self.pressure {
            *p = if p.is_nan() { 1.0 } else { p.clamp(0.0, 1.0) };
        }
        (!self.points.is_empty()).then_some(self)
    }

    /// The pressure at point `i` (0…1; 1 without pressure data).
    pub fn pressure_at(&self, i: usize) -> f32 {
        let p = self.pressure.get(i).or(self.pressure.last()).copied().unwrap_or(1.0);
        if p.is_nan() { 1.0 } else { p.clamp(0.0, 1.0) }
    }

    pub(crate) fn hash_into(&self, h: &mut Fnv) {
        h.u64(self.mode as u64);
        for v in [self.size, self.feather, self.flow, self.strength] {
            h.u64(v.to_bits());
        }
        h.u64(self.points.len() as u64);
        for (x, y) in &self.points {
            h.u64(x.to_bits());
            h.u64(y.to_bits());
        }
        h.u64(self.pressure.len() as u64);
        for p in &self.pressure {
            h.u64(u64::from(p.to_bits()));
        }
    }
}

/// One feature's strokes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RefineLayer {
    /// The feature's key ([`crate::Feature::KEY`]).
    pub feature: String,
    pub strokes: Vec<RefineStroke>,
}

/// The Refine strokes of every feature (for every face, or one face's own). Saved as a list of layers, so a merge
/// of settings (copy / paste, sync) replaces it whole.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Refine(pub Vec<RefineLayer>);

impl Refine {
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|l| l.strokes.is_empty())
    }

    /// The strokes of feature `key`, in the order they were painted.
    pub fn strokes(&self, key: &str) -> &[RefineStroke] {
        self.0.iter().find(|l| l.feature == key).map_or(&[], |l| l.strokes.as_slice())
    }

    /// Add a stroke to feature `key` (sanitized). False for an unknown feature, a stroke without points, or when
    /// the feature already has [`MAX_STROKES`].
    pub fn push(&mut self, key: &str, stroke: RefineStroke) -> bool {
        if !crate::FEATURES.iter().any(|f| f.key == key) {
            return false;
        }
        let Some(stroke) = stroke.sanitized() else { return false };
        match self.0.iter_mut().find(|l| l.feature == key) {
            Some(l) if l.strokes.len() >= MAX_STROKES => false,
            Some(l) => {
                l.strokes.push(stroke);
                true
            }
            None => {
                self.0.push(RefineLayer { feature: key.to_string(), strokes: vec![stroke] });
                true
            }
        }
    }

    /// Remove the strokes of feature `key` (every feature for `None`). Whether there were any.
    pub fn clear(&mut self, key: Option<&str>) -> bool {
        let before = self.count(None);
        match key {
            Some(k) => self.0.retain(|l| l.feature != k),
            None => self.0.clear(),
        }
        before > 0
    }

    /// How many strokes feature `key` has (every feature for `None`).
    pub fn count(&self, key: Option<&str>) -> usize {
        self.0.iter().filter(|l| key.is_none_or(|k| l.feature == k)).map(|l| l.strokes.len()).sum()
    }

    /// From JSON, leniently: a malformed stroke or layer, or one of a feature this build doesn't know, is left out
    /// and the rest kept; values are kept in range and the lists capped.
    pub fn from_value(v: &Value) -> Refine {
        let mut out = Refine::default();
        let Some(layers) = v.as_array() else { return out };
        for l in layers.iter().take(crate::FEATURES.len() * 4) {
            let Some(key) = l.get("feature").and_then(Value::as_str) else { continue };
            let Some(strokes) = l.get("strokes").and_then(Value::as_array) else { continue };
            for s in strokes.iter().take(MAX_STROKES) {
                if let Ok(s) = serde_json::from_value::<RefineStroke>(s.clone()) {
                    out.push(key, s);
                }
            }
        }
        out
    }
}

impl<'de> Deserialize<'de> for Refine {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Refine, D::Error> {
        Ok(Refine::from_value(&Value::deserialize(d)?))
    }
}

/// The box of `strokes`' points and their largest radius (long-edge fractions): what Add strokes can widen a mask
/// to, before the radius. `None` without points.
pub fn points_box<'a>(strokes: impl IntoIterator<Item = &'a RefineStroke>) -> Option<([f64; 4], f64)> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut r: f64 = 0.0;
    for s in strokes {
        for (x, y) in &s.points {
            b = [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)];
        }
        r = r.max(s.size);
    }
    (b[0] <= b[2] && b[1] <= b[3]).then_some((b, r))
}

/// How far from a face's box (in face sizes) an Add stroke still works on that face: a stroke further away is
/// refused by the command and ignored by a render.
pub const ADD_REACH: f64 = 1.0;

/// The middle of a stroke's points (normalised, the strokes' frame).
pub fn stroke_centre(s: &RefineStroke) -> Option<(f64, f64)> {
    let n = s.points.len();
    if n == 0 {
        return None;
    }
    let (sx, sy) = s.points.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let c = (sx / n as f64, sy / n as f64);
    (c.0.is_finite() && c.1.is_finite()).then_some(c)
}

/// How far point `(x, y)` (normalised, in `face`'s frame) is from the face's box, in the face's sizes (its longer
/// side, normalised); infinite for a face that is not a box.
pub fn add_distance(face: &crate::Face, (x, y): (f64, f64)) -> f64 {
    let b = face.bounds;
    let size = b.width().max(b.height());
    let dx = (b.x0 - x).max(x - b.x1).max(0.0);
    let dy = (b.y0 - y).max(y - b.y1).max(0.0);
    let d = (dx * dx + dy * dy).sqrt() / size;
    if d.is_finite() && size > 0.0 { d } else { f64::INFINITY }
}

/// One feature's strokes as a run sees them: the strokes, and whether they are one face's own (painted with that face
/// picked) or for every face.
#[derive(Clone, Copy)]
pub(crate) struct Layer<'a> {
    pub strokes: &'a Refine,
    pub picked: bool,
}

/// The face (its place in `scene.faces`) an Add stroke works on: `picked` for a picked face's own strokes, else the
/// nearest; `None` for a stroke that is not Add, or that is further than [`ADD_REACH`] from that face.
pub(crate) fn add_owner(scene: &crate::Scene<'_>, s: &RefineStroke, picked: Option<usize>) -> Option<usize> {
    if s.mode != RefineMode::Add {
        return None;
    }
    let (cx, cy) = stroke_centre(s)?;
    let (px, py) = scene.view().edit_to_px(cx, cy)?;
    let c = scene.view().to_norm(px, py);
    let dist = |k: usize| scene.faces.get(k).map_or(f64::INFINITY, |f| add_distance(f.face, c));
    let k = match picked {
        Some(k) => k,
        None => (0..scene.faces.len()).min_by(|a, b| dist(*a).total_cmp(&dist(*b)))?,
    };
    (dist(k) <= ADD_REACH).then_some(k)
}

/// For each face of `scene` (same order), where the Add strokes it owns (of the features that are on and read a
/// mask) can widen its masks: the box of their points and their largest radius (see [`points_box`]). A zoomed
/// render reads up to there.
pub fn add_reach(r: &crate::Retouch, input: &crate::Input, scene: &crate::Scene<'_>) -> Vec<Option<([f64; 4], f64)>> {
    let layers = crate::run::layers(r, input, scene, check_budget(r).is_ok());
    layers
        .iter()
        .enumerate()
        .map(|(k, (features, layer))| {
            let adds = crate::FEATURES
                .iter()
                .filter(|f| !f.add_parts.is_empty() && features.is_on(f.key))
                .flat_map(|f| layer.strokes.strokes(f.key))
                .filter(|s| add_owner(scene, s, layer.picked.then_some(k)) == Some(k));
            points_box(adds)
        })
        .collect()
}

/// Whether the strokes of `r` stay within what a render rasterises ([`FEATURE_WORK_CAP`] for each feature of each
/// set of strokes, [`TOTAL_WORK_CAP`] for all of them); the reason when not. Commands refuse a stroke that would
/// break it, and a render of a photo that already does (an old sidecar) leaves its Refine strokes out.
pub fn check_budget(r: &crate::Retouch) -> Result<(), String> {
    let mut total = 0.0;
    for refine in std::iter::once(&r.refine).chain(r.faces.iter().map(|f| &f.refine)) {
        for l in &refine.0 {
            let w = work(&l.strokes);
            if w > FEATURE_WORK_CAP {
                return Err(format!("the Refine strokes of {} are too many or too large to render: use fewer, shorter or smaller ones", l.feature));
            }
            total += w;
        }
    }
    if total > TOTAL_WORK_CAP {
        return Err("the Refine strokes of this photo are too many or too large to render: clear some".to_string());
    }
    Ok(())
}
