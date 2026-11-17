//! Fork (issue #288, `fork/relight.md`): Relight, a light placed in the photo that lights it by depth. Its settings
//! and its controls (section [`Section::Relight`], ids `relight.<field>` and, for each light, `relight.<n>.<field>`).
//!
//! The settings are plain numbers in the units the controls show; `lightcraft-pipeline` turns them into the light
//! (`lightcraft_relight::Light`). The light's place is in normalized photo coordinates (before the crop and the lens
//! corrections, like a mask's) and its depth between the photo's nearest and farthest, so a draft, the loupe and an
//! export make the same picture, and undo, presets, copy settings, sync, the control channel and MCP reach it as
//! they reach any slider.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::controls::{ControlSpec, Section, Track};
use crate::relight_effects::LightEffects;
pub use crate::relight_kinds::KindSettings;

/// The settings' JSON key (a namespace of this fork's own: a build without it ignores it).
pub const KEY: &str = "fork_relight";

/// The section id (its eye, `section.relight`).
pub const SECTION: &str = "relight";

/// The most lights Relight holds (`lightcraft_relight::MAX_LIGHTS`, which the pipeline checks it against).
pub const MAX_LIGHTS: usize = 8;

/// One light's settings (issue #299). Later parts add fields here (a kind, a gel, a pattern): each takes a default
/// and is read leniently, so a block written before it, or by another build, still reads.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightSettings {
    /// How strong the light is, 0..100 (0: it adds nothing).
    pub amount: f64,
    /// How wide and gentle its falloff is, 0 (a tight pool) .. 100 (broad, window-like light).
    pub softness: f64,
    /// Its colour, −100 (cool) .. 0 (neutral) .. 100 (warm).
    pub warmth: f64,
    /// Where it is: normalized photo coordinates, 0..1 (may lie a little outside the photo: −0.5..1.5).
    pub x: f64,
    pub y: f64,
    /// How far away, 0 (in front of the nearest thing in the photo) .. 100 (at the farthest).
    pub depth: f64,
    /// Advanced: how far the light wraps around shapes, 0 (follows them closely) .. 100 (ignores them).
    pub wrap: f64,
    /// What the light is and the numbers of its type (issue #300; a Point, as before, by default).
    #[serde(flatten)]
    pub kind: KindSettings,
    /// Its gel, pattern and haze (issue #302); written only where one differs from its default.
    #[serde(flatten)]
    pub effects: LightEffects,
}

impl Default for LightSettings {
    fn default() -> Self {
        Self {
            amount: 0.0,
            softness: 50.0,
            warmth: 0.0,
            x: 0.35,
            y: 0.3,
            depth: 30.0,
            wrap: 40.0,
            kind: KindSettings::default(),
            effects: LightEffects::default(),
        }
    }
}

/// Relight's settings: up to [`MAX_LIGHTS`] lights (at least one) and Darken Surroundings, which belongs to the whole
/// photo (what no light reaches gets darker). See the module docs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Relight {
    /// The lights, in order (the controls' `relight.<n>.…` index them from 0).
    pub lights: Vec<LightSettings>,
    /// Advanced: how much the rest of the photo darkens, 0..100.
    pub darken: f64,
}

impl Default for Relight {
    fn default() -> Self {
        Self { lights: vec![LightSettings::default()], darken: 0.0 }
    }
}

/// What a stored light may hold (every field optional; unknown ones ignored). The first form of the block (before
/// issue #299) was one such object with `darken` beside the others.
#[derive(Deserialize)]
#[serde(default)]
struct Stored {
    amount: f64,
    softness: f64,
    warmth: f64,
    x: f64,
    y: f64,
    depth: f64,
    darken: f64,
    wrap: f64,
    #[serde(flatten)]
    effects: LightEffects,
}

impl Default for Stored {
    fn default() -> Self {
        let d = LightSettings::default();
        Self {
            amount: d.amount,
            softness: d.softness,
            warmth: d.warmth,
            x: d.x,
            y: d.y,
            depth: d.depth,
            darken: 0.0,
            wrap: d.wrap,
            effects: d.effects,
        }
    }
}

impl Stored {
    /// The light these numbers and `stored`'s type fields make.
    fn light(&self, stored: &Value) -> LightSettings {
        let mut l = LightSettings {
            amount: self.amount,
            softness: self.softness,
            warmth: self.warmth,
            x: self.x,
            y: self.y,
            depth: self.depth,
            wrap: self.wrap,
            kind: KindSettings::read(stored),
            effects: self.effects,
        };
        l.sanitize();
        l
    }
}

