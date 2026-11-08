//! Match Look's residual Look (fork feature, `fork/match-look-followups.md`): a colour lookup in OkLab, fitted after
//! the sliders for what they cannot reach, with its own Amount. Saved with the edit; left out of the JSON while it
//! does nothing, so settings without it keep their JSON and `hash64`. The pipeline turns it into a 3D lookup table
//! (`lightcraft_pipeline::residual_look`).

use serde::{Deserialize, Serialize};

/// Lightness knots of [`ResidualLook::lightness`] (OkLab L = 0, 1/8, …, 1).
pub const LOOK_KNOTS: usize = 9;

/// The residual Look's numbers, all offsets in OkLab units (0 = no change).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResidualLook {
    /// How much of the Look applies, 0..100.
    pub amount: f64,
    /// Lightness offsets at L = 0, 1/8, …, 1, linear between.
    pub lightness: [f64; LOOK_KNOTS],
    /// The a, b offset of the shadows, midtones and highlights.
    pub cast: [[f64; 2]; 3],
    /// Colourfulness: 0.1 makes every colour 10 % more colourful.
    pub chroma: f64,
    /// Per colour mixer band: the L, a, b offset of that hue's colourful pixels.
    pub bands: [[f64; 3]; 8],
}

impl Default for ResidualLook {
    fn default() -> Self {
        ResidualLook { amount: 100.0, lightness: [0.0; LOOK_KNOTS], cast: [[0.0; 2]; 3], chroma: 0.0, bands: [[0.0; 3]; 8] }
    }
}

impl ResidualLook {
    /// No offsets: the Look changes nothing, whatever its Amount.
    pub fn is_identity(&self) -> bool {
        self.lightness.iter().chain(self.cast.iter().flatten()).chain(self.bands.iter().flatten()).all(|v| *v == 0.0) && self.chroma == 0.0
    }

    /// Not written to the settings JSON (see the module docs).
    pub fn is_default(&self) -> bool {
        self.is_identity()
    }

    /// Whether a render applies it.
    pub fn is_active(&self) -> bool {
        !self.is_identity() && self.amount.is_finite() && self.amount > 0.0
    }

    /// The same Look with every offset times `k` (Amount unchanged).
    pub fn scaled(&self, k: f64) -> ResidualLook {
        let mut r = self.clone();
        r.lightness.iter_mut().chain(r.cast.iter_mut().flatten()).chain(r.bands.iter_mut().flatten()).for_each(|v| *v *= k);
        r.chroma *= k;
        r
    }
}
