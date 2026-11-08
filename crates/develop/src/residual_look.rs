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

    /// `to` at `k` (0..1) of the way from `from`: with no Look before, `to` at `k` of its Amount; otherwise each
    /// offset (at its Look's Amount) moves `k` of the way, at Amount 100. Match Look's Amount uses it, so 1 % stays
    /// close to the photo's earlier Look instead of replacing it.
    pub fn blend(from: &ResidualLook, to: &ResidualLook, k: f64) -> ResidualLook {
        let k = if k.is_finite() { k.clamp(0.0, 1.0) } else { 1.0 };
        if from.is_identity() || !from.is_active() {
            return ResidualLook { amount: (to.amount * k).round(), ..to.clone() };
        }
        if k >= 1.0 {
            return to.clone();
        }
        let strength = |l: &ResidualLook| if l.amount.is_finite() { (l.amount / 100.0).clamp(0.0, 1.0) } else { 0.0 };
        let (a, b) = (from.scaled(strength(from)), to.scaled(strength(to)));
        let mut r = a.clone();
        let mix = |x: &mut f64, y: &f64| *x += (y - *x) * k;
        r.lightness.iter_mut().zip(&b.lightness).for_each(|(x, y)| mix(x, y));
        r.cast.iter_mut().flatten().zip(b.cast.iter().flatten()).for_each(|(x, y)| mix(x, y));
        r.bands.iter_mut().flatten().zip(b.bands.iter().flatten()).for_each(|(x, y)| mix(x, y));
        mix(&mut r.chroma, &b.chroma);
        r.amount = 100.0;
        r
    }

    /// The same Look with every offset times `k` (Amount unchanged).
    pub fn scaled(&self, k: f64) -> ResidualLook {
        let mut r = self.clone();
        r.lightness.iter_mut().chain(r.cast.iter_mut().flatten()).chain(r.bands.iter_mut().flatten()).for_each(|v| *v *= k);
        r.chroma *= k;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DevelopSettings, Section, SettingsGroup, apply_partial, extract_groups};

    fn with_look() -> DevelopSettings {
        let mut d = DevelopSettings::default();
        d.look.cast[1] = [0.01, -0.02];
        d.look.amount = 60.0;
        d
    }

    #[test]
    fn a_profile_reset_clears_the_look() {
        let mut d = with_look();
        d.reset_section(Section::Profile);
        assert_eq!(d.look, ResidualLook::default());
    }

    #[test]
    fn the_profile_group_carries_the_look_both_ways() {
        let patch = extract_groups(&with_look(), &[SettingsGroup::Profile]);
        assert_eq!(apply_partial(&DevelopSettings::default(), &patch, 1.0).look, with_look().look, "pasted onto a photo without one");
        let none = extract_groups(&DevelopSettings::default(), &[SettingsGroup::Profile]);
        assert!(apply_partial(&with_look(), &none, 1.0).look.is_identity(), "a photo without a Look pastes none");
        let light = extract_groups(&with_look(), &[SettingsGroup::Light]);
        assert!(light.get("look").is_none(), "only the Profile group");
    }

    #[test]
    fn blend_moves_from_the_earlier_look() {
        let from = with_look();
        let mut to = ResidualLook::default();
        to.cast[1] = [0.03, 0.0];
        let half = ResidualLook::blend(&from.look, &to, 0.5);
        assert!((half.cast[1][0] - (0.006 + 0.03) / 2.0).abs() < 1e-9 && half.amount == 100.0, "{half:?}");
        let fresh = ResidualLook::blend(&ResidualLook::default(), &to, 0.4);
        assert_eq!((fresh.amount, fresh.cast), (40.0, to.cast));
    }
}