/// Read leniently: a malformed block (another build's, a hand-edited sidecar, an agent) reads as off, and each
/// number is kept within its control's range (a value that is not a number takes its default). A block of the first
/// form (one light's numbers and `darken`) reads as one light; a block with `lights` keeps at most [`MAX_LIGHTS`] of
/// them (a malformed light reads as a default one, so the others keep their places) and at least one.
impl<'de> Deserialize<'de> for Relight {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let Some(object) = v.as_object() else { return Ok(Relight::default()) };
        let mut r = match object.get("lights") {
            None => {
                let Ok(s) = serde_json::from_value::<Stored>(v.clone()) else { return Ok(Relight::default()) };
                Relight { lights: vec![s.light(&v)], darken: s.darken }
            }
            Some(Value::Array(list)) => {
                let lights: Vec<LightSettings> = list
                    .iter()
                    .take(MAX_LIGHTS)
                    .map(|l| serde_json::from_value::<Stored>(l.clone()).map(|s| s.light(l)).unwrap_or_default())
                    .collect();
                let darken = match object.get("darken") {
                    None => 0.0,
                    Some(n) => n.as_f64().unwrap_or(0.0),
                };
                Relight { lights, darken }
            }
            Some(_) => return Ok(Relight::default()),
        };
        r.sanitize();
        Ok(r)
    }
}

impl LightSettings {
    /// Every number within its control's range.
    pub fn sanitize(&mut self) {
        for spec in all().filter(|c| c.id != "relight.darken") {
            if let Some(v) = spec.id.strip_prefix("relight.").and_then(|f| self.field(f)) {
                *v = spec.clamp(*v);
            }
        }
        self.kind.which = self.kind.which.round();
    }

    /// The light's type was `before` (an index of [`relight_kinds::NAMES`](crate::relight_kinds::NAMES)) and has just been
    /// set: a light that becomes a Sun / Window, with Wrap still at its default, starts at Wrap 0 (a sun lights what faces it
    /// and not the rest; Wrap still works when raised).
    pub fn kind_changed(&mut self, before: usize) {
        if self.kind.index() == 2 && before != 2 && self.wrap == LightSettings::default().wrap {
            self.wrap = 0.0;
        }
    }

    fn value(&self, field: &str) -> Option<f64> {
        let mut c = self.clone();
        c.field(field).map(|v| *v)
    }

    fn field(&mut self, field: &str) -> Option<&mut f64> {
        Some(match field {
            "amount" => &mut self.amount,
            "softness" => &mut self.softness,
            "warmth" => &mut self.warmth,
            "x" => &mut self.x,
            "y" => &mut self.y,
            "depth" => &mut self.depth,
            "wrap" => &mut self.wrap,
            _ => return self.effects.field(field).or_else(|| self.kind.field(field)),
        })
    }
}

impl Relight {
    pub fn is_default(&self) -> bool {
        *self == Relight::default()
    }

    /// Whether it changes the photo (the section's eye aside): some light has an Amount, or Darken Surroundings is up.
    pub fn is_on(&self) -> bool {
        self.darken > 0.0 || self.lights.iter().any(|l| l.amount > 0.0)
    }

    /// Every number within its control's range, one to [`MAX_LIGHTS`] lights.
    pub fn sanitize(&mut self) {
        self.lights.truncate(MAX_LIGHTS);
        if self.lights.is_empty() {
            self.lights.push(LightSettings::default());
        }
        self.lights.iter_mut().for_each(LightSettings::sanitize);
        self.darken = SPECS.iter().find(|c| c.id == "relight.darken").map_or(self.darken, |c| c.clamp(self.darken));
    }

    /// The number control `id` names (`relight.amount`: the first light's; `relight.<n>.amount`: light `n`'s;
    /// `relight.darken`: the photo's). The engine's commands read `relight.amount` as the selected light's.
    fn field(&mut self, id: &str) -> Option<&mut f64> {
        let (n, field) = split(id)?;
        if field == "darken" {
            return if n.is_none() { Some(&mut self.darken) } else { None };
        }
        self.lights.get_mut(n.unwrap_or(0))?.field(field)
    }
}

/// A stored block in its first form (issue #288: one light's numbers and `darken` in one object), for replaying a log
/// written before several lights: its patches name those flat keys. `None` when the block isn't of the new form.
pub fn first_form(block: &Value) -> Option<Value> {
    let o = block.as_object()?;
    let mut flat = o.get("lights")?.as_array()?.first()?.as_object()?.clone();
    if let Some(d) = o.get("darken") {
        flat.insert("darken".into(), d.clone());
    }
    Some(Value::Object(flat))
}

