//! Fork feature (`fork/portrait-presets.md`, issue #106): portrait presets that follow the same person across photos.
//!
//! A portrait preset is a [`Preset`] whose settings carry [`KEY`]: one face's whole retouch settings ([`Features`],
//! every feature as serde writes it, so a feature added later is carried with no change here) and who that face was
//! ([`Who`]: the person's name when the face is named, and its recognition embedding when the face index has one).
//! The rest of the preset's settings, when it was saved with the photo's other develop settings, apply as any
//! preset's do.
//!
//! Applying it gives the settings to the same person's face on each photo, as that face's own settings
//! ([`lightcraft_develop::retouch::Retouch::faces`]); every other face keeps what it had. The face is looked up in
//! the photo's face regions in the catalog (the names, and the embeddings the face scan made ahead of time), so
//! applying sets values and runs no model; the part masks come from the people parse when the photo renders.
//!
//! Refine brush strokes and spots are not part of a preset: a preset carries [`Features`] (sliders), and the
//! other develop settings it may carry never include Spots or Red Eye ([`other_groups`]).

use std::borrow::Cow;
use std::sync::Arc;

use lightcraft_catalog::{Photo, PhotoId};
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::presets::{Preset, SettingsGroup};
use lightcraft_develop::retouch::{Face, Features, KeyFrame, KeyMap, NormRect};
use lightcraft_meta::RegionKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::Session;

/// The key a portrait preset's settings keep its face part under (a namespace of this fork's own: a build without
/// it applies the rest of the preset and leaves faces alone).
pub const KEY: &str = "fork_portrait";

/// The preset group portrait presets are saved in by default.
pub const GROUP: &str = "Portrait";

/// How alike (cosine) two faces' embeddings must be to count as one person, when the recognition model gives no
/// threshold of its own.
const DEFAULT_THRESHOLD: f32 = 0.45;

/// Most of a person's named faces a match compares against (enough for every pose and age; a person named on
/// 10 000 photos would otherwise cost a pass over all of them per face).
const MAX_REFS: usize = 512;

/// A face's recognition embedding, and the model that made it (an embedding of another model is never compared).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    pub model: String,
    pub vector: Vec<f32>,
}

/// Who a face is: the person's name (trimmed, lower case) when named, and its embedding when known.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Who {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Embedding>,
}

impl Who {
    /// A person known by name only.
    pub fn named(name: &str) -> Who {
        Who { person: Some(name.trim().to_lowercase()).filter(|n| !n.is_empty()), embedding: None }
    }

    pub fn is_known(&self) -> bool {
        self.person.is_some() || self.embedding.as_ref().is_some_and(|e| !e.vector.is_empty())
    }
}

/// A portrait preset's face part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Portrait {
    /// The face's settings, every feature.
    pub features: Features,
    pub who: Who,
}

/// Most numbers an embedding read from a preset may have (models give 128 to 512).
const MAX_DIM: usize = 4096;

impl Portrait {
    pub fn to_value(&self) -> Value {
        let mut v = json!({"features": self.features});
        if let (Some(o), Ok(Value::Object(who))) = (v.as_object_mut(), serde_json::to_value(&self.who)) {
            o.extend(who);
        }
        v
    }

    /// The face part from a preset's JSON, leniently: malformed features read as off, a malformed identity as
    /// unknown. `None` when `v` is not an object.
    pub fn from_value(v: &Value) -> Option<Portrait> {
        let o = v.as_object()?;
        let features = Features::from_value(o.get("features").unwrap_or(&Value::Null));
        let person = o.get("person").and_then(Value::as_str).map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty() && n.chars().count() <= 200);
        let embedding = o
            .get("embedding")
            .and_then(|e| serde_json::from_value::<Embedding>(e.clone()).ok())
            .filter(|e| !e.vector.is_empty() && e.vector.len() <= MAX_DIM && e.vector.iter().all(|x| x.is_finite()));
        Some(Portrait { features, who: Who { person, embedding } })
    }
}

/// Whether `preset` is a portrait preset.
pub fn is_portrait(preset: &Preset) -> bool {
    preset.settings.get(KEY).is_some_and(Value::is_object)
}

