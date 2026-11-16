//! Fork (issue #288, `fork/relight.md`): Relight, a light placed in the photo that lights it by depth. Its settings
//! and its controls (section [`Section::Relight`], ids `relight.<field>`).
//!
//! The settings are plain numbers in the units the controls show; `lightcraft-pipeline` turns them into the light
//! (`lightcraft_relight::Light`). The light's place is in normalized photo coordinates (before the crop and the lens
//! corrections, like a mask's) and its depth between the photo's nearest and farthest, so a draft, the loupe and an
//! export make the same picture, and undo, presets, copy settings, sync, the control channel and MCP reach it as
//! they reach any slider.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::controls::{ControlSpec, Section, Track};

/// The settings' JSON key (a namespace of this fork's own: a build without it ignores it).
pub const KEY: &str = "fork_relight";

/// The section id (its eye, `section.relight`).
pub const SECTION: &str = "relight";

/// Relight's settings. See the module docs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Relight {
    /// How strong the light is, 0..100 (0: off).
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
    /// Advanced: how much the rest of the photo darkens, 0..100.
    pub darken: f64,
    /// Advanced: how far the light wraps around shapes, 0 (follows them closely) .. 100 (ignores them).
    pub wrap: f64,
}

impl Default for Relight {
    fn default() -> Self {
        Self { amount: 0.0, softness: 50.0, warmth: 0.0, x: 0.35, y: 0.3, depth: 30.0, darken: 0.0, wrap: 40.0 }
    }
}

/// What a stored block may hold (every field optional; unknown ones ignored).
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
}

impl Default for Stored {
    fn default() -> Self {
        let d = Relight::default();
        Self { amount: d.amount, softness: d.softness, warmth: d.warmth, x: d.x, y: d.y, depth: d.depth, darken: d.darken, wrap: d.wrap }
    }
}

/// Read leniently: a malformed block (another build's, a hand-edited sidecar, an agent) reads as off, and each
/// number is kept within its control's range (a value that is not a number takes its default).
impl<'de> Deserialize<'de> for Relight {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let Ok(s) = serde_json::from_value::<Stored>(v) else { return Ok(Relight::default()) };
        let mut r =
            Relight { amount: s.amount, softness: s.softness, warmth: s.warmth, x: s.x, y: s.y, depth: s.depth, darken: s.darken, wrap: s.wrap };
        r.sanitize();
        Ok(r)
    }
}

impl Relight {
    pub fn is_default(&self) -> bool {
        *self == Relight::default()
    }

    /// Whether it changes the photo (the section's eye aside).
    pub fn is_on(&self) -> bool {
        self.amount > 0.0 || self.darken > 0.0
    }

    /// Every number within its control's range.
    pub fn sanitize(&mut self) {
        for spec in SPECS {
            if let Some(v) = self.field(spec.id) {
                let clamped = spec.clamp(*v);
                *v = clamped;
            }
        }
    }

    fn field(&mut self, id: &str) -> Option<&mut f64> {
        Some(match id {
            "relight.amount" => &mut self.amount,
            "relight.softness" => &mut self.softness,
            "relight.warmth" => &mut self.warmth,
            "relight.x" => &mut self.x,
            "relight.y" => &mut self.y,
            "relight.depth" => &mut self.depth,
            "relight.darken" => &mut self.darken,
            "relight.wrap" => &mut self.wrap,
            _ => return None,
        })
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
];

/// Relight's control specs.
pub fn specs() -> impl Iterator<Item = &'static ControlSpec> {
    SPECS.iter()
}

/// The spec of control `id`, when it is one of Relight's.
pub fn find(id: &str) -> Option<&'static ControlSpec> {
    SPECS.iter().find(|c| c.id == id)
}

/// Read a Relight control (`None`: not one).
pub fn get(s: &crate::DevelopSettings, id: &str) -> Option<f64> {
    let mut r = s.relight.clone();
    r.field(id).map(|v| *v)
}

/// Write a Relight control, kept within its range. False: not one.
pub fn set(s: &mut crate::DevelopSettings, id: &str, v: f64) -> bool {
    let Some(spec) = find(id) else { return false };
    match s.relight.field(id) {
        Some(f) => {
            *f = spec.clamp(v);
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
        assert!(set(&mut s, "relight.amount", 60.0) && set(&mut s, "relight.x", 0.7));
        let v = s.to_json();
        assert_eq!(v[KEY]["amount"], json!(60.0));
        let back = DevelopSettings::from_json(&v).unwrap();
        assert_eq!(back.relight, s.relight);
        assert_ne!(s.hash64(), d.hash64());
        assert_eq!(get(&back, "relight.x"), Some(0.7));
    }

    #[test]
    fn a_malformed_block_reads_as_off_and_numbers_stay_in_range() {
        let v = json!({"fork_relight": "nonsense"});
        assert!(DevelopSettings::from_json(&v).unwrap().relight.is_default());
        let v = json!({"fork_relight": {"amount": 1e9, "x": -40.0, "extra": 1}});
        let r = DevelopSettings::from_json(&v).unwrap().relight;
        assert_eq!((r.amount, r.x), (100.0, -0.5));
        let v = json!({"fork_relight": {"amount": 50.0, "warmth": "warm"}});
        assert!(DevelopSettings::from_json(&v).unwrap().relight.is_default());
        let mut s = DevelopSettings::default();
        assert!(set(&mut s, "relight.depth", f64::NAN));
        assert_eq!(s.relight.depth, 30.0);
        assert!(!set(&mut s, "relight.nope", 1.0));
    }
}