/// A partial block of the first form (no `lights`: an older preset, a pasted setting) as a whole block that merges onto
/// `current`: the first light takes the numbers it names and Darken Surroundings its own. `None` when `partial` isn't
/// of the first form.
pub fn merge_first_form(current: &Relight, partial: &Value) -> Option<Value> {
    let o = partial.as_object().filter(|o| !o.contains_key("lights"))?;
    let mut r = current.clone();
    for (k, v) in o {
        let Some(n) = v.as_f64() else { continue };
        match k.as_str() {
            "darken" => r.darken = n,
            f => {
                if let Some(slot) = r.lights.first_mut().and_then(|l| l.field(f)) {
                    *slot = n;
                }
            }
        }
    }
    serde_json::to_value(r).ok()
}

/// `relight.amount` → `(None, "amount")`, `relight.2.amount` → `(Some(2), "amount")`.
fn split(id: &str) -> Option<(Option<usize>, &str)> {
    let rest = id.strip_prefix("relight.")?;
    match rest.split_once('.') {
        None => Some((None, rest)),
        Some((n, field)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => Some((Some(n.parse().ok()?), field)),
        Some(_) => None,
    }
}

const fn spec(id: &'static str, label: &'static str, (min, max, default): (f64, f64, f64), (step, decimals): (f64, u8), track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::Relight, min, max, default, step, decimals, track }
}

/// Relight's controls. The light's place (`relight.x`, `relight.y`) and depth are set by dragging its handles; they
/// are controls too, so agents and presets reach them by id.
pub static SPECS: &[ControlSpec] = &[
    spec("relight.amount", "Amount", (0.0, 100.0, 0.0), (1.0, 0), Track::Plain),
    spec("relight.softness", "Softness", (0.0, 100.0, 50.0), (1.0, 0), Track::Plain),
    spec("relight.warmth", "Warmth", (-100.0, 100.0, 0.0), (1.0, 0), Track::Temp),
    spec("relight.x", "Light Left to Right", (-0.5, 1.5, 0.35), (0.001, 3), Track::Plain),
    spec("relight.y", "Light Top to Bottom", (-0.5, 1.5, 0.3), (0.001, 3), Track::Plain),
    spec("relight.depth", "Light Depth", (0.0, 100.0, 30.0), (1.0, 0), Track::Plain),
    spec("relight.darken", "Darken Surroundings", (0.0, 100.0, 0.0), (1.0, 0), Track::Plain),
    spec("relight.wrap", "Wrap", (0.0, 100.0, 40.0), (1.0, 0), Track::Plain),
    // (issue #302: a light's gel, pattern and haze)
    spec("relight.gelHue", "Gel Hue", (0.0, 360.0, 0.0), (1.0, 0), Track::Rainbow),
    spec("relight.gelStrength", "Gel Strength", (0.0, 100.0, 0.0), (1.0, 0), Track::Plain),
    spec("relight.pattern", "Pattern", (0.0, 4.0, 0.0), (1.0, 0), Track::Plain),
    spec("relight.patternScale", "Pattern Scale", (0.0, 100.0, 50.0), (1.0, 0), Track::Plain),
    spec("relight.patternRotation", "Pattern Rotation", (-180.0, 180.0, 0.0), (1.0, 0), Track::Centered),
    spec("relight.patternSoftness", "Pattern Softness", (0.0, 100.0, 30.0), (1.0, 0), Track::Plain),
    // (issue #316: how far the slats are tilted open, how thin the mullions, how small the leaves, how many gaps)
    spec("relight.patternOpen", "Pattern Openness", (0.0, 100.0, 50.0), (1.0, 0), Track::Plain),
    spec("relight.haze", "Haze", (0.0, 100.0, 0.0), (1.0, 0), Track::Plain),
    // (issue #317: how the haze scatters the light's beams)
    spec("relight.scatter", "Forward Scatter", (0.0, 100.0, 50.0), (1.0, 0), Track::Plain),
    spec("relight.dust", "Dust", (0.0, 100.0, 0.0), (1.0, 0), Track::Plain),
    // (issue #315: how much the photo's own things block the light)
    spec("relight.shadows", "Cast Shadows", (0.0, 100.0, 0.0), (1.0, 0), Track::Plain),
];