/// The preset without its face part (what applies as any preset does), and the face part.
pub fn split(preset: &Preset) -> (Cow<'_, Preset>, Option<Portrait>) {
    let Some(part) = preset.settings.get(KEY) else { return (Cow::Borrowed(preset), None) };
    let portrait = Portrait::from_value(part);
    let mut rest = preset.clone();
    if let Some(o) = rest.settings.as_object_mut() {
        o.remove(KEY);
    }
    (Cow::Owned(rest), portrait)
}

/// The groups of other develop settings a portrait preset may carry: `groups` without Portrait (the face part
/// replaces it), Spots and Red Eye (marks on one photo).
pub fn other_groups(groups: &[SettingsGroup]) -> Vec<SettingsGroup> {
    groups.iter().copied().filter(|g| !matches!(g, SettingsGroup::Portrait | SettingsGroup::Spots | SettingsGroup::RedEye)).collect()
}

/// A portrait preset (no id yet: [`Session::add_presets`] gives one) of `portrait`, with `others` (the photo's
/// other develop settings, as [`lightcraft_develop::presets::extract_groups`] gives them).
pub fn make(name: &str, group: &str, portrait: &Portrait, others: Value) -> Preset {
    let mut settings = match others {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    settings.insert(KEY.to_string(), portrait.to_value());
    Preset { id: String::new(), name: name.to_string(), group: group.to_string(), settings: Value::Object(settings), favorite: false, builtin: false }
}

/// Why a photo did not get the face settings.
#[derive(Clone, Debug, PartialEq)]
pub enum Miss {
    /// No face found on it (yet).
    NoFaces,
    /// None of its faces is the person.
    NotThere,
}

impl Miss {
    pub fn as_str(&self) -> &'static str {
        match self {
            Miss::NoFaces => "no faces found on the photo yet",
            Miss::NotThere => "the person is not on the photo",
        }
    }
}

/// One photo's faces as matching sees them, and which one is the person.
pub struct Target {
    pub faces: Vec<Face>,
    pub index: usize,
}

/// Finds the person on photos: by name, else by how alike the faces' embeddings are.
pub struct Matcher {
    who: Who,
    /// Embeddings the person is known by: the preset's own (when made by the model in use) and their named faces'.
    refs: Vec<Arc<[f32]>>,
    threshold: f32,
    /// A photo with one face gives it the settings when nothing says it is someone else.
    lone: bool,
}

impl Matcher {
    /// A matcher for `who`. `library`: also compare with the faces named after them in the library (a pass over
    /// the catalog: once per command, not per photo). `lone`: a photo's only face counts as the person unless it is
    /// named as someone else.
    pub fn new(s: &Session, who: &Who, library: bool, lone: bool) -> Matcher {
        let mut refs = Vec::new();
        let tag = recognition::tag(s);
        if let (Some(e), Some(tag)) = (&who.embedding, &tag)
            && e.model == *tag
        {
            refs.push(Arc::from(e.vector.as_slice()));
        }
        if library && let Some(name) = &who.person {
            'photos: for ph in s.catalog.photos().filter(|ph| ph.in_library()) {
                for r in ph.meta.regions.iter().filter(|r| r.kind == RegionKind::Face) {
                    if r.name.as_deref().is_some_and(|n| n.trim().to_lowercase() == *name)
                        && let Some(e) = recognition::embedding(s, ph.id, &r.rect)
                    {
                        refs.push(e);
                        if refs.len() >= MAX_REFS {
                            break 'photos;
                        }
                    }
                }
            }
        }
        Matcher { who: who.clone(), refs, threshold: recognition::threshold(s), lone }
    }

    /// The person's face on photo `id`.
    pub fn find(&self, s: &Session, id: PhotoId) -> Result<Target, Miss> {
        let p = s.catalog.photo(id).ok_or(Miss::NoFaces)?;
        let (faces, embeddings) = faces_of(s, p);
        if faces.is_empty() {
            return Err(Miss::NoFaces);
        }
        let index = self.pick(&faces, &embeddings).ok_or(Miss::NotThere)?;
        Ok(Target { faces, index })
    }

    fn pick(&self, faces: &[Face], embeddings: &[Option<Arc<[f32]>>]) -> Option<usize> {
        let me = self.who.person.as_deref();
        // 1. named after them
        if let Some(name) = me
            && let Some(i) = faces.iter().position(|f| f.person.as_deref() == Some(name))
        {
            return Some(i);
        }
        // 2. the face most like them, never one named as someone else
        let mut best: Option<(f32, usize)> = None;
        if !self.refs.is_empty() {
            let refs: Vec<&[f32]> = self.refs.iter().map(|r| &r[..]).collect();
            for (i, f) in faces.iter().enumerate() {
                if f.person.is_some() && f.person.as_deref() != me {
                    continue;
                }
                let Some(e) = embeddings.get(i).and_then(Option::as_ref) else { continue };
                let Some(score) = lightcraft_faces::matching::person_score(e, &refs, lightcraft_faces::matching::TOP_FACES) else { continue };
                if score >= self.threshold && best.is_none_or(|(b, _)| score > b) {
                    best = Some((score, i));
                }
            }
        }
        if let Some((_, i)) = best {
            return Some(i);
        }
        // 3. the photo's only face, unless it is someone else
        if self.lone
            && let [only] = faces
            && (only.person.is_none() || only.person.as_deref() == me)
        {
            return Some(0);
        }
        None
    }
}

