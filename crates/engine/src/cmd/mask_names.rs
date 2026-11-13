//! The generated name of a new mask (issue #37): "<Kind> Mask <N>", the kind being its first part's ("Subject Mask 1",
//! "Sky Mask 1", "Describe Mask 2"…). N is the lowest number no mask of the photo uses for that kind, so deleting
//! "Subject Mask 1" and adding a Subject gives "Subject Mask 1" again, never a duplicate. A name the user typed, or a
//! saved mask's, is never touched.

use lightcraft_develop::{Mask, MaskShape};

/// What a mask kind (the `kind` of `mask.add`) is called in a generated name; the same words as the Create New Mask
/// tiles.
fn label(kind: &str) -> &'static str {
    match kind {
        "subject" => "Subject",
        "sky" => "Sky",
        "background" => "Background",
        "object" => "Object",
        "prompt" => "Describe",
        "brush" => "Brush",
        "linear" => "Linear",
        "radial" => "Radial",
        "luminanceRange" => "Luminance",
        "colorRange" => "Color",
        "depthRange" => "Depth",
        "people" => "People",
        "landscape" => "Landscape",
        _ => "New",
    }
}

/// The `kind` of `mask.add` that makes a part of this shape (the key of [`label`]).
pub fn kind_of(shape: &MaskShape) -> &'static str {
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

/// The name part `k` of `mask` is shown by (issue #61): the user's own name when it has one, else "<Kind> Submask
/// <n>" with n counted per kind inside the mask ("Subject Submask 1", "Brush Submask 2"; Linear, Radial, Luminance, Color
/// for the longer kinds, so a name fits its row); a Describe part keeps its
/// words (“red car” Submask). Worked out when shown, never stored, so parts saved earlier read the same way and a
/// photo is written only when a part is renamed.
pub fn submask_name(mask: &Mask, k: usize) -> String {
    let Some(c) = mask.components.get(k) else { return format!("Submask {}", k + 1) };
    if let Some(name) = &c.name {
        return name.clone();
    }
    if let MaskShape::Prompt { text, .. } = &c.shape {
        return format!("“{text}” Submask");
    }
    let kind = std::mem::discriminant(&c.shape);
    let n = mask.components.iter().take(k + 1).filter(|o| std::mem::discriminant(&o.shape) == kind).count();
    format!("{} Submask {n}", label(kind_of(&c.shape)))
}

/// The name for a new mask of `kind` among `masks`.
pub fn next(masks: &[Mask], kind: &str) -> String {
    let label = label(kind);
    let name = |n: u32| format!("{label} Mask {n}");
    let n = (1..).find(|n| !masks.iter().any(|m| m.name == name(*n))).unwrap_or(1);
    name(n)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn names(s: &Session) -> Vec<String> {
        let id = s.active().unwrap();
        s.develop_of(id).unwrap().masks.iter().map(|m| m.name.clone()).collect()
    }

    /// Every kind names its mask after the kind; a number is the lowest free one for that kind; a name the user
    /// gave stays.
    #[test]
    fn new_masks_are_named_after_their_first_part() {
        let mut s = Session::with_demo();
        let kinds = [
            ("subject", "Subject"),
            ("sky", "Sky"),
            ("background", "Background"),
            ("object", "Object"),
            ("brush", "Brush"),
            ("linear", "Linear"),
            ("radial", "Radial"),
            ("luminanceRange", "Luminance"),
            ("colorRange", "Color"),
        ];
        for (kind, label) in kinds {
            // (an Object needs its clicks and selection: no model is needed for the name)
            let seg = lightcraft_develop::SegMask::from_logits(4, &[5.0; 16]);
            let params = if kind == "object" { json!({"kind": kind, "points": [[0.5, 0.5]], "seg": seg}) } else { json!({"kind": kind}) };
            s.execute("mask.add", &params).unwrap();
            assert_eq!(names(&s).last().unwrap(), &format!("{label} Mask 1"), "{kind}");
        }
        let prompt = json!({"kind": "prompt", "text": "sky", "seg": lightcraft_develop::SegMask::from_logits(4, &[5.0; 16])});
        s.execute("mask.add", &prompt).unwrap();
        assert_eq!(names(&s).last().unwrap(), "Describe Mask 1");

        // a second Sky is number 2; deleting the first and adding another gives the lowest unused number
        s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
        assert_eq!(names(&s).last().unwrap(), "Sky Mask 2");
        let first_sky = {
            let id = s.active().unwrap();
            s.develop_of(id).unwrap().masks.iter().find(|m| m.name == "Sky Mask 1").unwrap().id
        };
        s.execute("mask.delete", &json!({"id": first_sky})).unwrap();
        s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
        assert_eq!(names(&s).last().unwrap(), "Sky Mask 1");
        s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
        assert_eq!(names(&s).last().unwrap(), "Sky Mask 3");
        let all = names(&s);
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(all.len(), unique.len(), "no duplicate names: {all:?}");

        // a name the user gave is kept, and does not take a number
        s.execute("mask.add", &json!({"kind": "sky", "name": "Sky Mask 4"})).unwrap();
        s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
        assert_eq!(names(&s).last().unwrap(), "Sky Mask 5", "4 is taken by the user's own name");
    }

    /// A new brush mask made by painting is a Brush Mask too.
    #[test]
    fn a_stroke_that_starts_a_mask_names_it_a_brush_mask() {
        let mut s = Session::with_demo();
        s.execute("mask.brushStroke", &json!({"points": [[0.3, 0.3], [0.4, 0.4]]})).unwrap();
        s.execute("mask.add", &json!({"kind": "brush"})).unwrap();
        assert_eq!(names(&s), vec!["Brush Mask 1".to_string(), "Brush Mask 2".to_string()]);
    }

    /// A part is "<Kind> Submask <n>", n counted per kind; the user's name and a Describe part's words stay.
    #[test]
    fn submasks_are_named_by_kind_and_count() {
        let mut s = Session::with_demo();
        s.execute("mask.add", &json!({"kind": "subject"})).unwrap();
        for kind in ["brush", "luminanceRange", "brush", "linear"] {
            s.execute("mask.addComponent", &json!({"kind": kind})).unwrap();
        }
        let shown = |s: &Session| {
            let m = s.develop_of(s.active().unwrap()).unwrap().masks[0].clone();
            (0..m.components.len()).map(|k| super::submask_name(&m, k)).collect::<Vec<_>>()
        };
        let want = ["Subject Submask 1", "Brush Submask 1", "Luminance Submask 1", "Brush Submask 2", "Linear Submask 1"];
        assert_eq!(shown(&s), want);
        // a user's name stays, and does not change the count of its kind
        s.execute("mask.component", &json!({"component": 1, "action": "rename", "name": "Hair"})).unwrap();
        assert_eq!(shown(&s), ["Subject Submask 1", "Hair", "Luminance Submask 1", "Brush Submask 2", "Linear Submask 1"]);
        // nothing was written for the others
        let d = s.develop_of(s.active().unwrap()).unwrap();
        assert!(d.masks[0].components.iter().enumerate().all(|(k, c)| (k == 1) == c.name.is_some()));
        // a Describe part keeps its words
        let m = lightcraft_develop::Mask {
            components: vec![lightcraft_develop::MaskComponent {
                name: None,
                op: lightcraft_develop::MaskOp::Add,
                invert: false,
                shape: lightcraft_develop::MaskShape::Prompt { text: "red car".into(), seg: None, detail: vec![], edge: 0.0, region: None },
            }],
            ..Default::default()
        };
        assert_eq!(super::submask_name(&m, 0), "“red car” Submask");
    }
}
