//! Fork feature (`fork/portrait-presets.md`, issue #106): portrait presets that follow the same person across photos.
//!
//! A portrait preset is a [`Preset`] whose settings carry [`KEY`]: one face's retouch settings (every feature, as
//! [`carry`] keeps them) and who that face was ([`Who`]: the person's name and a stable id when the face is named,
//! and its recognition embedding when the face index has one). A preset with nobody in it (saved from All Faces) is
//! an "any face" preset: it gives its settings to every face of the photos it is applied to. With Include Other
//! Settings the preset also holds the photo's other develop settings (a whole look), which apply as any preset's do.
//!
//! Applying it gives the settings to the same person's face on each photo, as that face's own settings
//! ([`lightcraft_develop::retouch::Retouch::faces`]); every other face keeps what it had. The face is looked up in
//! the photo's face regions in the catalog (the names, and the embeddings the face scan made ahead of time,
//! [`matching`]), so applying sets values and runs no model; the part masks come from the people parse when the
//! photo renders. Apply to Every Photo of a person is [`person`].
//!
//! A preset never carries marks made on one photo: Refine brush strokes, spots, red eye or local masks ([`carry`],
//! [`other_groups`]). Exported files and Settings Sync never carry the face embedding, and exported files not the
//! person either ([`for_file`]).

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

pub mod matching;
pub mod person;

pub use matching::{Matcher, Miss, Target};

/// The key a portrait preset's settings keep its face part under (a namespace of this fork's own: a build without
/// it applies the rest of the preset and leaves faces alone).
pub const KEY: &str = "fork_portrait";

/// The preset group portrait presets are saved in by default.
pub const GROUP: &str = "Portrait";

/// Most numbers an embedding read from a preset may have (models give 128 to 512).
const MAX_DIM: usize = 4096;

/// Longest person name kept (as `faces.setName` allows).
const MAX_NAME: usize = 200;

/// A face's recognition embedding, and the model that made it (an embedding of another model is never compared).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    pub model: String,
    pub vector: Vec<f32>,
}

/// Who a face is: the person's name (trimmed, lower case) and the preset's id for them when named, and its embedding
/// when known.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Who {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// Stays with the preset when the person is renamed (`faces.renamePerson` moves `person` along, [`rename_person`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Embedding>,
}

impl Who {
    /// A person known by name only.
    pub fn named(name: &str) -> Who {
        Who { person: clean_name(name), ..Who::default() }
    }

    /// Someone in particular (a name or an embedding); `false` for an "any face" preset.
    pub fn is_known(&self) -> bool {
        self.person.is_some() || self.embedding.as_ref().is_some_and(|e| !e.vector.is_empty())
    }
}

/// `name` trimmed and in lower case, as faces are matched by; `None` for no name or a name too long to be one.
pub fn clean_name(name: &str) -> Option<String> {
    let n = name.trim().to_lowercase();
    (!n.is_empty() && n.chars().count() <= MAX_NAME).then_some(n)
}

/// A new id for a person in a preset.
fn new_person_id(name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).hash(&mut h);
    std::process::id().hash(&mut h);
    format!("person-{:016x}", h.finish())
}

/// A portrait preset's face part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Portrait {
    /// The face's settings as the preset carries them ([`carry`]): per feature, the values a preset may hold.
    pub features: Value,
    pub who: Who,
}

impl Portrait {
    /// The face part of `features` for `who`. A named person gets an id here.
    pub fn of(features: &Features, mut who: Who) -> Portrait {
        if let Some(n) = &who.person
            && who.person_id.is_none()
        {
            who.person_id = Some(new_person_id(n));
        }
        Portrait { features: carry(&serde_json::to_value(features).unwrap_or(Value::Null)), who }
    }

    /// An "any face" preset: it goes to every face of the photos it is applied to.
    pub fn is_any_face(&self) -> bool {
        !self.who.is_known()
    }

