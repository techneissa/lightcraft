use std::cell::{Cell, RefCell};
use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_geom::Point;
use lightcraft_pipeline::{RenderRequest, SourceInfo};

thread_local! {
    /// The band share of renders on this thread (`None`: the app's).
    pub(super) static SHARE_HERE: Cell<Option<usize>> = const { Cell::new(None) };
    /// Bands filled on this thread.
    static BANDS: Cell<usize> = const { Cell::new(0) };
    /// Masks a banded render on this thread evaluated whole.
    pub(super) static WHOLE: Cell<usize> = const { Cell::new(0) };
    /// When set, each band's alphas read back into planes of the whole frame (`w × h` values each), in mask order.
    static CAPTURE: RefCell<Option<Vec<Vec<f32>>>> = const { RefCell::new(None) };
}

/// A band filled ([`super::note_band`]): counted, and its alphas read back into [`CAPTURE`] when it is set.
pub(super) fn note_band(cx: &mut crate::render::Cx<'_>, buf: &crate::ctx::Buf, (y0, rows): (usize, usize), (w, np): (usize, usize)) {
    BANDS.with(|c| c.set(c.get() + 1));
    CAPTURE.with(|c| {
        for (k, plane) in c.borrow_mut().iter_mut().flatten().enumerate() {
            let band = cx.slice(buf, k * np, w * rows);
            let values: Vec<f32> = cx.read(&band, w * rows);
            plane[y0 * w..(y0 + rows) * w].copy_from_slice(&values);
        }
    });
}

/// What a render (keeping nothing) of `s` with a band share of `share` bytes made and held.
struct Run {
    image: lightcraft_raster::Rgba8,
    bands: usize,
    whole: usize,
    /// The most bytes of device buffers the render held at once.
    peak: u64,
}

fn render(src: &Arc<lightcraft_raster::Rgb32f>, s: &DevelopSettings, share: usize) -> Run {
    SHARE_HERE.with(|c| c.set(Some(share)));
    BANDS.with(|c| c.set(0));
    WHOLE.with(|c| c.set(0));
    let _ = crate::ctx::thread_bytes();
    let req = RenderRequest::fit(src.width, src.height);
    let r = crate::render(src, &SourceInfo { raw: true, ..Default::default() }, s, &req, None).expect("gpu render");
    let peak = crate::ctx::thread_bytes();
    SHARE_HERE.with(|c| c.set(None));
    Run { image: r.image, bands: BANDS.with(Cell::get), whole: WHOLE.with(Cell::get), peak }
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
    let whole = render(&src, &s, usize::MAX);
    // (bands of a few dozen rows: the colour ranges and the light Refine Edges reach a few rows and go per band)
    let banded = render(&src, &s, 1_200_000);
    assert_eq!(whole.bands, 0, "a share that holds the frame evaluates the masks whole");
    assert!(banded.bands > 4, "the frame is in bands ({})", banded.bands);
    assert!(
        banded.whole > 0 && banded.whole < s.masks.len(),
        "some masks go whole (Auto Mask, Sky, decontaminated, wide Refine Edges) and the rest per band: {} whole",
        banded.whole
    );
    let differ = whole.image.data.iter().zip(&banded.image.data).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{differ} of {} pixels differ from the whole render", w * h);
}

/// Masks that reach far (a colour range and Refine Edges, whose box filters run their sums over runs of rows that start on
/// the whole frame's) evaluated in bands of a few hundred rows, not whole, give the whole frame's alphas bit for bit.
#[test]
fn wide_masks_in_bands_match_the_whole_alphas() {
    if !crate::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let (w, h) = (160, 1000);
    let src = Arc::new(lightcraft_scenes::demo_library()[1].render(w, h));
    let colour = MaskShape::ColorRange { samples: vec![[0.6, 0.05, 0.08], [0.45, -0.04, -0.06]], refine: 50.0 };
    let radial = MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.4, ry: 0.3, angle: 15.0, feather: 60.0, invert: false };
    let masks = vec![
        Mask { refine: 30.0, ..mask(1, vec![colour.clone()], lift(0.5)) },
        Mask { refine: 30.0, ..mask(2, vec![radial], lift(-0.4)) },
        mask(3, vec![colour], lift(0.3)),
    ];
    let s = DevelopSettings { masks, ..Default::default() };
    let ask = lightcraft_pipeline::masks::PlanesAsk { want: lightcraft_pipeline::masks::MaskWant::All, known: None };
    let req = RenderRequest::fit(w, h);
    let info = SourceInfo { raw: true, ..Default::default() };
    let whole = crate::mask_planes(&src, &info, &s, &req, None, &ask).and_then(|p| p.planes).expect("mask planes");
    CAPTURE.with(|c| *c.borrow_mut() = Some(vec![vec![f32::NAN; w * h]; 3]));
    // (about 320-row bands, wider than the masks' reach)
    let banded = render(&src, &s, 9_300_000);
    let planes = CAPTURE.with(|c| c.borrow_mut().take()).unwrap_or_default();
    assert!(banded.bands > 1 && banded.whole == 0, "{} bands, {} masks whole", banded.bands, banded.whole);
    assert_eq!(whole.len(), planes.len());
    for ((id, a), b) in whole.iter().zip(&planes) {
        let differ = a.data.iter().zip(b).filter(|(x, y)| x.to_bits() != y.to_bits()).count();
        assert_eq!(differ, 0, "mask {id}: {differ} of {} alphas differ from the whole frame's", w * h);
    }
}

/// A render that keeps nothing holds one band's masks (issue #406): with 12 masks, every device buffer it holds at once
/// comes to not much above what it holds with one, within the band share, while the whole frame's 12 alphas would add
/// a plane a mask.
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
    let one = render(&src, &settings(1), share);
    let twelve = render(&src, &settings(12), share);
    let whole = render(&src, &settings(12), usize::MAX);
    assert!(one.bands > 1 && twelve.bands > 1, "both in bands ({}, {})", one.bands, twelve.bands);
    let plane = (w * h * 4) as u64;
    assert!(twelve.peak <= one.peak + share as u64, "12 masks held {} bytes at once, one {}", twelve.peak, one.peak);
    assert!(whole.peak > twelve.peak + 6 * plane, "12 masks held {} bytes at once in bands, {} whole", twelve.peak, whole.peak);
}
