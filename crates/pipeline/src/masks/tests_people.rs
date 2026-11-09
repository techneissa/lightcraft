//! Fork (`fork/select-people.md`): a Select People part's finer grid (a small face in a group) keeps its own edge:
//! its pixels are filtered with its own window, not with the coarse grid's.

use super::*;

/// A photo with a small dark disc (a small face), a coarse 64² people grid over the photo and a fine 256² one over the
/// disc, each one cell larger than the photo's disc.
fn scene(n: usize) -> (Rgb32f, Plane, SegMask, SegMask) {
    let (cx, cy, r) = (0.25, 0.25, 0.05);
    let dark = |x: f64, y: f64| (x - cx).hypot(y - cy) < r;
    let img = Rgb32f::from_fn(n, n, |x, y| [if dark((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64) { 0.05f32 } else { 0.6 }; 3]);
    let log_l = img.map(crate::local::log_lum);
    let disc = |side: usize, rect: [f64; 4]| -> Vec<f32> {
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        (0..side * side)
            .map(|i| {
                let x = rect[0] + ((i % side) as f64 + 0.5) / side as f64 * w;
                let y = rect[1] + ((i / side) as f64 + 0.5) / side as f64 * h;
                if (x - cx).hypot(y - cy) < r + w / side as f64 { 12.0 } else { -12.0 }
            })
            .collect()
    };
    let coarse = SegMask::from_logits(64, &disc(64, [0.0, 0.0, 1.0, 1.0])).made_by("people:segformer");
    let rect = [0.15, 0.15, 0.35, 0.35];
    let fine = SegMask::from_logits_in(256, &disc(256, rect), rect).made_by("people:segformer");
    (img, log_l, coarse, fine)
}

fn alpha(n: usize, seg: Option<SegMask>, detail: Vec<SegMask>, img: &Rgb32f, log_l: &Plane) -> Plane {
    let shape = MaskShape::People { person: 0, parts: vec!["hair".into()], face: None, seg, detail, edge: 0.0 };
    let frame = Frame::new(n, n, &lightcraft_develop::DevelopSettings::default(), true);
    shape_alpha(&shape, &frame, n, n, img, log_l, 0.0, None)
}

#[test]
fn a_finer_grid_is_filtered_with_its_own_window() {
    let n = 1024;
    let (img, log_l, coarse, fine) = scene(n);
    // the fine grid's cells are under a pixel here: inside it the mask is the fine grid as it is, with or without the
    // coarse grid around it (before: the coarse grid's 24 px window filtered it too)
    let both = alpha(n, Some(coarse), vec![fine.clone()], &img, &log_l);
    let alone = alpha(n, None, vec![fine], &img, &log_l);
    let inside = (n * 16 / 100..n * 34 / 100).flat_map(|y| (n * 16 / 100..n * 34 / 100).map(move |x| (x, y)));
    let worst = inside.map(|(x, y)| (both.get(x, y) - alone.get(x, y)).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "the fine grid's edge moved by up to {worst}");
}
