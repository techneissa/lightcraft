//! Fork (`fork/adaptive-masks.md`, issue #239): AI mask parts that adapt to each photo.
//!
//! An AI part (Subject, Sky, Background, Object, Describe, Select People) keeps what it selects (its kind and
//! parameters: the description, the clicks, the parts and whose) apart from what a model found on one photo (`seg`
//! and `detail`). A preset, a paste or a sync carries what it selects; on another photo the part is marked
//! [waiting](SegMask::waiting) until that photo's own selection is found (`lightcraft_engine::adaptive`). A waiting
//! part selects nothing, and a mask with one doesn't render at all ([`Mask::renders`]), so the source photo's selection
//! never shows on another photo.

use crate::{DevelopSettings, Mask, MaskShape, SegMask};

/// The [`SegMask::model`] of a selection still to be found on its photo.
pub const WAITING: &str = "waiting";

impl SegMask {
    /// A selection still to be found on this photo. It selects nothing (a build that doesn't know the mark draws it
    /// as nothing too).
    pub fn waiting() -> SegMask {
        SegMask::nothing().made_by(WAITING)
    }

    /// A selection of nothing: a model found nothing on the photo.
    pub fn nothing() -> SegMask {
        SegMask::from_logits(2, &[-16.0; 4])
    }

    /// A selection still to be found again on the photo it was found on (Update AI Masks): it keeps this one, which
    /// comes back when the new search fails or finds nothing ([`SegMask::previous`]), so a selection the user had is
    /// never lost. A build that doesn't know the mark draws the kept selection.
    pub fn waiting_over(self) -> SegMask {
        let model = format!("{WAITING}+{}", self.model.as_deref().unwrap_or_default());
        SegMask { model: Some(model), ..self }
    }

    /// Whether this selection is still to be found ([`SegMask::waiting`], [`SegMask::waiting_over`]).
    pub fn is_waiting(&self) -> bool {
        self.model.as_deref().is_some_and(|m| m == WAITING || m.starts_with(KEPT))
    }

    /// The selection a [`SegMask::waiting_over`] mark keeps.
    pub fn previous(&self) -> Option<SegMask> {
        let rest = self.model.as_deref()?.strip_prefix(KEPT)?;
        Some(SegMask { model: (!rest.is_empty()).then(|| rest.to_string()), ..self.clone() })
    }
}

/// How the [`SegMask::model`] of a [`SegMask::waiting_over`] mark starts.
const KEPT: &str = "waiting+";

impl MaskShape {
    /// Whether a model finds what this part selects, on each photo: Subject, Sky, Background, Object, Describe and
    /// Select People.
    pub fn is_detected(&self) -> bool {
        matches!(
            self,
            MaskShape::Subject { .. }
                | MaskShape::Sky { .. }
                | MaskShape::Background { .. }
                | MaskShape::Object { .. }
                | MaskShape::Prompt { .. }
                | MaskShape::People { .. }
        )
    }

    /// What this AI part selects, without what was found on one photo (no `seg`, no `detail`) and without its Edge
    /// (a render setting): two parts with the same recipe find the same selection on a photo. `None` for other parts.
    pub fn recipe(&self) -> Option<MaskShape> {
        if !self.is_detected() {
            return None;
        }
        let mut r = self.clone();
        if let Some((seg, detail, edge)) = r.found_mut() {
            *seg = None;
            detail.clear();
            *edge = 0.0;
        }
        Some(r)
    }

    /// Whether this AI part is still to be found on its photo.
    pub fn is_waiting(&self) -> bool {
        self.seg().is_some_and(SegMask::is_waiting)
    }

    /// Mark this AI part as still to be found on its photo (nothing for other parts).
    pub fn set_waiting(&mut self) {
        self.set_found(Some(SegMask::waiting()), Vec::new());
    }

    /// Mark this AI part as still to be found again on its own photo (Update AI Masks), keeping what it has: a
    /// search that fails gives it back ([`SegMask::waiting_over`]). One with nothing found yet just waits.
    pub fn set_waiting_again(&mut self) {
        match self.seg().filter(|s| !s.is_waiting()).cloned() {
            Some(prev) => {
                if let Some((seg, _, _)) = self.found_mut() {
                    *seg = Some(prev.waiting_over());
                }
            }
            None => self.set_waiting(),
        }
    }

    /// Store what a model found for this AI part (`None`: drawn the classical way; nothing for other parts).
    pub fn set_found(&mut self, found: Option<SegMask>, more: Vec<SegMask>) {
        if let Some((seg, detail, _)) = self.found_mut() {
            *seg = found;
            *detail = more;
        }
    }

