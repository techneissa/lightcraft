//! Skin tone: Even Tone, Redness and Face Light.
//!
//! Today one slider, Amount, works as Face Light: it brightens the face's Facial Skin mask by up to
//! [`FACE_LIGHT_GAIN`] at 100, and leaves every pixel outside the mask, and a face without the mask, alone. It is
//! the proof that part masks reach the step (`fork/retouch-masks.md`); the feature's own issue replaces it, in this
//! module, its example (`examples/skin_tone.rs`), its test file (`tests/skin_tone.rs`) and its section of the
//! Portrait panel; nothing else.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

/// How much brighter Facial Skin is at Amount 100 (a factor of 1.5: a little over half a stop).
pub const FACE_LIGHT_GAIN: f32 = 0.5;

/// The Skin Tone settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinTone {
    /// 0…100; 0 is off.
    pub amount: f64,
}

impl Feature for SkinTone {
    const KEY: &'static str = "skinTone";
    const LABEL: &'static str = "Skin Tone";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.skinTone.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        let amount = if self.amount.is_finite() { self.amount.clamp(0.0, 100.0) as f32 / 100.0 } else { 0.0 };
        let gain = FACE_LIGHT_GAIN * amount;
        let Some([x0, y0, x1, y1]) = scene.mask_box(face, Part::FacialSkin) else { return };
        if gain <= 0.0 || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let w = img.width;
        for y in y0..y1.min(img.height) {
            for x in x0..x1.min(w) {
                let m = scene.mask_at(face, Part::FacialSkin, x, y);
                if m <= 0.0 {
                    continue;
                }
                if let Some(px) = img.data.get_mut(y * w + x) {
                    let k = 1.0 + gain * m;
                    *px = [px[0] * k, px[1] * k, px[2] * k];
                }
            }
        }
    }
}