    pub fn to_value(&self) -> Value {
        let mut v = json!({"features": self.features});
        if let (Some(o), Ok(Value::Object(who))) = (v.as_object_mut(), serde_json::to_value(&self.who)) {
            o.extend(who);
        }
        v
    }

    /// The face part from a preset's JSON, leniently: what a preset may not carry is dropped, a malformed identity
    /// reads as unknown. `None` when `v` is not an object.
    pub fn from_value(v: &Value) -> Option<Portrait> {
        let o = v.as_object()?;
        let features = carry(o.get("features").unwrap_or(&Value::Null));
        let person = o.get("person").and_then(Value::as_str).and_then(clean_name);
        let person_id = o.get("personId").and_then(Value::as_str).filter(|i| !i.is_empty() && i.len() <= 64).map(str::to_string);
        Some(Portrait { features, who: Who { person, person_id, embedding: o.get("embedding").and_then(embedding_of) } })
    }
}

/// An embedding from a preset's JSON: its length is checked before anything is copied.
fn embedding_of(e: &Value) -> Option<Embedding> {
    let model = e.get("model")?.as_str().filter(|m| !m.is_empty() && m.len() <= 512)?;
    let a = e.get("vector")?.as_array().filter(|a| !a.is_empty() && a.len() <= MAX_DIM)?;
    let vector = a.iter().map(|x| x.as_f64().map(|x| x as f32).filter(|x| x.is_finite())).collect::<Option<Vec<f32>>>()?;
    Some(Embedding { model: model.to_string(), vector })
}

/// Field names a preset never carries, whatever their value: marks made on one photo (Refine brush strokes, spots,
/// masks drawn on it).
const NEVER: [&str; 6] = ["stroke", "spot", "mark", "brush", "refine", "mask"];

/// Most numbers in one carried list (a colour is three).
const MAX_LIST: usize = 4;

/// What a portrait preset carries of a face's settings (`Features` as JSON), and nothing else: per feature, the
/// fields that are a number (a slider), a switch, or a short list of numbers (a colour), unless the field's name
/// says it is a mark made on one photo ([`NEVER`]). Lists of points or shapes (strokes, spots) and nested settings
/// are dropped, so a feature that adds them never takes them into a preset.
pub fn carry(features: &Value) -> Value {
    let mut out = Map::new();
    for (feature, fields) in features.as_object().into_iter().flatten() {
        let Some(fields) = fields.as_object() else { continue };
        let kept: Map<String, Value> = fields.iter().filter(|(k, v)| carried(k, v)).map(|(k, v)| (k.clone(), v.clone())).collect();
        out.insert(feature.clone(), Value::Object(kept));
    }
    Value::Object(out)
}

