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
//!
//! A second Invert puts every part back exactly as it was (`Mask.before_invert`).

use lightcraft_develop::{Mask, MaskOp};

/// The table's rewrite of one part's (mode, own Invert).
fn turned((op, own): (MaskOp, bool)) -> (MaskOp, bool) {
    match op {
        MaskOp::Add => (MaskOp::Subtract, own),
        MaskOp::Subtract => (MaskOp::Add, own),
        MaskOp::Intersect => (MaskOp::Add, !own),
    }
}

/// Turns `m` around: `Mask.invert` flips and every part after the first changes as the table above says. Saved masks
/// are never rewritten, only a call of this is.
///
/// Inverting twice selects the same area, but the table alone would bring an Intersect part back as a Subtract with
/// its Invert on (the same pixels under another name). So a call keeps what the parts were in `Mask.before_invert`;
/// the next call puts exactly that back when the parts are still what the table made of it (any edit since, or a
/// reorder, fails that check and the table applies again).
pub(super) fn invert(m: &mut Mask) {
    m.invert = !m.invert;
    let now: Vec<(MaskOp, bool)> = m.components.iter().skip(1).map(|c| (c.op, c.invert)).collect();
    let before = std::mem::take(&mut m.before_invert);
    let undoing = !before.is_empty() && before.iter().copied().map(turned).eq(now.iter().copied());
    let target: Vec<(MaskOp, bool)> = if undoing { before } else { now.iter().copied().map(turned).collect() };
    for (c, (op, own)) in m.components.iter_mut().skip(1).zip(target) {
        (c.op, c.invert) = (op, own);
    }
    if !undoing {
        m.before_invert = now;
    }
}

#[cfg(test)]
mod tests {
    use lightcraft_develop::{BrushStroke, DevelopSettings, MaskComponent, MaskShape};
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
                    assert_eq!(twice.components, m.components, "{what}: twice puts every part back as it was");
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

    /// Inverting twice restores every part exactly (mode and own Invert), whatever it was, on a mask that is not
    /// inverted and one that is, with a gradient base and a brush base.
    #[test]
    fn inverting_twice_restores_every_part_exactly() {
        let ops = [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect];
        let brush = MaskShape::Brush { strokes: vec![BrushStroke { points: vec![Point::new(0.3, 0.1)], size: 0.05, ..Default::default() }] };
        for base in [half(0.5, false), brush] {
            for mask_invert in [false, true] {
                for base_own in [false, true] {
                    for (o1, o2) in ops.iter().flat_map(|a| ops.iter().map(move |b| (*a, *b))) {
                        for (own1, own2) in [(false, false), (false, true), (true, false), (true, true)] {
                            let m = Mask {
                                invert: mask_invert,
                                components: vec![
                                    part(MaskOp::Add, base_own, base.clone()),
                                    part(o1, own1, half(0.75, false)),
                                    part(o2, own2, half(0.2, true)),
                                ],
                                ..Default::default()
                            };
                            let mut once = m.clone();
                            invert(&mut once);
                            assert_eq!(once.invert, !mask_invert);
                            let mut twice = once.clone();
                            invert(&mut twice);
                            let what = format!("invert={mask_invert} base own {base_own}: {o1:?}/{own1}, {o2:?}/{own2}");
                            assert_eq!(twice, m, "{what}: twice gives back the same mask");
                            // and a third time turns it again, as the first
                            let mut thrice = twice.clone();
                            invert(&mut thrice);
                            assert_eq!(thrice, once, "{what}: three times is one time");
                        }
                    }
                }
            }
        }
    }

    /// A part edited after Invert is not put back from the old note: the table applies again.
    #[test]
    fn an_edit_after_invert_makes_the_next_invert_use_the_table() {
        let mut m = Mask {
            components: vec![part(MaskOp::Add, false, half(0.5, false)), part(MaskOp::Intersect, false, half(0.25, false))],
            ..Default::default()
        };
        invert(&mut m);
        assert_eq!((m.components[1].op, m.components[1].invert), (MaskOp::Add, true));
        m.components[1].op = MaskOp::Subtract;
        invert(&mut m);
        assert_eq!((m.components[1].op, m.components[1].invert), (MaskOp::Add, true), "Subtract with Invert on becomes Add with it on");
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