/// Relight's control specs: the per-light ones (`relight.amount` …, templates of `relight.<n>.amount` …) and
/// `relight.darken`.
pub fn specs() -> impl Iterator<Item = &'static ControlSpec> {
    all()
}

/// The per-light specs and `relight.darken`, then the light type's (issue #300).
fn all() -> impl Iterator<Item = &'static ControlSpec> {
    SPECS.iter().chain(crate::relight_kinds::SPECS)
}

/// The spec of control `id`, when it is one of Relight's: `relight.amount` (the first light's; the engine's commands
/// take the selected light's), `relight.<n>.amount` (light `n`'s, whether or not it exists; its template spec) or
/// `relight.darken`.
pub fn find(id: &str) -> Option<&'static ControlSpec> {
    let (n, field) = split(id)?;
    if field == "darken" && n.is_some() {
        return None;
    }
    all().find(|c| c.id.strip_prefix("relight.") == Some(field))
}

/// A per-light control id read for light `selected`: `relight.amount` → `relight.<selected>.amount`; `None` for any
/// other id (`relight.darken`, `relight.2.amount`, not Relight's).
pub fn for_light(id: &str, selected: usize) -> Option<String> {
    match split(id)? {
        (None, field) if field != "darken" => Some(format!("relight.{selected}.{field}")),
        _ => None,
    }
}

/// Every per-light control of the lights `s` has (`relight.0.amount` …), with its template spec.
pub fn light_instances(s: &crate::DevelopSettings) -> Vec<(String, &'static ControlSpec)> {
    let mut out = Vec::new();
    for n in 0..s.relight.lights.len() {
        for spec in all().filter(|c| c.id != "relight.darken") {
            if let Some(field) = spec.id.strip_prefix("relight.") {
                out.push((format!("relight.{n}.{field}"), spec));
            }
        }
    }
    out
}

/// Read a Relight control (`None`: not one, or the light isn't there).
pub fn get(s: &crate::DevelopSettings, id: &str) -> Option<f64> {
    let (n, field) = split(id)?;
    if field == "darken" {
        return n.is_none().then_some(s.relight.darken);
    }
    s.relight.lights.get(n.unwrap_or(0))?.value(field)
}

