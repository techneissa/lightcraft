//! Fork feature (`fork/hair-edges.md`): a Subject grid whose edge BiRefNet-matting drew (`SegMask::model` ending
//! `+matting`, 1024²) draws the same outline in a preview and in an export.

use super::*;

fn frame(w: usize, h: usize) -> Frame {
    Frame::new(w, h, &lightcraft_develop::DevelopSettings::default(), true)
}

/// A dark disc (radius 0.3) on a light background at `n × n`, and a matted 1024² grid whose disc is one cell larger
/// (the photo's edge and the stored edge differ, as a real matte's soft strands differ from the photo's luminance edge).
fn scene(n: usize) -> (Rgb32f, Plane, SegMask) {
    scene_at(n, 1.0)
}

fn scene_at(n: usize, cells: f64) -> (Rgb32f, Plane, SegMask) {
    let dark = |x: f64, y: f64| (x - 0.5).hypot(y - 0.5) < 0.3;
    let lum = |x: usize, y: usize| if dark((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64) { 0.05f32 } else { 0.6 };
    let img = Rgb32f::from_fn(n, n, |x, y| [lum(x, y); 3]);
    let log_l = img.map(crate::local::log_lum);
    let side = 1024;
    let r = 0.3 + cells / side as f64;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = (((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64);
            if (x - 0.5).hypot(y - 0.5) < r { 12.0 } else { -12.0 }
        })
        .collect();
    (img, log_l, SegMask::from_logits(side, &logits).made_by("birefnet+matting"))
}

/// Where the edge crosses the row through the centre, as a fraction of the width (the integral of alpha past it).
fn edge_at(n: usize) -> f64 {
    edge_of(n, scene(n))
}

fn edge_of(n: usize, (img, log_l, seg): (Rgb32f, Plane, SegMask)) -> f64 {
    let shape = MaskShape::Subject { seg: Some(seg), detail: vec![], edge: 0.0 };
    let a = shape_alpha(&shape, &frame(n, n), n, n, &img, &log_l, 0.0, None);
    let covered: f32 = (n / 2..n).map(|x| a.get(x, n / 2)).sum();
    ((n / 2) as f64 + covered as f64) / n as f64
}

#[test]
fn a_matted_grid_draws_the_same_outline_in_a_preview_and_an_export() {
    // a preview finer than the grid's cells (no filter before the fix) and two exports coarser (filtered before)
    let preview = edge_at(512);
    for n in [2048, 4096] {
        let export = edge_at(n);
        assert!((export - preview).abs() * 512.0 < 0.1, "{n} px: the edge at {export:.5}, the preview's at {preview:.5}");
    }
}
