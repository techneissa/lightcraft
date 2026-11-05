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

/// One kind of selection the model computes. `text` and `main_only` are read only with the `sam` feature.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub struct Concept {
    /// `subject`, `sky` or `background`.
    pub kind: &'static str,
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    pub text: &'static str,
    /// The found selection is turned around (Background is everything but the subject).
    pub invert: bool,
    /// Only the main subject's instances count (Subject and Background): the largest and any of at least a third of
    /// its area, not every small figure far away.
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    pub main_only: bool,
}

/// The concept a classical shape stands for.
pub fn of(shape: &MaskShape) -> Option<Concept> {
    match shape {
        MaskShape::Subject { .. } => Some(Concept { kind: "subject", text: SUBJECT, invert: false, main_only: true }),
        MaskShape::Sky { .. } => Some(Concept { kind: "sky", text: SKY, invert: false, main_only: false }),
        MaskShape::Background { .. } => Some(Concept { kind: "background", text: SUBJECT, invert: true, main_only: true }),
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
    s.segmenter.can_select(c.kind) && !s.media.has_matte(photo, matte_kinds(c.kind))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lightcraft_develop::DevelopSettings;
    use lightcraft_pipeline::masks::Mattes;
    use lightcraft_raster::{Image, Rgb32f};
    use serde_json::json;

    use super::*;
    use crate::media::SourceRef;
    use lightcraft_pipeline::SourceInfo;

    /// A session that has SAM 3 (a folder that looks installed) and decodes the active photo into the cache, with
    /// `mattes` when given.
    fn session(name: &str, mattes: Option<Mattes>) -> (Session, PhotoId, std::path::PathBuf) {
        let mut s = Session::with_demo();
        s.segmenter.background = true;
        let dir = std::env::temp_dir().join(format!("lc-concepts-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"\x10\x00\x00\x00\x00\x00\x00\x00{}").unwrap();
        std::fs::write(dir.join("vocab.json"), b"{}").unwrap();
        std::fs::write(dir.join("merges.txt"), b"#version: 0.2\n").unwrap();
        s.segmenter.dir = Some(dir.clone());
        let id = s.active().unwrap();
        let info = SourceInfo { mattes: mattes.map(Arc::new), ..Default::default() };
        let mut job = s.render_job(id, 64, 64, false, true).unwrap();
        job.source = SourceRef::File {
            path: "synthetic.dng".into(),
            max_edge: 64,
            loader: Some(Arc::new(move |_, _, _, _| Ok((Rgb32f::new(64, 64), info.clone())))),
            fallback: None,
            denoise: None,
            camera_profile: None,
            reduced: None,
            options: Default::default(),
        };
        job.settings = Arc::new(DevelopSettings::default());
        let r = job.run();
        s.accept(&r);
        (s, id, dir)
    }

    fn matte() -> Mattes {
        let mut m = Mattes::default();
        m.push(MatteKind::Sky, Image::from_fn(4, 4, |_, _| 255u8));
        m
    }

    /// A photo with an embedded sky matte keeps it (no model run, no queued job); its Subject, with no matte for it,
    /// goes to the model.
    #[test]
    fn an_embedded_matte_is_never_replaced_by_the_model() {
        let (mut s, id, dir) = session("matte", Some(matte()));
        assert!(!wanted(&s, id, &MaskShape::sky()), "the sky matte wins");
        assert!(wanted(&s, id, &MaskShape::subject()), "no person matte: the model");
        assert!(!s.is_rough_ai_mask(&MaskShape::sky()), "a matte is not a rough mask");
        let r = s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
        assert!(r.get("pending").is_none() && !s.segmenter.busy(), "added at once, from the matte: {r}");
        assert!(matches!(&s.develop_of(id).unwrap().masks[0].components[0].shape, MaskShape::Sky { seg: None, .. }));
        let r = s.execute("mask.add", &json!({"kind": "subject"})).unwrap();
        assert_eq!(r["pending"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without a matte a Sky stays rough until SAM 3 has run; with SAM 3 missing the part is rough and says so.
    #[test]
    fn a_part_the_heuristic_draws_is_rough() {
        let (mut s, id, dir) = session("rough", None);
        assert!(wanted(&s, id, &MaskShape::sky()));
        assert!(s.is_rough_ai_mask(&MaskShape::sky()), "from before: no stored selection");
        let found = lightcraft_develop::SegMask::from_logits(4, &[5.0; 16]);
        assert!(!s.is_rough_ai_mask(&MaskShape::Sky { seg: Some(found), detail: vec![], edge: 0.0 }));
        assert!(!s.is_rough_ai_mask(&MaskShape::Brush { strokes: vec![] }));
        // no model: the mask is made the current way, at once
        s.segmenter.dir = None;
        assert!(!wanted(&s, id, &MaskShape::sky()));
        let r = s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
        assert!(r.get("pending").is_none());
        let shape = s.develop_of(id).unwrap().masks[0].components[0].shape.clone();
        assert!(s.is_rough_ai_mask(&shape));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mask saved before (`{"kind": "sky"}`, no selection) opens as a rough Sky and is saved back the same way.
    #[test]
    fn masks_saved_before_keep_their_look() {
        for (json, shape) in [
            (r#"{"kind":"sky"}"#, MaskShape::sky()),
            (r#"{"kind":"subject"}"#, MaskShape::subject()),
            (r#"{"kind":"background"}"#, MaskShape::background()),
        ] {
            let read: MaskShape = serde_json::from_str(json).unwrap();
            assert_eq!(read, shape);
            assert_eq!(serde_json::to_string(&read).unwrap(), json, "saved back unchanged");
        }
    }
}