/// Photo `p`'s faces for matching and keying: its face regions (named, with their embeddings), or, on a photo the
/// face scan has not looked at, the faces portrait retouching found (without embeddings).
fn faces_of(s: &Session, p: &Photo) -> (Vec<Face>, Vec<Option<Arc<[f32]>>>) {
    let mut faces = Vec::new();
    let mut embeddings = Vec::new();
    for r in p.meta.regions.iter().filter(|r| r.kind == RegionKind::Face) {
        let upright = NormRect::new(r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1);
        if !upright.is_valid() {
            continue;
        }
        let b = p.develop.orientation.map_norm_rect(r.rect);
        faces.push(Face {
            bounds: NormRect::new(b.x0, b.y0, b.x1, b.y1),
            upright: Some(upright),
            person: r.name.as_deref().map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()),
            // the face scan looks at the photo with default settings: before lens corrections and transform
            key_frame: KeyFrame::Photo,
            ..Face::boxed(upright)
        });
        embeddings.push(recognition::embedding(s, p.id, &r.rect));
    }
    if faces.is_empty()
        && let Some(found) = s.retouch_faces_of(p.id).found
    {
        faces = found.input.faces.iter().map(|f| Face { masks: Vec::new(), ..f.clone() }).collect();
        embeddings = vec![None; faces.len()];
    }
    (faces, embeddings)
}

/// `to` at `amount` of the way from `from` (1 = `to`): numbers move, other values switch at half, as a preset's
/// amount moves develop settings.
fn blend(from: &Features, to: &Features, amount: f64) -> Features {
    if (amount - 1.0).abs() < 1e-9 {
        return to.clone();
    }
    let (Ok(a), Ok(b)) = (serde_json::to_value(from), serde_json::to_value(to)) else { return to.clone() };
    Features::from_value(&blend_value(&a, &b, amount))
}

fn blend_value(a: &Value, b: &Value, t: f64) -> Value {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let mut out = a.clone();
            for (k, bv) in b {
                let v = match a.get(k) {
                    Some(av) => blend_value(av, bv, t),
                    None => bv.clone(),
                };
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        // integers (a choice, a count) are discrete, like strings
        (Value::Number(x), Value::Number(y)) if x.is_f64() || y.is_f64() => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            serde_json::Number::from_f64(x + (y - x) * t).map_or_else(|| b.clone(), Value::Number)
        }
        _ if t >= 0.5 => b.clone(),
        _ => a.clone(),
    }
}

/// Give face `target.index` of `target.faces` `features` (at `amount`) as its own settings in `d`. False when it
/// can't have its own (too many faces already have theirs).
pub fn set_face(d: &mut DevelopSettings, target: &Target, features: &Features, amount: f64, map: Option<&dyn KeyMap>) -> bool {
    let from = d.retouch.settings_for_in(&target.faces, target.index, map).clone();
    let next = blend(&from, features, amount);
    match d.retouch.override_mut_in(&target.faces, target.index, map) {
        Some(own) => {
            *own = next;
            true
        }
        None => false,
    }
}

