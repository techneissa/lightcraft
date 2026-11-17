//! Fork (issue #302, `fork/relight.md`): the effects of one Relight light: a gel, a pattern and haze. They are part of
//! the light's settings ([`crate::relight::LightSettings::effects`]); the pipeline turns them into
//! `lightcraft_relight::Effects`. Issue #315 adds Shadows (how much the photo's own things block the light), issue #316 the pattern's openness (`patternOpen`). A light with none
//! writes nothing, so an edit made before them reads and saves as it was; a block with some reads leniently like the rest (a number that is not a number takes its default).
//! Issue #317 makes Haze a medium the light's beams show in, and adds Forward Scatter and Dust to it; issue #331 adds Mist,
//! Mist Height and Distance Fade.

use serde::{Deserialize, Serialize};

/// The pattern's number in the control `relight.<n>.pattern`: none, blinds, window panes, leaves, dapple.
pub const PATTERNS: [&str; 5] = ["None", "Blinds", "Window Panes", "Leaves", "Dapple"];

/// Named gels: the name, the hue (degrees) and the strength (0..100) it sets (`relight.setGel`, the section's swatches).
pub const GELS: [(&str, f64, f64); 5] =
    [("Warm Amber", 38.0, 70.0), ("Teal", 178.0, 65.0), ("Magenta", 305.0, 65.0), ("Deep Blue", 232.0, 75.0), ("Red", 0.0, 75.0)];

// (what the saved form leaves out: a number at its default)
fn zero(v: &f64) -> bool {
    *v == 0.0
}
fn fifty(v: &f64) -> bool {
    *v == 50.0
}
fn thirty(v: &f64) -> bool {
    *v == 30.0
}

/// A light's effects, as the numbers its controls show.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LightEffects {
    /// The gel's hue, 0..360 degrees (0 red, 120 green, 240 blue).
    #[serde(skip_serializing_if = "zero")]
    pub gel_hue: f64,
    /// The gel's strength, 0 (no gel) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub gel_strength: f64,
    /// The pattern: 0 none, 1 blinds, 2 window panes, 3 leaves, 4 dapple ([`PATTERNS`]).
    #[serde(skip_serializing_if = "zero")]
    pub pattern: f64,
    /// The pattern's size, 0 (fine) .. 100 (large).
    #[serde(skip_serializing_if = "fifty")]
    pub pattern_scale: f64,
    /// The pattern's turn, −180..180 degrees.
    #[serde(skip_serializing_if = "zero")]
    pub pattern_rotation: f64,
    /// How soft the pattern's edges are, 0..100.
    #[serde(skip_serializing_if = "thirty")]
    pub pattern_softness: f64,
    /// The haze the light's beams show in, 0..100.
    #[serde(skip_serializing_if = "zero")]
    pub haze: f64,
    /// How much the haze scatters the light onward (issue #317): 0 every way alike .. 100 mostly onward, so beams glow most
    /// looking into the light.
    #[serde(skip_serializing_if = "fifty")]
    pub scatter: f64,
    /// How much the haze's density varies through the scene (issue #317), 0..100.
    #[serde(skip_serializing_if = "zero")]
    pub dust: f64,
    /// How much of the light the photo's own things block (issue #315), 0 (none) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub shadows: f64,
    /// How open the pattern is (issue #316), 0..100: the blinds' slat tilt, a window's mullions, the leaves' size, the
    /// canopy's gaps.
    #[serde(skip_serializing_if = "fifty")]
    pub pattern_open: f64,
    /// Low mist (issue #331): how thick the air is near the ground the depth shows, 0 (none) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub mist: f64,
    /// How high the mist reaches above the ground (issue #331), 0 (a thin layer) .. 100 (above the camera).
    #[serde(skip_serializing_if = "thirty")]
    pub mist_height: f64,
    /// Distance Fade (issue #331): how much far things fade into the haze and mist, 0..100.
    #[serde(skip_serializing_if = "zero")]
    pub fade: f64,
    /// How bright and big the light's own source shows as a point when it is inside the photo (issue #334), 0..100.
    #[serde(skip_serializing_if = "zero")]
    pub source: f64,
}

impl Default for LightEffects {
    fn default() -> Self {
        Self {
            gel_hue: 0.0,
            gel_strength: 0.0,
            pattern: 0.0,
            pattern_scale: 50.0,
            pattern_rotation: 0.0,
            pattern_softness: 30.0,
            haze: 0.0,
            scatter: 50.0,
            dust: 0.0,
            shadows: 0.0,
            pattern_open: 50.0,
            mist: 0.0,
            mist_height: 30.0,
            fade: 0.0,
            source: 0.0,
        }
    }
}

impl LightEffects {
    /// The number the control `field` (`gelHue`, `gelStrength`, `pattern`, `patternScale`, `patternRotation`,
    /// `patternSoftness`, `patternOpen`, `haze`, `scatter`, `dust`, `shadows`, `mist`, `mistHeight`, `fade`) names.
    pub(crate) fn field(&mut self, field: &str) -> Option<&mut f64> {
        Some(match field {
            "gelHue" => &mut self.gel_hue,
            "gelStrength" => &mut self.gel_strength,
            "pattern" => &mut self.pattern,
            "patternScale" => &mut self.pattern_scale,
            "patternRotation" => &mut self.pattern_rotation,
            "patternSoftness" => &mut self.pattern_softness,
            "patternOpen" => &mut self.pattern_open,
            "haze" => &mut self.haze,
            "scatter" => &mut self.scatter,
            "dust" => &mut self.dust,
            "shadows" => &mut self.shadows,
            "mist" => &mut self.mist,
            "mistHeight" => &mut self.mist_height,
            "fade" => &mut self.fade,
            "source" => &mut self.source,
            _ => return None,
        })
    }

    /// The named gel `name` (case and spaces aside), as `(hue, strength)`.
    pub fn gel(name: &str) -> Option<(f64, f64)> {
        let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        GELS.iter().find(|(n, ..)| key(n) == key(name)).map(|(_, hue, strength)| (*hue, *strength))
    }

    /// Every number as JSON (the saved form leaves out what is at its default).
    pub fn to_full_json(&self) -> serde_json::Value {
        serde_json::json!({
            "gelHue": self.gel_hue, "gelStrength": self.gel_strength, "pattern": self.pattern, "patternScale": self.pattern_scale,
            "patternRotation": self.pattern_rotation, "patternSoftness": self.pattern_softness, "patternOpen": self.pattern_open, "haze": self.haze,
            "scatter": self.scatter, "dust": self.dust, "shadows": self.shadows, "mist": self.mist, "mistHeight": self.mist_height, "fade": self.fade, "source": self.source,
        })
    }
}
