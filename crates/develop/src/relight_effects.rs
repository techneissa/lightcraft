//! Fork (issue #302, `fork/relight.md`): the effects of one Relight light: a gel, a pattern and haze. They are part of
//! the light's settings ([`crate::relight::LightSettings::effects`]); the pipeline turns them into
//! `lightcraft_relight::Effects`. Issue #315 adds Shadows (how much the photo's own things block the light), issue #316 the pattern's openness (`patternOpen`). A light with none
//! writes nothing, so an edit made before them reads and saves as it was; a block with some reads leniently like the rest (a number that is not a number takes its default).
//! Issue #317 makes Haze a medium the light's beams show in, and adds Forward Scatter and Dust to it; issue #331 adds Mist,
//! Mist Height and Distance Fade. Issue #332 adds Edge Glow and
//! Sunburst: light that spills round an edge the light sits just behind. Issue #330 adds a sun's rays in the sky (Sky Rays,
//! shaped by the user and cut by the clouds and near things), its soft rays opposite (Opposite Rays) and where the sun is
//! in the photo for them.

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
fn half(v: &f64) -> bool {
    *v == 0.5
}
fn tenth(v: &f64) -> bool {
    *v == 0.1
}
fn seven(v: &f64) -> bool {
    *v == 7.0
}
fn sixty(v: &f64) -> bool {
    *v == 60.0
}
fn eighty(v: &f64) -> bool {
    *v == 80.0
}
fn eighty_five(v: &f64) -> bool {
    *v == 85.0
}
fn full_turn(v: &f64) -> bool {
    *v == 360.0
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
    /// How much light spills round an edge the light sits just behind (issue #332), 0 (none) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub edge_glow: f64,
    /// How strong the few faint rays at that edge are (issue #332), 0 (none) .. 100.
    #[serde(skip_serializing_if = "zero")]
    pub sunburst: f64,
    /// A sun's rays in the sky (issue #330), 0..100: how much light they add, within the headroom below white.
    #[serde(skip_serializing_if = "zero")]
    pub sky_rays: f64,
    /// How many rays fan out over the spread (issue #330), 3..60, a whole number.
    #[serde(skip_serializing_if = "seven")]
    pub ray_count: f64,
    /// How far the rays reach from the sun (issue #330), 0..100 (100: one and a half times the photo's long side).
    #[serde(skip_serializing_if = "sixty")]
    pub ray_length: f64,
    /// How wide each ray is (issue #330), 0 (a thin line) .. 100 (about 15° round the sun).
    #[serde(skip_serializing_if = "eighty")]
    pub ray_width: f64,
    /// How much the rays' widths and brightness differ from ray to ray (issue #330), 0..100; random from a seed fixed per
    /// photo.
    #[serde(skip_serializing_if = "eighty")]
    pub ray_variation: f64,
    /// Where the rays fan towards from the sun (issue #330), degrees −180..180: 0 straight down, 90 right, −90 left.
    #[serde(skip_serializing_if = "zero")]
    pub ray_direction: f64,
    /// How wide the fan of rays is round that direction (issue #330), 10..360 degrees.
    #[serde(skip_serializing_if = "full_turn")]
    pub ray_spread: f64,
    /// How soft each ray's edges are (issue #330), 0..100.
    #[serde(skip_serializing_if = "eighty_five")]
    pub ray_softness: f64,
    /// How much the photo's clouds and near things shape the rays (issue #330), 0 (the rays pass over everything but a
    /// thick cloud's own body) .. 100.
    #[serde(skip_serializing_if = "sixty")]
    pub ray_blocking: f64,
    /// A sun's soft rays converging on the point opposite it (issue #330), 0..100.
    #[serde(skip_serializing_if = "zero")]
    pub opposite_rays: f64,
    /// Whether the rays' centre is where `rays_x`, `rays_y` put it (1), or found from the sky (0: the brightest place of
    /// the sky for Sky Rays, the middle of the horizon for Opposite Rays).
    #[serde(skip_serializing_if = "zero")]
    pub rays_placed: f64,
    /// The rays' centre in normalized photo coordinates (it may lie above the photo): the sun for Sky Rays, the point
    /// opposite the sun for Opposite Rays.
    #[serde(skip_serializing_if = "half")]
    pub rays_x: f64,
    #[serde(skip_serializing_if = "tenth")]
    pub rays_y: f64,
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
            edge_glow: 0.0,
            sunburst: 0.0,
            sky_rays: 0.0,
            ray_count: 7.0,
            ray_length: 60.0,
            ray_width: 80.0,
            ray_variation: 80.0,
            ray_direction: 0.0,
            ray_spread: 360.0,
            ray_softness: 85.0,
            ray_blocking: 60.0,
            opposite_rays: 0.0,
            rays_placed: 0.0,
            rays_x: 0.5,
            rays_y: 0.1,
        }
    }
}

impl LightEffects {
    /// The number the control `field` (`gelHue`, `gelStrength`, `pattern`, `patternScale`, `patternRotation`,
    /// `patternSoftness`, `patternOpen`, `haze`, `scatter`, `dust`, `shadows`, `mist`, `mistHeight`, `fade`, `source`, `edgeGlow`, `sunburst`, `skyRays`,
    /// `rayCount`, `rayLength`, `rayWidth`, `rayVariation`, `rayDirection`, `raySpread`, `raySoftness`, `rayBlocking`,
    /// `oppositeRays`, `raysPlaced`, `raysX`, `raysY`) names.
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
            "edgeGlow" => &mut self.edge_glow,
            "sunburst" => &mut self.sunburst,
            "skyRays" => &mut self.sky_rays,
            "rayCount" => &mut self.ray_count,
            "rayLength" => &mut self.ray_length,
            "rayWidth" => &mut self.ray_width,
            "rayVariation" => &mut self.ray_variation,
            "rayDirection" => &mut self.ray_direction,
            "raySpread" => &mut self.ray_spread,
            "raySoftness" => &mut self.ray_softness,
            "rayBlocking" => &mut self.ray_blocking,
            "oppositeRays" => &mut self.opposite_rays,
            "raysPlaced" => &mut self.rays_placed,
            "raysX" => &mut self.rays_x,
            "raysY" => &mut self.rays_y,
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
            "edgeGlow": self.edge_glow, "sunburst": self.sunburst, "skyRays": self.sky_rays, "rayCount": self.ray_count,
            "rayLength": self.ray_length, "rayWidth": self.ray_width, "rayVariation": self.ray_variation, "rayDirection": self.ray_direction,
            "raySpread": self.ray_spread, "raySoftness": self.ray_softness, "rayBlocking": self.ray_blocking, "oppositeRays": self.opposite_rays,
            "raysPlaced": self.rays_placed, "raysX": self.rays_x, "raysY": self.rays_y,
        })
    }
}
