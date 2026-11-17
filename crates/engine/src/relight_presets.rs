//! Fork (issue #301, `fork/relight.md`): light presets in the session. The presets themselves, and the numbers that
//! place them round a subject, are `lightcraft_develop::relight_presets`; this finds the photo's subject, keeps the
//! user's own presets (saved in the library's presets file) and applies one.
//!
//! The subject is a face: the first of the photo's face regions (the face scan's, else those portrait retouching found;
//! the largest, with any about as large), a face worked out from what a person or subject mask of the photo selects, a
//! face at the photo's centre. Nothing is computed for the purpose: no
//! model runs and no pixel is read.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight_presets::{LightPreset, MAX_NAME, MAX_USER, Subject, built_in, selection_box};
use lightcraft_develop::{MaskShape, SegMask};
use lightcraft_meta::RegionKind;

use crate::Session;

/// What the subject was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectSource {
    /// A person or subject mask of the photo.
    Mask,
    /// The photo's faces.
    Face,
    /// Nothing found: the photo's centre.
    Centre,
}

impl SubjectSource {
    pub fn id(self) -> &'static str {
        match self {
            SubjectSource::Mask => "mask",
            SubjectSource::Face => "face",
            SubjectSource::Centre => "centre",
        }
    }
}

/// The presets read from a library's file: none without an id and a name, none whose id repeats (or is a built-in one's)
/// or whose name is a built-in preset's or an earlier one's (letter case aside), at most [`MAX_USER`], each as a user preset,
/// its name cut to [`MAX_NAME`] characters.
pub fn read_user(list: Vec<LightPreset>) -> Vec<LightPreset> {
    let mut ids: Vec<String> = built_in().iter().map(|p| p.id.clone()).collect();
    let mut names: Vec<String> = built_in().iter().map(|p| p.name.to_lowercase()).collect();
    let mut out = Vec::new();
    for mut p in list {
        let Some(name) = lightcraft_develop::relight_presets::clean_name(&p.name) else { continue };
        let name: String = name.chars().take(MAX_NAME).collect();
        let name = name.trim().to_string();
        if p.id.trim().is_empty() || ids.contains(&p.id) || names.contains(&name.to_lowercase()) || out.len() >= MAX_USER {
            continue;
        }
        p.name = name;
        p.builtin = false;
        ids.push(p.id.clone());
        names.push(p.name.to_lowercase());
        out.push(p);
    }
    out
}

/// The union of boxes (`[x0, y0, x1, y1]`).
fn union(boxes: impl IntoIterator<Item = [f64; 4]>) -> Option<[f64; 4]> {
    boxes.into_iter().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

/// The box a mask's selection covers, over the photo's masks that select a person or the subject (not inverted).
fn mask_box(d: &DevelopSettings) -> Option<[f64; 4]> {
    fn seg_of(shape: &MaskShape) -> Option<&SegMask> {
        match shape {
            MaskShape::Subject { seg, .. } | MaskShape::People { seg, .. } => seg.as_ref(),
            _ => None,
        }
    }
    union(
        d.masks
            .iter()
            .filter(|m| m.visible && !m.invert)
            .flat_map(|m| m.components.iter())
            .filter(|c| !c.invert && c.op == lightcraft_develop::MaskOp::Add)
            .filter_map(|c| seg_of(&c.shape).and_then(selection_box)),
    )
}

/// The faces of a photo as boxes in the photo's frame: the largest, and any about as large (a group), as one box.
fn face_box(faces: &[[f64; 4]]) -> Option<[f64; 4]> {
    let area = |f: &[f64; 4]| (f[2] - f[0]).max(0.0) * (f[3] - f[1]).max(0.0);
    let biggest = faces.iter().map(area).fold(0.0, f64::max);
    if biggest <= 0.0 {
        return None;
    }
    union(faces.iter().filter(|f| area(f) >= 0.4 * biggest).copied())
}

impl Session {
    /// Where photo `id`'s subject is (see the module docs) and what it was taken from.
    pub fn relight_subject(&self, id: PhotoId) -> (Subject, SubjectSource) {
        let (s, source) = self.relight_subject_box(id);
        (self.with_subject_depth(id, s), source)
    }

    /// [`Session::relight_subject`] without its depth.
    fn relight_subject_box(&self, id: PhotoId) -> (Subject, SubjectSource) {
        let Some(p) = self.catalog.photo(id) else { return (Subject::CENTRE, SubjectSource::Centre) };
        let mut faces: Vec<[f64; 4]> = p
            .meta
            .regions
            .iter()
            .filter(|r| r.kind == RegionKind::Face)
            .map(|r| {
                let b = p.develop.orientation.map_norm_rect(r.rect);
                [b.x0, b.y0, b.x1, b.y1]
            })
            .collect();
        if faces.is_empty()
            && let Some(found) = self.retouch_faces_of(id).found
        {
            faces = found.input.faces.iter().map(|f| [f.bounds.x0, f.bounds.y0, f.bounds.x1, f.bounds.y1]).collect();
        }
        if let Some(s) = face_box(&faces).and_then(Subject::from_face) {
            return (s, SubjectSource::Face);
        }
        match mask_box(&p.develop).and_then(Subject::from_mask_box) {
            Some(s) => (s, SubjectSource::Mask),
            None => (Subject::CENTRE, SubjectSource::Centre),
        }
    }

    /// `s` with its depth: the median depth of what its box holds in photo `id`'s field, when the field is made.
    fn with_subject_depth(&self, id: PhotoId, s: Subject) -> Subject {
        let Some(made) = self.relight_made(id) else { return s };
        let field = &made.input.field;
        let b = [s.x - s.w / 2.0, s.y - s.h / 2.0, s.x + s.w / 2.0, s.y + s.h / 2.0];
        match field.depth_in(b) {
            Some(lz) => {
                let (d, span) = field.light_depth_of(lz);
                s.with_depth(100.0 * d, span)
            }
            None => s,
        }
    }

    /// Every preset: the built-in ones, then the user's own.
    pub fn relight_preset_list(&self) -> Vec<LightPreset> {
        built_in().iter().chain(&self.relight_presets).cloned().collect()
    }

    /// The preset `key` names: its id, else its name (letter case aside).
    pub fn relight_preset(&self, key: &str) -> Option<LightPreset> {
        let all = || built_in().iter().chain(&self.relight_presets);
        let key = key.trim();
        all().find(|p| p.id == key).or_else(|| all().find(|p| p.name.eq_ignore_ascii_case(key))).cloned()
    }
}

#[cfg(test)]
mod tests;