    /// The found selection, its finer passes and the Edge of an AI part.
    fn found_mut(&mut self) -> Option<(&mut Option<SegMask>, &mut Vec<SegMask>, &mut f64)> {
        match self {
            MaskShape::Subject { seg, detail, edge }
            | MaskShape::Sky { seg, detail, edge }
            | MaskShape::Background { seg, detail, edge }
            | MaskShape::Object { seg, detail, edge, .. }
            | MaskShape::Prompt { seg, detail, edge, .. }
            | MaskShape::People { seg, detail, edge, .. } => Some((seg, detail, edge)),
            _ => None,
        }
    }

    /// The found selection and its finer passes, copied (AI parts only).
    fn found(&self) -> Option<(Option<SegMask>, Vec<SegMask>)> {
        let mut me = self.clone();
        me.found_mut().map(|(s, d, _)| (s.take(), std::mem::take(d)))
    }
}

impl Mask {
    /// Whether a part of this mask is still to be found on its photo.
    pub fn is_waiting(&self) -> bool {
        self.components.iter().any(|c| c.shape.is_waiting())
    }

    /// Whether a render draws this mask: visible, with parts, none of them still to be found (until then the mask
    /// shows nothing, neither the part nor its inverse).
    pub fn renders(&self) -> bool {
        self.visible && !self.components.is_empty() && !self.is_waiting()
    }
}

/// One AI part still to be found on a photo: its mask, its place in the mask and what it selects.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub mask: u32,
    pub component: usize,
    pub recipe: MaskShape,
}

impl DevelopSettings {
    /// The AI parts still to be found on the photo, in mask order.
    pub fn waiting(&self) -> Vec<Waiting> {
        let mut out = Vec::new();
        for m in &self.masks {
            for (i, c) in m.components.iter().enumerate() {
                if c.shape.is_waiting()
                    && let Some(recipe) = c.shape.recipe()
                {
                    out.push(Waiting { mask: m.id, component: i, recipe });
                }
            }
        }
        out
    }

    /// Mark every AI part as still to be found again (Update AI Masks), each keeping what it has until then
    /// ([`MaskShape::set_waiting_again`]); `keep` says which parts may be (a part no model on this computer can find
    /// keeps what it has). How many were marked.
    pub fn set_ai_waiting(&mut self, keep: impl Fn(&MaskShape) -> bool) -> usize {
        let mut n = 0;
        for m in &mut self.masks {
            for c in &mut m.components {
                if c.shape.is_detected() && keep(&c.shape) {
                    c.shape.set_waiting_again();
                    n += 1;
                }
            }
        }
        n
    }
}

/// `incoming`, settings another photo's masks reach photo `own`'s with (Paste, Sync, Auto Sync): each AI part keeps
/// `own`'s selection when `own` has the same part found already (the same mask, place and recipe, the photo turned
/// the same way), else it waits to be found on this photo. How many wait.
pub fn adapt(own: Option<&DevelopSettings>, incoming: &mut DevelopSettings) -> usize {
    let turned_alike = own.is_some_and(|o| o.orientation == incoming.orientation);
    let mut waiting = 0;
    for m in &mut incoming.masks {
        let theirs = own.filter(|_| turned_alike).and_then(|o| o.masks.iter().find(|q| q.id == m.id));
        for (i, c) in m.components.iter_mut().enumerate() {
            let Some(recipe) = c.shape.recipe() else { continue };
            let kept = theirs
                .and_then(|q| q.components.get(i))
                .map(|q| &q.shape)
                .filter(|q| !q.is_waiting() && q.recipe().as_ref() == Some(&recipe))
                .and_then(MaskShape::found);
            match kept {
                Some((seg, detail)) => c.shape.set_found(seg, detail),
                None => {
                    c.shape.set_waiting();
                    waiting += 1;
                }
            }
        }
    }
    waiting
}

/// Leave only what each AI part selects (presets: a preset never carries one photo's selection).
pub fn strip_found(masks: &mut [Mask]) {
    for m in masks {
        for c in &mut m.components {
            if c.shape.is_detected() {
                c.shape.set_found(None, Vec::new());
            }
        }
    }
}

/// Whether two lists of parts are the same apart from what was found for their AI parts.
pub fn same_parts(a: &[crate::MaskComponent], b: &[crate::MaskComponent]) -> bool {
    let plain = |c: &crate::MaskComponent| {
        let mut c = c.clone();
        if c.shape.is_detected() {
            c.shape.set_found(None, Vec::new());
        }
        c
    };
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| plain(x) == plain(y))
}

#[cfg(test)]
mod tests;