fn carried(name: &str, v: &Value) -> bool {
    let lower = name.to_lowercase();
    if NEVER.iter().any(|w| lower.contains(w)) {
        return false;
    }
    match v {
        Value::Number(_) | Value::Bool(_) => true,
        Value::Array(a) => a.len() <= MAX_LIST && a.iter().all(Value::is_number),
        _ => false,
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

/// `preset` as it leaves this Mac: never with the face's embedding; with the person (name and id) only when `person`
/// (Settings Sync to the same user's other Macs), not in an exported file. A preset without a face part as it is.
pub fn for_file(preset: &Preset, person: bool) -> Cow<'_, Preset> {
    let Some(Value::Object(part)) = preset.settings.get(KEY) else { return Cow::Borrowed(preset) };
    let mut part = part.clone();
    part.remove("embedding");
    if !person {
        part.remove("person");
        part.remove("personId");
    }
    let mut out = preset.clone();
    if let Some(o) = out.settings.as_object_mut() {
        o.insert(KEY.to_string(), Value::Object(part));
    }
    Cow::Owned(out)
}

/// After `faces.renamePerson` renamed (or merged) `from` into `to`: portrait presets that follow `from` follow `to`,
/// keeping their person id. A person named `from` later is someone new and does not get them. Returns how many.
pub fn rename_person(presets: &mut crate::presets::PresetList, from: &str, to: &str) -> usize {
    let (Some(from), Some(to)) = (clean_name(from), clean_name(to)) else { return 0 };
    let follows =
        |p: &Preset| p.settings.get(KEY).and_then(|v| v.get("person")).and_then(Value::as_str).and_then(clean_name).as_deref() == Some(from.as_str());
    // (the list is written to disk when it changes: touch it only when a preset follows them)
    if from == to || !presets.iter().any(follows) {
        return 0;
    }
    let mut n = 0;
    for p in presets.iter_mut() {
        let Some(part) = p.settings.get_mut(KEY).and_then(Value::as_object_mut) else { continue };
        if part.get("person").and_then(Value::as_str).and_then(clean_name).as_deref() == Some(from.as_str()) {
            part.insert("person".into(), Value::String(to.clone()));
            n += 1;
        }
    }
    n
}

/// The groups of other develop settings a portrait preset may carry: `groups` without Portrait (the face part
/// replaces it), Spots, Red Eye and Masks (marks and masks made on one photo).
pub fn other_groups(groups: &[SettingsGroup]) -> Vec<SettingsGroup> {
    groups
        .iter()
        .copied()
        .filter(|g| !matches!(g, SettingsGroup::Portrait | SettingsGroup::Spots | SettingsGroup::RedEye | SettingsGroup::Masks))
        .collect()
}

/// A portrait preset (no id yet: [`Session::add_presets`] gives one) of `portrait`, with `others` (the photo's
/// other develop settings, as [`lightcraft_develop::presets::extract_groups`] gives them).
pub fn make(name: &str, group: &str, portrait: &Portrait, others: Value) -> Preset {
    let mut settings = match others {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    for k in ["masks", "spots", "red_eye", lightcraft_develop::retouch::KEY] {
        settings.remove(k);
    }
    settings.insert(KEY.to_string(), portrait.to_value());
    Preset { id: String::new(), name: name.to_string(), group: group.to_string(), settings: Value::Object(settings), favorite: false, builtin: false }
}

/// `to` (carried values) at `amount` of the way from `from` (1 = `to`): numbers move, other values switch at half,
/// as a preset's amount moves develop settings. What `to` does not carry keeps `from`'s value.
fn blend(from: &Features, to: &Value, amount: f64) -> Features {
    let Ok(a) = serde_json::to_value(from) else { return from.clone() };
    Features::from_value(&blend_value(&a, to, amount.clamp(0.0, 2.0)))
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

/// Give face `target.index` of `target.faces` `features` (carried values, at `amount`) as its own settings in `d`.
/// False when it can't have its own (too many faces already have theirs).
pub fn set_face(d: &mut DevelopSettings, target: &Target, features: &Value, amount: f64, map: Option<&dyn KeyMap>) -> bool {
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

/// An "any face" preset's `features` (at `amount`) for every face in `d`: the settings for every face, and each
/// face's own.
pub fn set_all_faces(d: &mut DevelopSettings, features: &Value, amount: f64) {
    d.retouch.global = blend(&d.retouch.global, features, amount);
    for o in &mut d.retouch.faces {
        o.settings = blend(&o.settings, features, amount);
    }
}

/// Photo `p`'s faces for matching and keying: its face regions (named, with their embeddings), or, on a photo the
/// face scan has not looked at, the faces portrait retouching found (without embeddings or masks).
pub(crate) fn faces_of(s: &Session, p: &Photo) -> (Vec<Face>, Vec<Option<Arc<[f32]>>>) {
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
            person: r.name.as_deref().and_then(clean_name),
            // the face scan looks at the photo with default settings: before lens corrections and transform
            key_frame: KeyFrame::Photo,
            ..Face::boxed(upright)
        });
        embeddings.push(matching::recognition::embedding(s, p.id, &r.rect));
    }
    if faces.is_empty()
        && let Some(found) = s.retouch_faces_of(p.id).found
    {
        // the box, the person and the frame: never the masks
        faces = found
            .input
            .faces
            .iter()
            .map(|f| Face { bounds: f.bounds, upright: f.upright, person: f.person.clone(), key_frame: f.key_frame, ..Face::boxed(f.bounds) })
            .collect();
        embeddings = vec![None; faces.len()];
    }
    (faces, embeddings)
}

impl Session {
    /// Photo `id`'s settings with `portrait` given (at `amount`, on top of `d`) to the person's face found by `m`, or
    /// to every face for an "any face" preset.
    pub fn portrait_onto(&self, id: PhotoId, d: &mut DevelopSettings, portrait: &Portrait, m: &Matcher, amount: f64) -> Result<(), Miss> {
        if portrait.is_any_face() {
            set_all_faces(d, &portrait.features, amount);
            return Ok(());
        }
        let target = m.find(self, id)?;
        let keys = self.retouch_key_map(id);
        if set_face(d, &target, &portrait.features, amount, keys.as_ref().map(|k| k as &dyn KeyMap)) { Ok(()) } else { Err(Miss::Full) }
    }

    /// Who `face` (one of photo `id`'s faces as `retouch.faces` lists them) is: its name, and the embedding of the
    /// face region it lies on.
    pub fn portrait_who(&self, id: PhotoId, face: &Face) -> Who {
        let mut who = Who { person: face.person.clone(), ..Who::default() };
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
                who.person = r.name.as_deref().and_then(clean_name);
            }
            if let (Some(e), Some(model)) = (matching::recognition::embedding(self, id, &r.rect), matching::recognition::tag(self)) {
                who.embedding = Some(Embedding { model, vector: e.to_vec() });
            }
        }
        who
    }
}

