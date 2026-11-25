//! Brush strokes shared between copies of the settings (issue #370).
//!
//! A stroke's points and pressures are `Arc`s, so a copy of the settings (each History and undo step is one) shares
//! them. Settings read from a file are new copies; [`DevelopSettings::share_strokes_with`] makes the strokes that are
//! the same as an earlier step's share that step's points again, as if they had been painted in this session.

use std::sync::Arc;

use crate::{BrushStroke, DevelopSettings, MaskShape};

impl DevelopSettings {
    /// Make each brush stroke that holds exactly the points and pressures of the stroke at the same place in `earlier`
    /// (same mask id, same part, same position; Lens Blur's brushes too) share that stroke's. Nothing else changes:
    /// numbers are compared by their bits, so a sign of zero or a NaN is never swapped for another.
    pub fn share_strokes_with(&mut self, earlier: &DevelopSettings) {
        for mask in &mut self.masks {
            let Some(old) = earlier.masks.iter().find(|o| o.id == mask.id) else { continue };
            for (part, old_part) in mask.components.iter_mut().zip(&old.components) {
                if let (MaskShape::Brush { strokes }, MaskShape::Brush { strokes: old }) = (&mut part.shape, &old_part.shape) {
                    share_all(strokes, old);
                }
            }
        }
        share_all(&mut self.lens_blur.focus_strokes, &earlier.lens_blur.focus_strokes);
        share_all(&mut self.lens_blur.blur_strokes, &earlier.lens_blur.blur_strokes);
    }
}

fn share_all(strokes: &mut [BrushStroke], earlier: &[BrushStroke]) {
    for (s, e) in strokes.iter_mut().zip(earlier) {
        share(&mut s.points, &e.points, |a, b| a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits());
        share(&mut s.pressure, &e.pressure, |a: &f32, b: &f32| a.to_bits() == b.to_bits());
    }
}

fn share<T>(a: &mut Arc<[T]>, b: &Arc<[T]>, same: impl Fn(&T, &T) -> bool) {
    if !Arc::ptr_eq(a, b) && a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| same(x, y)) {
        *a = Arc::clone(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mask, MaskComponent};
    use lightcraft_geom::Point;

    fn painted(strokes: Vec<BrushStroke>) -> DevelopSettings {
        let part = MaskComponent { name: None, op: Default::default(), invert: false, shape: MaskShape::Brush { strokes } };
        DevelopSettings { masks: vec![Mask { id: 1, components: vec![part], ..Default::default() }], ..Default::default() }
    }

    fn stroke(x: f64) -> BrushStroke {
        BrushStroke { points: vec![Point::new(x, 0.5), Point::new(x + 0.1, 0.5)].into(), ..Default::default() }
    }

    fn points(d: &DevelopSettings, i: usize) -> Option<&Arc<[Point]>> {
        match &d.masks[0].components[0].shape {
            MaskShape::Brush { strokes } => Some(&strokes[i].points),
            _ => None,
        }
    }

    #[test]
    fn equal_strokes_share_and_a_changed_sign_of_zero_does_not() {
        let earlier = painted(vec![stroke(0.1), stroke(-0.0)]);
        let mut read = painted(vec![stroke(0.1), stroke(0.0), stroke(0.3)]);
        read.share_strokes_with(&earlier);
        assert!(Arc::ptr_eq(points(&read, 0).unwrap(), points(&earlier, 0).unwrap()), "the same stroke shares the earlier step's points");
        assert!(!Arc::ptr_eq(points(&read, 1).unwrap(), points(&earlier, 1).unwrap()), "0.0 is not -0.0: kept as read");
        assert_eq!(points(&read, 1).unwrap()[0].x.to_bits(), 0.0f64.to_bits());
    }
}
