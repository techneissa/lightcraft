use std::cell::Cell;
use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_geom::Point;
use lightcraft_pipeline::{RenderRequest, SourceInfo};

thread_local! {
    /// The band share of renders on this thread (`None`: the app's).
    pub(super) static SHARE_HERE: Cell<Option<usize>> = const { Cell::new(None) };
    /// Most bytes of alphas a render on this thread held at once.
    pub(super) static ALPHAS: Cell<usize> = const { Cell::new(0) };
    /// Bands finished on this thread.
    pub(super) static BANDS: Cell<usize> = const { Cell::new(0) };
}

/// A render (keeping nothing) of `s` with a band share of `share` bytes: the image, the bands it was finished in, and
/// the most bytes of alphas it held at once.
fn render(src: &Arc<lightcraft_raster::Rgb32f>, s: &DevelopSettings, share: usize) -> (lightcraft_raster::Rgba8, usize, usize) {
    SHARE_HERE.with(|c| c.set(Some(share)));
    ALPHAS.with(|c| c.set(0));
    BANDS.with(|c| c.set(0));
    let req = RenderRequest::fit(src.width, src.height);
    let r = crate::render(src, &SourceInfo { raw: true, ..Default::default() }, s, &req, None).expect("gpu render");
    SHARE_HERE.with(|c| c.set(None));
    (r.image, BANDS.with(Cell::get), ALPHAS.with(Cell::get))
}

fn mask(id: u32, shapes: Vec<MaskShape>, adjust: LocalAdjustments) -> Mask {
    let op = |k: usize| if k == 0 { MaskOp::Add } else { MaskOp::Subtract };
    let components = shapes.into_iter().enumerate().map(|(k, shape)| MaskComponent { name: None, op: op(k), invert: false, shape }).collect();
    Mask { id, components, adjust, ..Default::default() }
}

fn stroke(y: f64, size: f64, erase: bool, auto_mask: bool) -> BrushStroke {
    let points: Vec<Point> = (0..12).map(|k| Point::new(0.1 + 0.07 * k as f64, y + 0.04 * (k as f64 * 0.7).sin())).collect();
    BrushStroke { points: points.into(), size, erase, auto_mask, ..Default::default() }
}

fn lift(exposure: f64) -> LocalAdjustments {
    LocalAdjustments { exposure, saturation: 20.0, ..Default::default() }
}

/// A frame in many bands gives the whole render's pixels, bit for bit, with every kind of mask (issue #406): brush strokes
/// with an erase, Auto Mask, both gradients, both ranges, a stored AI selection, Sky (no kernel), Refine Edges evaluated
/// per band and over the whole frame, Decontaminate, and local Moiré (the chromaticity planes).
#[test]
fn banded_masks_match_the_whole_render() {
    if !crate::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let (w, h) = (240, 420);
    let src = Arc::new(lightcraft_scenes::demo_library()[1].render(w, h));
    let side = 16;
    let ramp: Vec<f32> = (0..side * side).map(|i| ((i % side) as f32 - 7.5) * 0.8 + ((i / side) as f32 - 7.5) * 0.3).collect();
    let seg = SegMask::from_logits(side, &ramp);
    let radial = |cx: f64, cy: f64| MaskShape::Radial { center: Point::new(cx, cy), rx: 0.3, ry: 0.2, angle: 15.0, feather: 60.0, invert: false };
    let colour = MaskShape::ColorRange { samples: vec![[0.6, 0.05, 0.08], [0.45, -0.04, -0.06]], refine: 50.0 };
    let masks = vec![
        mask(
            1,
            vec![MaskShape::Brush {
                strokes: vec![stroke(0.2, 0.06, false, false), stroke(0.5, 0.08, false, false), stroke(0.3, 0.02, true, false)],
            }],
            lift(0.7),
        ),
        mask(2, vec![MaskShape::Brush { strokes: vec![stroke(0.7, 0.06, false, true)] }], lift(-0.5)),
        mask(3, vec![MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.8) }, radial(0.6, 0.6)], lift(0.4)),
        mask(4, vec![MaskShape::LuminanceRange { lo: 0.3, hi: 0.7, lo_feather: 0.1, hi_feather: 0.1 }], lift(-0.3)),
        mask(5, vec![colour.clone()], LocalAdjustments { moire: 60.0, ..lift(0.5) }),
        Mask { refine: 5.0, ..mask(6, vec![radial(0.4, 0.3)], lift(0.6)) },
        Mask { refine: 80.0, ..mask(7, vec![colour, radial(0.5, 0.8)], lift(-0.6)) },
        Mask { decontaminate: 70.0, ..mask(8, vec![MaskShape::Subject { seg: Some(seg), detail: Vec::new(), edge: 0.0 }], lift(0.8)) },
        Mask { decontaminate: 50.0, invert: true, ..mask(9, vec![radial(0.3, 0.7)], lift(-0.4)) },
        mask(10, vec![MaskShape::Sky { seg: None, detail: Vec::new(), edge: 0.0 }], lift(0.3)),
    ];
    let s = DevelopSettings { masks, ..Default::default() };
    let (whole, whole_bands, _) = render(&src, &s, usize::MAX);
    let (banded, bands, _) = render(&src, &s, 0);
    assert_eq!(whole_bands, 0, "a share that holds the frame evaluates the masks whole");
    assert!(bands > 4, "the frame is in bands ({bands})");
    let differ = whole.data.iter().zip(&banded.data).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{differ} of {} pixels differ from the whole render", w * h);
}

/// A render that keeps nothing holds one band's alphas (issue #406): with 12 masks it holds about as much as with one,
/// within the band share, and far less than the whole frame's 12 planes.
#[test]
fn twelve_masks_hold_one_bands_alphas() {
    if !crate::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let (w, h) = (240, 960);
    let src = Arc::new(lightcraft_scenes::demo_library()[0].render(w, h));
    let shapes = |k: usize| match k % 3 {
        0 => MaskShape::Linear { start: Point::new(0.0, 0.1 * k as f64), end: Point::new(0.0, 0.5) },
        1 => MaskShape::Radial { center: Point::new(0.5, 0.08 * k as f64), rx: 0.2, ry: 0.1, angle: 0.0, feather: 50.0, invert: false },
        _ => MaskShape::Brush { strokes: vec![stroke(0.08 * k as f64, 0.05, false, false)] },
    };
    let settings =
        |count: usize| DevelopSettings { masks: (0..count).map(|k| mask(k as u32 + 1, vec![shapes(k)], lift(0.3))).collect(), ..Default::default() };
    let share = 1 << 20;
    let (_, bands_one, one) = render(&src, &settings(1), share);
    let (_, bands, twelve) = render(&src, &settings(12), share);
    let (_, _, whole) = render(&src, &settings(12), usize::MAX);
    assert!(bands_one > 1 && bands > 1, "both in bands ({bands_one}, {bands})");
    assert!(twelve <= one + share, "12 masks held {twelve} bytes of alphas, one {one}");
    assert!(twelve * 8 < whole, "12 masks in bands held {twelve} bytes of alphas, whole {whole}");
}