impl Session {
    /// Photo `id`'s settings with `portrait` given to the person's face (found by `m`) at `amount`, on top of `d`.
    pub fn portrait_onto(&self, id: PhotoId, d: &mut DevelopSettings, portrait: &Portrait, m: &Matcher, amount: f64) -> Result<(), Miss> {
        let target = m.find(self, id)?;
        let keys = self.retouch_key_map(id);
        if set_face(d, &target, &portrait.features, amount, keys.as_ref().map(|k| k as &dyn KeyMap)) { Ok(()) } else { Err(Miss::NotThere) }
    }

    /// Who `face` (one of photo `id`'s faces as `retouch.faces` lists them) is: its name, and the embedding of the
    /// face region it lies on.
    pub fn portrait_who(&self, id: PhotoId, face: &Face) -> Who {
        let mut who = Who { person: face.person.clone(), embedding: None };
        let Some(p) = self.catalog.photo(id) else { return who };
        let key = face.key_bounds();
        let best = p
            .meta
            .regions
            .iter()
            .filter(|r| r.kind == RegionKind::Face)
            .map(|r| (iou(&key, &NormRect::new(r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1)), r))
            .filter(|(o, _)| *o >= 0.3)
            .max_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, r)) = best {
            if who.person.is_none() {
                who.person = r.name.as_deref().map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty());
            }
            if let (Some(e), Some(model)) = (recognition::embedding(self, id, &r.rect), recognition::tag(self)) {
                who.embedding = Some(Embedding { model, vector: e.to_vec() });
            }
        }
        who
    }

    /// Who the only face of photo `id` is, when it has exactly one (found by portrait retouching, else by the face
    /// scan); else nobody.
    pub fn portrait_lone_who(&self, id: PhotoId) -> Who {
        if let Some(found) = self.retouch_faces_of(id).found {
            return match found.input.faces.as_slice() {
                [only] => self.portrait_who(id, only),
                _ => Who::default(),
            };
        }
        let Some(p) = self.catalog.photo(id) else { return Who::default() };
        match faces_of(self, p).0.as_slice() {
            [only] => self.portrait_who(id, only),
            _ => Who::default(),
        }
    }
}

fn iou(a: &NormRect, b: &NormRect) -> f64 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let inter = w * h;
    let union = a.width().abs() * a.height().abs() + b.width().abs() * b.height().abs() - inter;
    if union > 0.0 && inter.is_finite() { inter / union } else { 0.0 }
}

/// The face index (recognition embeddings), where this build has one.
#[cfg(not(target_arch = "wasm32"))]
mod recognition {
    use std::sync::Arc;

    use lightcraft_catalog::PhotoId;
    use lightcraft_geom::Rect;

    use crate::Session;

    /// The model the index's embeddings were made by; `None` while the index holds none.
    pub fn tag(s: &Session) -> Option<String> {
        Some(s.faces.index.tag.clone()).filter(|t| !t.is_empty())
    }

    pub fn embedding(s: &Session, id: PhotoId, rect: &Rect) -> Option<Arc<[f32]>> {
        s.faces.index.get(id.0, rect).cloned()
    }

    pub fn threshold(s: &Session) -> f32 {
        s.faces
            .embedder
            .as_ref()
            .and_then(|(_, e)| e.manifest().thresholds.match_cosine)
            .filter(|t| t.is_finite() && *t > 0.0)
            .unwrap_or(super::DEFAULT_THRESHOLD)
    }
}

#[cfg(target_arch = "wasm32")]
mod recognition {
    use std::sync::Arc;

    use lightcraft_catalog::PhotoId;
    use lightcraft_geom::Rect;

    use crate::Session;

    pub fn tag(_: &Session) -> Option<String> {
        None
    }

    pub fn embedding(_: &Session, _: PhotoId, _: &Rect) -> Option<Arc<[f32]>> {
        None
    }

    pub fn threshold(_: &Session) -> f32 {
        super::DEFAULT_THRESHOLD
    }
}
