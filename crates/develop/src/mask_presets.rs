//! Fork (issue #329, `fork/mask-presets.md`): mask presets. A preset is one mask kept as a recipe: its parts (kind,
//! parameters, mode, Invert), its effect sliders, Refine Edges and Decontaminate. It is applied to a photo as a new mask.
//!
//! **Found again, never copied.** An AI part (Subject, Sky, Background, Object, Describe, Select People) keeps what it
//! selects and not the selection a model found on the photo it was saved from (`adaptive::strip_found`); applied, the
//! part waits and the photo's own selection is found there (`fork/adaptive-masks.md`). A Select People part keeps whose
//! face it is only when the face is named; one known only by where its box was is everyone's.
//!
//! **Relative to the photo.** A mask's places (a brush stroke's points, a gradient's ends and centre, the clicks of an
//! Object) are normalized to the uncropped photo as it is shown (after the user's rotation), and a gradient's radii and a
//! brush's size are fractions of its long edge. A preset keeps them as they are, so it lands at the same relative place,
//! the same share of the long edge large, on a photo of any size or orientation. A mask drawn on the corrected frame
//! (`Mask::on_photo` false) is put on the photo first (the engine does it before saving).

use serde::{Deserialize, Serialize};

pub use crate::relight_presets::{MAX_NAME, MAX_USER, clean_name};
use crate::{Mask, MaskShape};

/// A saved mask: an id, a name and the mask as a recipe. `mask.id` is 0 and `mask.name` the preset's own name, until
/// applied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaskPreset {
    pub id: String,
    pub name: String,
    pub mask: Mask,
}

/// A shape's `kind` tag as saved (`"radial"`, `"luminanceRange"`…).
fn kind_name(shape: &MaskShape) -> &'static str {
    match shape {
        MaskShape::Brush { .. } => "brush",
        MaskShape::Linear { .. } => "linear",
        MaskShape::Radial { .. } => "radial",
        MaskShape::ColorRange { .. } => "colorRange",
        MaskShape::LuminanceRange { .. } => "luminanceRange",
        MaskShape::DepthRange { .. } => "depthRange",
        MaskShape::Subject { .. } => "subject",
        MaskShape::Sky { .. } => "sky",
        MaskShape::Background { .. } => "background",
        MaskShape::Object { .. } => "object",
        MaskShape::Prompt { .. } => "prompt",
        MaskShape::People { .. } => "people",
        MaskShape::Landscape { .. } => "landscape",
    }
}

impl MaskPreset {
    /// A preset of mask `m` (already on the photo: `m.on_photo`) named `name`, without what a model found on its photo.
    pub fn capture(id: &str, name: &str, m: &Mask) -> MaskPreset {
        let mut mask = m.clone();
        mask.id = 0;
        mask.name = name.to_string();
        mask.visible = true;
        mask.before_invert.clear();
        crate::adaptive::strip_found(std::slice::from_mut(&mut mask));
        for c in &mut mask.components {
            if let MaskShape::People { person, face, .. } = &mut c.shape {
                // (the source photo's own index of the person: found again by `face`, or everyone's)
                *person = 0;
                if face.as_ref().is_some_and(|k| k.person.is_none()) {
                    *face = None;
                }
            }
        }
        MaskPreset { id: id.into(), name: name.into(), mask }
    }

    /// The kinds of the preset's parts (`MaskShape`'s `kind` names), in order.
    pub fn kinds(&self) -> Vec<&'static str> {
        self.mask.components.iter().map(|c| kind_name(&c.shape)).collect()
    }

    /// How many of the preset's parts a model finds on each photo.
    pub fn ai_parts(&self) -> usize {
        self.mask.components.iter().filter(|c| c.shape.is_detected()).count()
    }

    /// Whether the preset can be applied: it has a part.
    pub fn is_usable(&self) -> bool {
        !self.mask.components.is_empty()
    }

    /// The preset's mask as a new mask of a photo: id `id`, named `name`, its AI parts waiting to be found on the photo.
    /// `None` for a preset without a part.
    pub fn instance(&self, id: u32, name: &str) -> Option<Mask> {
        if !self.is_usable() {
            return None;
        }
        let mut mask = self.mask.clone();
        mask.id = id;
        mask.name = name.to_string();
        mask.visible = true;
        mask.on_photo = true;
        mask.before_invert.clear();
        for c in &mut mask.components {
            if c.shape.is_detected() {
                c.shape.set_waiting();
            }
        }
        Some(mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MaskComponent, MaskOp, SegMask};
    use lightcraft_geom::Point;

    fn part(op: MaskOp, shape: MaskShape) -> MaskComponent {
        MaskComponent { name: None, op, invert: false, shape }
    }

    #[test]
    fn a_preset_keeps_what_a_part_selects_and_not_what_was_found() {
        let found = SegMask::from_logits(2, &[5.0, -5.0, -5.0, -5.0]);
        let m = Mask {
            id: 4,
            name: "Sky Mask 1".into(),
            components: vec![
                part(MaskOp::Add, MaskShape::Sky { seg: Some(found.clone()), detail: vec![found], edge: 20.0 }),
                part(
                    MaskOp::Subtract,
                    MaskShape::Radial { center: Point::new(0.3, 0.6), rx: 0.2, ry: 0.1, angle: 0.0, feather: 40.0, invert: false },
                ),
            ],
            ..Default::default()
        };
        let p = MaskPreset::capture("user-1", "Warm Sky", &m);
        assert_eq!((p.mask.id, p.mask.name.as_str()), (0, "Warm Sky"));
        assert!(p.mask.components[0].shape.seg().is_none(), "no pixels in the preset");
        assert_eq!(p.kinds(), ["sky", "radial"]);
        assert_eq!(serde_json::to_value(&p.mask.components[1].shape).unwrap()["kind"], "radial", "the names are the saved tags");
        let on = p.instance(1, "Warm Sky").unwrap();
        assert!(on.components[0].shape.is_waiting(), "the sky waits to be found on the photo");
        assert_eq!(on.components[1], m.components[1], "the gradient comes as it was");
    }
}