fn iou(a: &NormRect, b: &NormRect) -> f64 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let inter = w * h;
    let union = a.width().abs() * a.height().abs() + b.width().abs() * b.height().abs() - inter;
    if union > 0.0 && inter.is_finite() { inter / union } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field that holds marks made on one photo never enters a preset, whatever feature adds it; sliders, switches
    /// and colours do.
    #[test]
    fn a_preset_carries_values_never_strokes_spots_or_masks() {
        let face = json!({
            "skin": {"smoothness": 40.0, "refineStrokes": [{"x": 0.1, "y": 0.2, "r": 0.01}], "strokes": [[0.1, 0.2]]},
            "blemish": {"amount": 30.0, "keep_moles": true, "spots": [[0.5, 0.5, 0.01]], "spotList": [1.0, 2.0]},
            "teeth_lips": {"lip_color": [0.8, 0.2, 0.3], "brush": 1.0, "maskEdge": 0.5},
            "eyes": {"whites": 10.0, "nested": {"a": 1.0}, "long": [1.0, 2.0, 3.0, 4.0, 5.0], "label": "x"},
        });
        let c = carry(&face);
        assert_eq!(
            c,
            json!({
                "skin": {"smoothness": 40.0},
                "blemish": {"amount": 30.0, "keep_moles": true},
                "teeth_lips": {"lip_color": [0.8, 0.2, 0.3]},
                "eyes": {"whites": 10.0},
            })
        );
        // and a preset read from a file is held to it too
        let p = Portrait::from_value(&json!({"features": face, "embedding": {"model": "m", "vector": vec![0.5; MAX_DIM + 1]}})).unwrap();
        assert_eq!(p.features, c);
        assert_eq!(p.who.embedding, None, "an embedding past the length is never read");
    }

    /// Applying keeps what the preset does not carry: the face's own marks stay on its photo.
    #[test]
    fn applying_keeps_the_faces_own_values_the_preset_does_not_carry() {
        let mut from = Features::default();
        from.set("retouch.skin.smoothness", 10.0);
        from.set("retouch.eyes.whites", 50.0);
        let next = blend(&from, &json!({"skin": {"smoothness": 70.0}}), 1.0);
        assert_eq!((next.get("retouch.skin.smoothness"), next.get("retouch.eyes.whites")), (Some(70.0), Some(50.0)));
    }
}
