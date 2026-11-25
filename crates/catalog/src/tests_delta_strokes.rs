//! Issue #378: a brush stroke's log patch converts only that stroke to JSON, and is the same patch, byte for byte,
//! as the diff of the settings' whole JSON.

use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;

use crate::delta_strokes::{Skeletons, whole};

fn stroke(i: usize) -> BrushStroke {
    let y = 0.05 + (i % 90) as f64 * 0.01;
    BrushStroke { points: (0..40).map(|k| Point::new(0.1 + k as f64 * 0.013, y)).collect(), ..Default::default() }
}

fn brush(strokes: Vec<BrushStroke>) -> MaskComponent {
    MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes } }
}

fn strokes(d: &mut DevelopSettings, mask: usize) -> &mut Vec<BrushStroke> {
    while d.masks.len() <= mask {
        let id = d.masks.len() as u32 + 1;
        d.masks.push(Mask { id, name: format!("Brush {id}"), components: vec![brush(vec![])], ..Default::default() });
    }
    if !matches!(d.masks[mask].components[0].shape, MaskShape::Brush { .. }) {
        d.masks[mask].components[0] = brush(vec![]);
    }
    match &mut d.masks[mask].components[0].shape {
        MaskShape::Brush { strokes } => strokes,
        _ => panic!("a brush mask"),
    }
}

/// Each edit's patch from the settings before it, as the log writes it and as the whole JSON's diff gives it.
fn check(edits: &[Arc<DevelopSettings>]) {
    let mut sk = Skeletons::default();
    for w in edits.windows(2) {
        let fast = serde_json::to_string(&sk.patch(&w[0], &w[1])).unwrap();
        let slow = serde_json::to_string(&whole(&w[0], &w[1])).unwrap();
        assert_eq!(fast, slow);
    }
}

#[test]
fn brush_edit_patches_are_the_whole_json_diff() {
    let mut edits = vec![Arc::new(DevelopSettings::default())];
    let mut push = |f: &mut dyn FnMut(&mut DevelopSettings)| {
        let mut d = (**edits.last().unwrap()).clone();
        f(&mut d);
        edits.push(Arc::new(d));
    };
    for i in 0..12 {
        push(&mut |d| strokes(d, 0).push(stroke(i)));
    }
    push(&mut |d| strokes(d, 1).push(stroke(50)));
    push(&mut |d| strokes(d, 0).push(stroke(13)));
    // a stroke erased from the middle; one stroke read back as a copy (new points, same values)
    push(&mut |d| _ = strokes(d, 0).remove(4));
    push(&mut |d| strokes(d, 0)[2].points = strokes(d, 0)[2].points.to_vec().into());
    // a point's zero changes sign; a stroke gains pressures; a stroke's size changes
    push(&mut |d| {
        let mut p = strokes(d, 0)[3].points.to_vec();
        p[0] = Point::new(0.0, 0.5);
        strokes(d, 0)[3].points = p.into();
    });
    push(&mut |d| {
        let mut p = strokes(d, 0)[3].points.to_vec();
        p[0] = Point::new(-0.0, 0.5);
        strokes(d, 0)[3].points = p.into();
    });
    push(&mut |d| strokes(d, 0)[5].pressure = vec![0.5f32; 40].into());
    push(&mut |d| strokes(d, 0)[6].size = 0.07);
    // Lens Blur's brushes; a mask's brush replaced by a gradient; a mask removed
    push(&mut |d| d.lens_blur.focus_strokes.push(stroke(70)));
    push(&mut |d| d.lens_blur.blur_strokes = vec![stroke(71), stroke(72)]);
    push(&mut |d| d.masks[1].components[0].shape = MaskShape::Linear { start: Point::new(0.1, 0.1), end: Point::new(0.9, 0.9) });
    push(&mut |d| _ = d.masks.remove(1));
    push(&mut |d| strokes(d, 0).push(stroke(14)));
    // undo to an earlier state, then a stroke from there
    let back = edits[9].clone();
    edits.push(back);
    edits.push(Arc::new({
        let mut d = (*edits[9]).clone();
        strokes(&mut d, 0).push(stroke(15));
        d
    }));
    // a stroke holding the stand-in's own number, and a gradient holding it, are diffed whole and still right
    let mark = f64::from_bits(0x3E80_9D2A_57C4_E1B3);
    edits.push(Arc::new({
        let mut d = (**edits.last().unwrap()).clone();
        let mut s = stroke(16);
        (s.size, s.flow, s.density) = (mark, mark, mark);
        strokes(&mut d, 0).push(s);
        d
    }));
    edits.push(Arc::new({
        let mut d = (**edits.last().unwrap()).clone();
        d.masks.push(Mask {
            id: 9,
            components: vec![MaskComponent {
                shape: MaskShape::Linear { start: Point::new(mark, mark), end: Point::new(0.5, mark) },
                ..brush(vec![])
            }],
            ..Default::default()
        });
        d
    }));
    edits.push(Arc::new({
        let mut d = (**edits.last().unwrap()).clone();
        strokes(&mut d, 0).push(stroke(17));
        d
    }));
    check(&edits);
}

#[test]
fn a_new_stroke_converts_only_itself() {
    let mut sk = Skeletons::default();
    let mut d = Arc::new(DevelopSettings::default());
    for i in 0..60 {
        let mut n = (*d).clone();
        strokes(&mut n, 0).push(stroke(i));
        let n = Arc::new(n);
        let before = sk.strokes.converted;
        sk.patch(&d, &n).unwrap();
        assert_eq!(sk.strokes.converted - before, 1, "stroke {i} converted the strokes before it");
        d = n;
    }
}
