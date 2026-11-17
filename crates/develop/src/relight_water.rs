//! Fork (issue #333, `fork/relight.md`): Relight's underwater look. It belongs to the whole photo (like Darken Surroundings),
//! not to a light: water colour by distance (red fades first, then green; Clarity and Tint), shafts of sun from the surface,
//! and caustics on what faces up. The pipeline turns it into the water's colour planes and a sun of its own
//! (`lightcraft_relight::Water`, `lightcraft_pipeline::relight::scene_of`). It is saved only while some number is off its
//! default, and read leniently like the rest of the block.

use serde::{Deserialize, Serialize};

fn zero(v: &f64) -> bool {
    *v == 0.0
}
fn fifty(v: &f64) -> bool {
    *v == 50.0
}

/// The underwater look's numbers, as its controls show them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WaterSettings {
    /// How much water there is between the camera and each thing, 0 (none: the look is off) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub amount: f64,
    /// How far colour carries, 0 (murky) .. 100 (clear).
    #[serde(skip_serializing_if = "fifty")]
    pub clarity: f64,
    /// The water's colour, −100 (blue ocean) .. 0 (blue-green) .. 100 (green lake).
    #[serde(skip_serializing_if = "zero")]
    pub tint: f64,
    /// Shafts of sun coming down from the surface, 0 (none) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub shafts: f64,
    /// The moving-water net of light on surfaces that face up, 0 (none) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub caustics: f64,
}

impl Default for WaterSettings {
    fn default() -> Self {
        Self { amount: 0.0, clarity: 50.0, tint: 0.0, shafts: 0.0, caustics: 0.0 }
    }
}

impl WaterSettings {
    pub fn is_default(&self) -> bool {
        *self == WaterSettings::default()
    }

    /// Whether the look changes the photo: some of its three parts is up.
    pub fn is_on(&self) -> bool {
        self.amount > 0.0 || self.shafts > 0.0 || self.caustics > 0.0
    }

    /// The number the control `field` (`water`, `waterClarity`, `waterTint`, `waterShafts`, `waterCaustics`) names.
    pub(crate) fn field(&mut self, field: &str) -> Option<&mut f64> {
        Some(match field {
            "water" => &mut self.amount,
            "waterClarity" => &mut self.clarity,
            "waterTint" => &mut self.tint,
            "waterShafts" => &mut self.shafts,
            "waterCaustics" => &mut self.caustics,
            _ => return None,
        })
    }

    /// Every number within its control's range (a number that is not a number takes its default).
    pub fn sanitize(&mut self) {
        let d = WaterSettings::default();
        for (field, default) in
            [("water", d.amount), ("waterClarity", d.clarity), ("waterTint", d.tint), ("waterShafts", d.shafts), ("waterCaustics", d.caustics)]
        {
            let spec = SPECS.iter().find(|c| c.id.strip_prefix("relight.") == Some(field));
            if let (Some(spec), Some(v)) = (spec, self.field(field)) {
                *v = if v.is_finite() { spec.clamp(*v) } else { default };
            }
        }
    }
}

use crate::controls::{ControlSpec, Section, Track};

const fn spec(id: &'static str, label: &'static str, (min, max, default): (f64, f64, f64), track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::Relight, min, max, default, step: 1.0, decimals: 0, track }
}

/// The underwater look's controls (the photo's, not a light's: `relight.water`, never `relight.<n>.water`).
pub static SPECS: &[ControlSpec] = &[
    spec("relight.water", "Water Depth", (0.0, 100.0, 0.0), Track::Plain),
    spec("relight.waterClarity", "Water Clarity", (0.0, 100.0, 50.0), Track::Plain),
    spec("relight.waterTint", "Water Tint", (-100.0, 100.0, 0.0), Track::Centered),
    spec("relight.waterShafts", "Surface Shafts", (0.0, 100.0, 0.0), Track::Plain),
    spec("relight.waterCaustics", "Caustics", (0.0, 100.0, 0.0), Track::Plain),
];

/// Whether `field` (the part of a control id after `relight.`) is one of the photo's own, not a light's.
pub fn is_shared(field: &str) -> bool {
    field == "darken" || SPECS.iter().any(|c| c.id.strip_prefix("relight.") == Some(field))
}