/// Write a Relight control, kept within its range. False: not one, or the light isn't there.
pub fn set(s: &mut crate::DevelopSettings, id: &str, v: f64) -> bool {
    let Some(spec) = find(id) else { return false };
    let light = split(id).map(|(n, _)| n.unwrap_or(0));
    let before = light.and_then(|n| s.relight.lights.get(n)).map(|l| l.kind.index());
    match s.relight.field(id) {
        Some(f) => {
            // (the light's type is a whole number)
            *f = if spec.id == "relight.kind" { spec.clamp(v).round() } else { spec.clamp(v) };
            if spec.id == "relight.kind"
                && let (Some(n), Some(before)) = (light, before)
                && let Some(l) = s.relight.lights.get_mut(n)
            {
                l.kind_changed(before);
            }
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DevelopSettings;
    use serde_json::json;

    #[test]
    fn it_is_left_out_while_default_and_read_back_when_set() {
        let d = DevelopSettings::default();
        assert!(d.to_json().get(KEY).is_none());
        let mut s = d.clone();
        assert!(set(&mut s, "relight.amount", 60.0) && set(&mut s, "relight.x", 0.7) && set(&mut s, "relight.darken", 20.0));
        s.relight.lights.push(LightSettings { amount: 40.0, warmth: -30.0, ..LightSettings::default() });
        let v = s.to_json();
        assert_eq!(v[KEY]["lights"][0]["amount"], json!(60.0));
        assert_eq!(v[KEY]["lights"][1]["warmth"], json!(-30.0));
        assert_eq!(v[KEY]["darken"], json!(20.0));
        let back = DevelopSettings::from_json(&v).unwrap();
        assert_eq!(back.relight, s.relight);
        assert_ne!(s.hash64(), d.hash64());
        assert_eq!(get(&back, "relight.x"), Some(0.7));
        assert_eq!((get(&back, "relight.1.amount"), get(&back, "relight.2.amount")), (Some(40.0), None));
    }

    #[test]
    fn a_block_of_the_first_form_reads_as_one_light() {
        // what builds before issue #299 wrote: one light's numbers and Darken Surroundings in one object
        let v = json!({"fork_relight": {"amount": 70.0, "softness": 20.0, "warmth": 15.0, "x": 0.6, "y": 0.4, "depth": 55.0, "darken": 35.0, "wrap": 10.0}});
        let r = DevelopSettings::from_json(&v).unwrap().relight;
        let one = LightSettings { amount: 70.0, softness: 20.0, warmth: 15.0, x: 0.6, y: 0.4, depth: 55.0, wrap: 10.0, ..LightSettings::default() };
        assert_eq!(r, Relight { lights: vec![one], darken: 35.0 });
        // a field that was left out keeps its default
        let r = DevelopSettings::from_json(&json!({"fork_relight": {"amount": 70.0}})).unwrap().relight;
        assert_eq!(r.lights, vec![LightSettings { amount: 70.0, ..LightSettings::default() }]);
    }

    #[test]
    fn a_malformed_block_reads_as_off_and_numbers_stay_in_range() {
        let v = json!({"fork_relight": "nonsense"});
        assert!(DevelopSettings::from_json(&v).unwrap().relight.is_default());
        let v = json!({"fork_relight": {"amount": 1e9, "x": -40.0, "extra": 1}});
        let r = DevelopSettings::from_json(&v).unwrap().relight;
        assert_eq!((r.lights[0].amount, r.lights[0].x), (100.0, -0.5));
        let v = json!({"fork_relight": {"amount": 50.0, "warmth": "warm"}});
        assert!(DevelopSettings::from_json(&v).unwrap().relight.is_default());
        let mut s = DevelopSettings::default();
        assert!(set(&mut s, "relight.depth", f64::NAN));
        assert_eq!(s.relight.lights[0].depth, 30.0);
        assert!(!set(&mut s, "relight.nope", 1.0) && !set(&mut s, "relight.1.amount", 1.0) && !set(&mut s, "relight.0.darken", 1.0));
        // lights that aren't a list, an empty list, entries that aren't lights, and a hundred lights
        for lights in [json!("many"), json!({"a": 1})] {
            assert!(DevelopSettings::from_json(&json!({"fork_relight": {"lights": lights}})).unwrap().relight.is_default());
        }
        let empty = DevelopSettings::from_json(&json!({"fork_relight": {"lights": [], "darken": "dark"}})).unwrap().relight;
        assert!(empty.is_default());
        let odd = json!({"fork_relight": {"lights": [{"amount": 30.0}, 7, null, {"warmth": "x"}, {"amount": -5.0, "x": 9.0}], "darken": 500.0}});
        let r = DevelopSettings::from_json(&odd).unwrap().relight;
        assert_eq!((r.lights.len(), r.darken), (5, 100.0));
        assert_eq!((r.lights[0].amount, r.lights[1].clone(), r.lights[3].clone()), (30.0, LightSettings::default(), LightSettings::default()));
        assert_eq!((r.lights[4].amount, r.lights[4].x), (0.0, 1.5));
        let hundred = json!({"fork_relight": {"lights": (0..100).map(|i| json!({"amount": f64::from(i)})).collect::<Vec<_>>()}});
        let r = DevelopSettings::from_json(&hundred).unwrap().relight;
        assert_eq!(r.lights.len(), MAX_LIGHTS);
        assert_eq!(r.lights[7].amount, 7.0);
        // a list longer than the cap built in code is cut where it is sanitized
        let mut r = Relight { lights: vec![LightSettings::default(); 20], darken: f64::NAN };
        r.sanitize();
        assert_eq!(r.lights.len(), MAX_LIGHTS);
    }

    #[test]
    fn controls_reach_each_light_by_index_and_the_shorthand_the_first() {
        let mut s = DevelopSettings::default();
        s.relight.lights.push(LightSettings::default());
        assert!(set(&mut s, "relight.1.softness", 80.0) && set(&mut s, "relight.warmth", -50.0));
        assert_eq!((get(&s, "relight.1.softness"), get(&s, "relight.softness")), (Some(80.0), Some(50.0)));
        assert_eq!((get(&s, "relight.0.warmth"), get(&s, "relight.1.warmth")), (Some(-50.0), Some(0.0)));
        assert_eq!(for_light("relight.amount", 2).as_deref(), Some("relight.2.amount"));
        assert_eq!((for_light("relight.darken", 2), for_light("relight.1.amount", 2)), (None, None));
        assert_eq!(light_instances(&s).len(), 68);
        assert_eq!(crate::controls::find("relight.1.wrap").map(|c| c.id), Some("relight.wrap"));
        assert!(crate::controls::find("relight.x.wrap").is_none() && crate::controls::find("relight.0.darken").is_none());
        // a section reset puts all the lights back
        s.reset_section(crate::Section::Relight);
        assert!(s.relight.is_default());
    }

    #[test]
    fn effects_are_left_out_while_default_read_back_when_set_and_read_leniently() {
        // a light without them saves exactly as before: not one of the new keys
        let mut s = DevelopSettings::default();
        assert!(set(&mut s, "relight.amount", 60.0));
        let v = s.to_json();
        let keys: Vec<&String> = v[KEY]["lights"][0].as_object().unwrap().keys().collect();
        assert!(
            !keys.iter().any(|k| [
                "gelHue",
                "gelStrength",
                "pattern",
                "patternScale",
                "patternRotation",
                "patternSoftness",
                "patternOpen",
                "haze",
                "scatter",
                "dust",
                "shadows"
            ]
            .contains(&k.as_str())),
            "{keys:?}"
        );
        // set ones are saved by id, only those, and read back
        for (id, v) in [
            ("relight.0.gelHue", 200.0),
            ("relight.gelStrength", 70.0),
            ("relight.pattern", 3.0),
            ("relight.patternScale", 20.0),
            ("relight.haze", 35.0),
            ("relight.0.shadows", 60.0),
            ("relight.scatter", 80.0),
            ("relight.0.dust", 25.0),
        ] {
            assert!(set(&mut s, id, v), "{id}");
        }
        let v = s.to_json();
        assert_eq!(v[KEY]["lights"][0]["gelHue"], json!(200.0));
        assert!(v[KEY]["lights"][0].get("patternSoftness").is_none());
        assert_eq!(v[KEY]["lights"][0]["shadows"], json!(60.0));
        assert_eq!((&v[KEY]["lights"][0]["scatter"], &v[KEY]["lights"][0]["dust"]), (&json!(80.0), &json!(25.0)));
        let back = DevelopSettings::from_json(&v).unwrap();
        assert_eq!(back.relight, s.relight);
        assert_eq!((get(&back, "relight.0.pattern"), get(&back, "relight.patternSoftness")), (Some(3.0), Some(30.0)));
        // out of range numbers are clamped; wrong-typed ones read the light as a default one; an older block has defaults
        let r = DevelopSettings::from_json(
            &json!({"fork_relight": {"lights": [{"amount": 50.0, "gelHue": 9e9, "pattern": 77.0, "haze": -4.0, "shadows": 900.0}]}}),
        )
        .unwrap()
        .relight;
        assert_eq!((r.lights[0].effects.gel_hue, r.lights[0].effects.pattern, r.lights[0].effects.haze), (360.0, 4.0, 0.0));
        assert_eq!(r.lights[0].effects.shadows, 100.0);
        let r = DevelopSettings::from_json(&json!({"fork_relight": {"lights": [{"amount": 50.0, "gelHue": "teal"}]}})).unwrap().relight;
        assert_eq!(r.lights[0], LightSettings::default());
        assert!(DevelopSettings::from_json(&json!({"fork_relight": {"amount": 50.0}})).unwrap().relight.lights[0].effects == LightEffects::default());
        assert_eq!(LightEffects::gel("warm amber"), Some((38.0, 70.0)));
        assert_eq!(LightEffects::gel("nope"), None);
    }

    #[test]
    fn a_preset_of_the_first_form_lands_on_the_first_light() {
        let mut photo = DevelopSettings::default();
        photo.relight.lights[0] = LightSettings { amount: 40.0, x: 0.6, ..LightSettings::default() };
        photo.relight.lights.push(LightSettings { amount: 30.0, ..LightSettings::default() });
        let old = json!({"fork_relight": {"amount": 70.0, "darken": 20.0}});
        let m = photo.merged(&old).unwrap();
        assert_eq!((m.relight.lights[0].amount, m.relight.lights[0].x, m.relight.darken), (70.0, 0.6, 20.0));
        assert_eq!(m.relight.lights[1].amount, 30.0, "the other lights stay");
        // a block of the new shape merges as it always did
        let new = json!({"fork_relight": {"lights": [{"amount": 10.0}], "darken": 5.0}});
        assert_eq!(photo.merged(&new).unwrap().relight.lights.len(), 1);
        // and the first form is what an older log's patches name
        let flat = first_form(&serde_json::to_value(&photo.relight).unwrap()).unwrap();
        assert_eq!((flat["amount"].as_f64(), flat["x"].as_f64(), flat.get("lights")), (Some(40.0), Some(0.6), None));
    }
}
