//! Fork feature (`fork/apple-vision-masks.md`, issue #115): a Subject grid whose edge Apple Vision drew
//! (`SegMask::model` ending `+vision`) draws the same outline in a preview and in an export, like a `+matting` one.

use super::*;

/// Where the edge crosses the centre row of an `n × n` render of a dark disc (r 0.3) whose stored grid is one cell
/// larger, as a fraction of the width.
fn edge_at(n: usize) -> f64 {
    let dark = |x: f64, y: f64| (x - 0.5).hypot(y - 0.5) < 0.3;
    let lum = |x: usize, y: usize| if dark((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64) { 0.05f32 } else { 0.6 };
    let img = Rgb32f::from_fn(n, n, |x, y| [lum(x, y); 3]);
    let log_l = img.map(crate::local::log_lum);
    let side = 1024;
    let r = 0.3 + 1.0 / side as f64;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = (((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64);
            if (x - 0.5).hypot(y - 0.5) < r { 12.0 } else { -12.0 }
        })
        .collect();
    let seg = SegMask::from_logits(side, &logits).made_by("birefnet+vision");
    let shape = MaskShape::Subject { seg: Some(seg), detail: vec![], edge: 0.0 };
    let frame = Frame::new(n, n, &lightcraft_develop::DevelopSettings::default(), true);
    let a = shape_alpha(&shape, &frame, n, n, &img, &log_l, 0.0, None);
    let covered: f32 = (n / 2..n).map(|x| a.get(x, n / 2)).sum();
    ((n / 2) as f64 + covered as f64) / n as f64
}

#[test]
fn a_grid_apple_vision_drew_draws_the_same_outline_in_a_preview_and_an_export() {
    let preview = edge_at(512);
    for n in [2048, 4096] {
        let export = edge_at(n);
        assert!((export - preview).abs() * 512.0 < 0.1, "{n} px: the edge at {export:.5}, the preview's at {preview:.5}");
    }
}
