//! Invert Mask (issue #72): the whole mask selects the opposite of what it selected before.
//!
//! `evaluate_one` (pipeline) turns only the base (first) part around when `Mask.invert` is on, so the later parts work
//! on the inverted mask. To make the whole mask the opposite, each later part changes with it, in the same edit:
//!
//! | before          | after                                  | because                               |
//! | --------------- | -------------------------------------- | ------------------------------------- |
//! | Add C           | Subtract C                             | not (X or C) = (not X) without C      |
//! | Subtract C      | Add C                                  | not (X without C) = (not X) or C      |
//! | Intersect C     | Add C, the part's own Invert flipped   | not (X and C) = (not X) or (not C)    |

use lightcraft_develop::{Mask, MaskOp};

/// Turns `m` around: `Mask.invert` flips and every part after the first changes as the table above says. Saved masks
/// are never rewritten, only a call of this is.
pub(super) fn invert(m: &mut Mask) {
    m.invert = !m.invert;
    for c in m.components.iter_mut().skip(1) {
        match c.op {
            MaskOp::Add => c.op = MaskOp::Subtract,
            MaskOp::Subtract => c.op = MaskOp::Add,
            MaskOp::Intersect => {
                c.op = MaskOp::Add;
                c.invert = !c.invert;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use lightcraft_develop::{DevelopSettings, MaskComponent, MaskShape};
    use lightcraft_geom::Point;
    use lightcraft_pipeline::geometry::Frame;
    use lightcraft_pipeline::masks::evaluate_one;
    use lightcraft_raster::{Plane, Rgb32f};
    use serde_json::json;

    use super::*;
    use crate::Session;

    const W: usize = 100;
    const H: usize = 40;

    /// A hard-edged shape: the part of the photo left of `at` (`vertical`: above it).
    fn half(at: f64, vertical: bool) -> MaskShape {
        if vertical {
            MaskShape::Linear { start: Point::new(0.5, at), end: Point::new(0.5, at + 0.001) }
        } else {
            MaskShape::Linear { start: Point::new(at, 0.5), end: Point::new(at + 0.001, 0.5) }
        }
    }

    fn part(op: MaskOp, invert: bool, shape: MaskShape) -> MaskComponent {
        MaskComponent { name: None, op, invert, shape }
    }

    fn alpha(m: &Mask) -> Plane {
        let img = Rgb32f::from_fn(W, H, |_, _| [0.5; 3]);
        let log_l = Plane::from_fn(W, H, |_, _| 0.5f32.log2());
        let f = Frame::new(W, H, &DevelopSettings::default(), true);
        evaluate_one(m, &f, W, H, &img, &log_l, 0.0, None)
    }

    /// Every pixel of `after` is the opposite of `before`'s.
    fn assert_opposite(before: &Plane, after: &Plane, what: &str) {
        for y in 0..H {
            for x in 0..W {
                let (b, a) = (before.get(x, y), after.get(x, y));
                assert!((a - (1.0 - b)).abs() < 1e-4, "{what}: pixel {x},{y} was {b}, is {a}");
            }
        }
    }

    /// The base, then every mode of the second and third part (with and without the part's own Invert), on a mask
    /// that is not inverted and one that is: Invert selects exactly the opposite, and doing it twice the same area.
    #[test]
    fn invert_selects_the_opposite_of_every_mix_of_modes() {
        let ops = [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect];
        for mask_invert in [false, true] {
            for (o1, o2) in ops.iter().flat_map(|a| ops.iter().map(move |b| (*a, *b))) {
                for own in [false, true] {
                    let m = Mask {
                        invert: mask_invert,
                        components: vec![
                            part(MaskOp::Add, false, half(0.5, false)),
                            part(o1, own, half(0.75, false)),
                            part(o2, false, half(0.2, true)),
                        ],
                        ..Default::default()
                    };
                    let what = format!("invert={mask_invert} {o1:?} (own invert {own}) then {o2:?}");
                    let before = alpha(&m);
                    let mut once = m.clone();
                    invert(&mut once);
                    let after = alpha(&once);
                    assert_opposite(&before, &after, &what);
                    let mut twice = once.clone();
                    invert(&mut twice);
                    assert_eq!(alpha(&twice).data, before.data, "{what}: twice selects the same area");
                }
            }
        }
    }

    /// The case of the issue: two Add parts. After Invert the second part's area is excluded too.
    #[test]
    fn invert_of_two_add_parts_excludes_both() {
        let m =
            Mask { components: vec![part(MaskOp::Add, false, half(0.25, false)), part(MaskOp::Add, false, half(0.75, false))], ..Default::default() };
        // (the second part's own area is x < 0.75, which contains the first's)
        let mut inv = m.clone();
        invert(&mut inv);
        assert_eq!(inv.components[1].op, MaskOp::Subtract);
        let a = alpha(&inv);
        assert!(a.get(10, 20) < 0.01, "the first part's area");
        assert!(a.get(60, 20) < 0.01, "the second part's area");
        assert!(a.get(90, 20) > 0.99, "outside both");
    }

    /// An Intersect part comes back as Add with its own Invert on.
    #[test]
    fn an_intersect_part_becomes_add_with_its_own_invert_flipped() {
        let mut m = Mask {
            components: vec![part(MaskOp::Add, false, half(0.5, false)), part(MaskOp::Intersect, false, half(0.25, false))],
            ..Default::default()
        };
        invert(&mut m);
        assert!(m.invert);
        assert_eq!((m.components[1].op, m.components[1].invert), (MaskOp::Add, true));
        invert(&mut m);
        assert_eq!((m.components[1].op, m.components[1].invert), (MaskOp::Subtract, true));
    }

    fn two_part_session() -> Session {
        let mut s = Session::with_demo();
        s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
        s.execute("mask.addComponent", &json!({"kind": "radial", "op": "add", "center": [0.8, 0.5]})).unwrap();
        s
    }

    fn masks(s: &Session) -> Vec<Mask> {
        s.develop_of(s.active().unwrap()).unwrap().masks.clone()
    }

    /// `mask.invert` rewrites the later parts in one undo step.
    #[test]
    fn the_command_inverts_the_parts_in_one_undo_step() {
        let mut s = two_part_session();
        let before = masks(&s);
        s.execute("mask.invert", &json!({})).unwrap();
        let after = masks(&s);
        assert!(after[0].invert);
        assert_eq!(after[0].components[1].op, MaskOp::Subtract);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(masks(&s), before, "one undo step puts the mask back");
    }

    /// Duplicate and Invert gives the copy the inverted parts and leaves the original alone.
    #[test]
    fn duplicate_and_invert_inverts_the_copy_only() {
        let mut s = two_part_session();
        let before = masks(&s);
        s.execute("mask.duplicate", &json!({"invert": true})).unwrap();
        let after = masks(&s);
        assert_eq!(after[0], before[0]);
        assert!(after[1].invert);
        assert_eq!(after[1].components[1].op, MaskOp::Subtract);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(masks(&s), before);
    }
}
