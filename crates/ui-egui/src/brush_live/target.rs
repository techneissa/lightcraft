//! The look a live brush stroke paints toward: the photo's settings with the brush's coverage
//! forced to everything (painting) or nothing (erasing), so one render shows what full coverage of
//! the mask does and [`super::composite`] mixes it in under the brush.

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};

/// `d` with the brush coverage of mask `mask` (`None`: the mask the stroke will create, starting
/// from `fresh`) full when `erase` is false and empty when it's true. `None` when painting would
/// change nothing on screen: the mask has no adjustment, is inverted or edge-refined (the live
/// blend doesn't model those; the committed render does), or there's nothing to erase.
pub fn settings(d: &DevelopSettings, mask: Option<u32>, erase: bool, fresh: LocalAdjustments) -> Option<DevelopSettings> {
    let none = MaskShape::Brush { strokes: Vec::new() };
    let full = MaskComponent { name: None, op: MaskOp::Add, invert: true, shape: none.clone() };
    let mut out = d.clone();
    let existing = mask.and_then(|id| out.masks.iter().position(|m| m.id == id));
    let Some(i) = existing else {
        // the stroke will make the mask; painting shows its starting adjustment
        if erase || fresh == LocalAdjustments::default() {
            return None;
        }
        let id = out.next_mask_id();
        out.masks.push(Mask { id, adjust: fresh, components: vec![full], ..Default::default() });
        return Some(out);
    };
    let m = out.masks.get_mut(i)?;
    if m.invert || m.refine > 0.0 || m.adjust == LocalAdjustments::default() || !m.visible {
        return None;
    }
    // the command adds strokes to the last brush component
    let comp = m.components.iter_mut().rev().find(|c| matches!(c.shape, MaskShape::Brush { .. }));
    match (comp, erase) {
        (Some(c), false) => *c = MaskComponent { op: c.op, name: c.name.take(), ..full },
        (Some(c), true) => {
            c.shape = none;
            c.invert = false;
        }
        (None, false) => m.components.push(full),
        (None, true) => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::BrushStroke;
    use lightcraft_geom::Point;

    fn adjusted() -> LocalAdjustments {
        LocalAdjustments { exposure: 1.0, ..Default::default() }
    }

    fn with_mask(adjust: LocalAdjustments) -> DevelopSettings {
        let stroke = BrushStroke { points: vec![Point::new(0.5, 0.5)], ..Default::default() };
        let comp = MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes: vec![stroke] } };
        DevelopSettings { masks: vec![Mask { id: 3, adjust, components: vec![comp], ..Default::default() }], ..Default::default() }
    }

    fn coverage(d: &DevelopSettings) -> f32 {
        use lightcraft_pipeline::{geometry::Frame, masks::evaluate_one};
        use lightcraft_raster::{Plane, Rgb32f};
        let (w, h) = (40, 30);
        let a = evaluate_one(&d.masks[0], &Frame::new(w, h, &Default::default(), true), w, h, &Rgb32f::new(w, h), &Plane::new(w, h), 0.0, None);
        a.data.iter().sum::<f32>() / (w * h) as f32
    }

    #[test]
    fn painting_covers_everything_and_erasing_nothing() {
        let d = with_mask(adjusted());
        assert!(coverage(&d) < 0.5, "the stroke alone covers a small part");
        let full = settings(&d, Some(3), false, LocalAdjustments::default()).unwrap();
        assert!((coverage(&full) - 1.0).abs() < 1e-6);
        let empty = settings(&d, Some(3), true, LocalAdjustments::default()).unwrap();
        assert_eq!(coverage(&empty), 0.0);
        // the adjustment itself, and the photo's other settings, are untouched
        assert_eq!(full.masks[0].adjust, d.masks[0].adjust);
        assert_eq!(full.masks.len(), 1);
    }

    #[test]
    fn a_stroke_that_would_show_nothing_asks_for_no_render() {
        assert!(settings(&with_mask(LocalAdjustments::default()), Some(3), false, LocalAdjustments::default()).is_none());
        assert!(settings(&DevelopSettings::default(), None, false, LocalAdjustments::default()).is_none());
        assert!(settings(&DevelopSettings::default(), None, true, adjusted()).is_none());
        let mut inverted = with_mask(adjusted());
        inverted.masks[0].invert = true;
        assert!(settings(&inverted, Some(3), false, adjusted()).is_none());
        let mut refined = with_mask(adjusted());
        refined.masks[0].refine = 30.0;
        assert!(settings(&refined, Some(3), false, adjusted()).is_none());
    }

    #[test]
    fn the_first_stroke_of_a_new_mask_previews_its_starting_adjustment() {
        let d = DevelopSettings::default();
        let full = settings(&d, None, false, adjusted()).unwrap();
        assert_eq!(full.masks.len(), 1);
        assert_eq!(full.masks[0].id, 1);
        assert!((coverage(&full) - 1.0).abs() < 1e-6);
        // a mask without a brush component gets one that covers everything; a subtracting brush keeps its op
        let mut no_brush = with_mask(adjusted());
        no_brush.masks[0].components.clear();
        assert_eq!(settings(&no_brush, Some(3), false, adjusted()).unwrap().masks[0].components.len(), 1);
        assert!(settings(&no_brush, Some(3), true, adjusted()).is_none());
        let mut sub = with_mask(adjusted());
        sub.masks[0].components[0].op = MaskOp::Subtract;
        assert_eq!(settings(&sub, Some(3), false, adjusted()).unwrap().masks[0].components[0].op, MaskOp::Subtract);
    }
}
