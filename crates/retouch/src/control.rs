//! Slider specs of the retouching features. Each feature module lists its own ([`crate::Feature::CONTROLS`]);
//! `lightcraft-develop` turns them into develop control specs (section Portrait), so the panel, the control
//! channel and MCP reach every value by id.

/// One slider: id, label, range, default and how it is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    /// `retouch.<feature>.<field>`, e.g. `retouch.skin.smoothness`.
    pub id: &'static str,
    /// Title Case, shown on the slider.
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    /// The value that leaves the picture alone.
    pub default: f64,
    pub step: f64,
    /// Decimal places shown.
    pub decimals: u8,
    /// Zero in the middle of the track (a −100…+100 slider).
    pub centered: bool,
}

impl Spec {
    /// A 0…100 slider that is off at 0 (most retouching amounts).
    pub const fn amount(id: &'static str, label: &'static str) -> Spec {
        Spec { id, label, min: 0.0, max: 100.0, default: 0.0, step: 1.0, decimals: 0, centered: false }
    }

    /// A −100…+100 slider that is off at 0.
    pub const fn signed(id: &'static str, label: &'static str) -> Spec {
        Spec { id, label, min: -100.0, max: 100.0, default: 0.0, step: 1.0, decimals: 0, centered: true }
    }

    /// `v` within the range; the default for a value that is not a number.
    pub fn clamp(&self, v: f64) -> f64 {
        if v.is_finite() { v.clamp(self.min, self.max) } else { self.default }
    }
}

/// A slider of the feature settings `T`: its spec and how to read and write its value.
pub struct Control<T: 'static> {
    pub spec: Spec,
    pub get: fn(&T) -> f64,
    pub set: fn(&mut T, f64),
}

impl<T> Control<T> {
    /// The value, or the default when the stored one is not a number.
    pub fn read(&self, s: &T) -> f64 {
        let v = (self.get)(s);
        if v.is_finite() { v } else { self.spec.default }
    }

    /// Write `v`, kept within the range.
    pub fn write(&self, s: &mut T, v: f64) {
        (self.set)(s, self.spec.clamp(v));
    }
}

/// The control of `controls` with id `id`.
pub fn find<T>(controls: &'static [Control<T>], id: &str) -> Option<&'static Control<T>> {
    controls.iter().find(|c| c.spec.id == id)
}
