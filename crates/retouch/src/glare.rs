//! Glasses glare removal (`fork/glare-repair.md`, issue #74).
//!
//! The repair itself is a model run outside the render: the engine repaints the lenses of one face (FLUX.1 Fill where
//! glare hides the eyes, or LaMa over small glints) once, when asked, and hands the result to every render as the
//! face's [`Patch`](crate::input::Patch): a gain per pixel and a feathered lens mask. This feature only blends it in:
//! `pixel × (1 + Amount · alpha · (gain − 1))`. A slider drag runs no model, and the preview, the loupe and the
//! export apply the same patch, sampled in photo coordinates.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Scene, Scope, Spec, Stage};

/// The Glasses Glare settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glare {
    /// 0…100: how much of the repair shows; 0 is off.
    pub amount: f64,
}

impl Feature for Glare {
    const KEY: &'static str = "glare";
    const LABEL: &'static str = "Glasses Glare";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.glare.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];

    /// The lenses (`area::lenses`).
    fn area(scene: &Scene<'_>, faces: &[usize]) -> Option<lightcraft_raster::Plane> {
        crate::area::lenses(scene, faces)
    }

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let amount = (self.amount / 100.0).clamp(0.0, 1.0) as f32;
        let Some(f) = face.and_then(|i| scene.faces.get(i)) else { return };
        let Some(patch) = f.face.patch.as_deref() else { return };
        if amount <= 0.0 || img.width != scene.width || img.height != scene.height || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some([x0, y0, x1, y1]) = scene.area_box(&patch.area) else { return };
        let view = scene.view();
        for y in y0..y1 {
            for x in x0..x1 {
                let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
                let Some((a, gain)) = patch.sample(nx, ny) else { continue };
                let Some(px) = img.data.get_mut(y * img.width + x) else { continue };
                let k = a * amount;
                for (c, g) in px.iter_mut().zip(gain) {
                    let v = *c * (1.0 + k * (g - 1.0));
                    if v.is_finite() {
                        *c = v;
                    }
                }
            }
        }
    }
}
