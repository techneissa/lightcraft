//! Subject, Sky and Background found by SAM 3 (issue #37): the words the model is asked for, whether a new
//! selection should be computed by it at all, and how the result is stored.
//!
//! The model is asked for a person first, then an animal, a vehicle, then any main object; the first of these that
//! it finds with confidence is the subject. Background is the subject turned around (its logits negated), stored with
//! the mask like the others, so previews and exports never need the model. Without the model, or when the photo has
//! an embedded matte for the kind (a DNG's sky or person matte), the shape stays the classical one.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{MaskShape, SegMask};
use lightcraft_pipeline::masks::MatteKind;

use crate::{EngineError, Result, Session};

/// What SAM 3 is asked for to find the subject (`|` separates alternatives, tried in order).
pub const SUBJECT: &str = "person | animal | vehicle | subject, main object, foreground";
/// What SAM 3 is asked for to find the sky.
pub const SKY: &str = "sky";

/// One kind of selection the model computes.
pub struct Concept {
    /// `subject`, `sky` or `background`.
    pub kind: &'static str,
    pub text: &'static str,
    /// The found selection is turned around (Background is everything but the subject).
    pub invert: bool,
}

/// The concept a classical shape stands for.
pub fn of(shape: &MaskShape) -> Option<Concept> {
    match shape {
        MaskShape::Subject { .. } => Some(Concept { kind: "subject", text: SUBJECT, invert: false }),
        MaskShape::Sky { .. } => Some(Concept { kind: "sky", text: SKY, invert: false }),
        MaskShape::Background { .. } => Some(Concept { kind: "background", text: SUBJECT, invert: true }),
        _ => None,
    }
}

/// The embedded mattes that stand for a kind (the pipeline uses them before anything else).
fn matte_kinds(kind: &str) -> &'static [MatteKind] {
    match kind {
        "sky" => &[MatteKind::Sky],
        _ => &[MatteKind::Person, MatteKind::Skin, MatteKind::Hair, MatteKind::Teeth, MatteKind::Glasses],
    }
}

/// Whether a new `shape` on `photo` is computed by SAM 3: the model (or a remote worker) is there and the photo has
/// no embedded matte for it.
pub fn wanted(s: &Session, photo: PhotoId, shape: &MaskShape) -> bool {
    let Some(c) = of(shape) else { return false };
    s.segmenter.has_model() && !s.media.has_matte(photo, matte_kinds(c.kind))
}

/// What the user is told when the model finds nothing.
pub fn not_found(kind: &str) -> String {
    match kind {
        "background" => "No subject was found in this photo, so there is no background to select.".into(),
        _ => format!("No {kind} was found in this photo."),
    }
}

/// Store a found selection in `shape` (turned around for Background). Nothing found is an error, never a rough mask.
pub fn store(shape: &mut MaskShape, found: Option<SegMask>, c: &Concept) -> Result<()> {
    let found = found.ok_or_else(|| EngineError::Other(not_found(c.kind)))?;
    let found = if c.invert { found.invert().ok_or_else(|| EngineError::Other(not_found(c.kind)))? } else { found };
    match shape {
        MaskShape::Subject { seg, .. } | MaskShape::Sky { seg, .. } | MaskShape::Background { seg, .. } => *seg = Some(found),
        _ => {}
    }
    Ok(())
}

impl Session {
    /// Whether `shape` is a Subject, Sky or Background that the classical heuristic draws, not SAM 3 and not an
    /// embedded matte of the active photo: a rough mask. The panel says so.
    pub fn is_rough_ai_mask(&self, shape: &MaskShape) -> bool {
        let Some(c) = of(shape) else { return false };
        let none = matches!(shape, MaskShape::Subject { seg: None, .. } | MaskShape::Sky { seg: None, .. } | MaskShape::Background { seg: None, .. });
        none && !self.active().is_some_and(|id| self.media.has_matte(id, matte_kinds(c.kind)))
    }
}
