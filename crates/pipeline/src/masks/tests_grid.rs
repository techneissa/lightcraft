//! Linear and radial masks through a lens or perspective warp (issue #212): the alphas from the position grid equal
//! the exact per-pixel ones within a quarter of an 8-bit level, over the whole frame and in a zoom window.

use super::grid::{self, ALPHA_TOL};
use super::*;
use lightcraft_develop::{DevelopSettings, EmbeddedLens, EmbeddedWarp};
use lightcraft_geom::Point;

/// A 6000 × 4000 photo with a strong embedded lens warp (barrel, tangential terms, off-centre), manual distortion
/// and a perspective fix: every term the warp has.
fn warped_frame() -> Frame {
    let mut s = DevelopSettings::default();
    s.optics.lens_profile = true;
    s.optics.distortion = -30.0;
    s.geometry.vertical = 25.0;
    s.geometry.rotate = 4.0;
    let lens = EmbeddedLens {
        warp: Some(EmbeddedWarp { planes: [[1.0, -0.08, 0.02, -0.004, 0.002, -0.001]; 3], center: Point::new(0.47, 0.53), radius: 0.62 }),
        ..Default::default()
    };
    let mut f = Frame::with_lens(6000, 4000, &s, true, Some(&lens));
    f.masks_on_photo = true;
    assert!(f.mask_warp().is_some(), "the masks go through the warp");
    f
}

fn shapes() -> Vec<(&'static str, MaskShape)> {
    vec![
        ("linear", MaskShape::Linear { start: Point::new(0.3, 0.1), end: Point::new(0.6, 0.7) }),
        // 6 px long at full size: most cells are mapped exactly
        ("sharp linear", MaskShape::Linear { start: Point::new(0.5, 0.5), end: Point::new(0.5005, 0.5005) }),
        ("radial", MaskShape::Radial { center: Point::new(0.45, 0.55), rx: 0.22, ry: 0.15, angle: 20.0, feather: 50.0, invert: false }),
        ("hard radial", MaskShape::Radial { center: Point::new(0.8, 0.2), rx: 0.1, ry: 0.06, angle: -35.0, feather: 0.0, invert: true }),
    ]
}

fn alpha(shape: &MaskShape, frame: &Frame, w: usize, h: usize) -> Plane {
    shape_alpha(shape, frame, w, h, &Rgb32f::new(0, 0), &Plane::new(0, 0), 0.0, None)
}

fn worst(a: &Plane, b: &Plane) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.iter().zip(&b.data).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max)
}

#[test]
fn warped_gradients_equal_the_exact_positions_over_the_whole_frame() {
    let frame = warped_frame();
    // a preview of the photo (a grid cell covers 2.5 photo pixels each way) and the photo's own size, cropped by
    // the lens correction
    for (w, h) in [(2400, 1600), frame.fit(6000, 6000)] {
        for (name, shape) in shapes() {
            let fast = alpha(&shape, &frame, w, h);
            let exact = grid::exactly(|| alpha(&shape, &frame, w, h));
            let d = worst(&fast, &exact);
            assert!(d as f64 <= ALPHA_TOL + 1e-6, "{name} at {w}×{h}: {d} from the exact alpha ({} levels)", d * 255.0);
            assert!(exact.data.iter().any(|v| *v > 0.5) && exact.data.iter().any(|v| *v < 0.5), "{name}: an edge in the frame");
        }
        let g = grid::pos_grid(&frame, w, h).expect("a grid");
        // every cell of a soft radial, all but the edge of a hard one
        let soft = grid::Ramp::radial(frame.norm_to_long(Point::new(0.45, 0.55)), 0.22, 0.15, 20.0, 0.5);
        let (fast, all) = grid::interpolated_cells(&g, &soft);
        assert_eq!(fast, all, "{w}×{h}: a soft radial interpolates {fast} of {all} cells");
        let hard = grid::Ramp::radial(frame.norm_to_long(Point::new(0.8, 0.2)), 0.1, 0.06, -35.0, 0.0);
        let (fast, all) = grid::interpolated_cells(&g, &hard);
        assert!(fast * 100 >= all * 95, "{w}×{h}: a hard radial interpolates {fast} of {all} cells");
    }
}

#[test]
fn a_zoom_window_has_the_pixels_of_the_whole_frame() {
    let frame = warped_frame();
    let (fw, fh) = (3000, 2000);
    // (not on the grid's lines, and across the hard radial's edge)
    let win = crate::PixelWindow { x: 2213, y: 157, w: 517, h: 389 };
    let wframe = frame.window(fw, fh, win);
    for (name, shape) in shapes() {
        let whole = alpha(&shape, &frame, fw, fh);
        let part = alpha(&shape, &wframe, win.w, win.h);
        let exact = grid::exactly(|| alpha(&shape, &wframe, win.w, win.h));
        let cut = Plane::from_fn(win.w, win.h, |x, y| whole.data[(win.y + y) * fw + win.x + x]);
        let (d, e) = (worst(&part, &cut), worst(&part, &exact));
        assert!(d <= 1e-5, "{name}: the window is {d} from the whole frame's pixels");
        assert!(e as f64 <= ALPHA_TOL + 1e-6, "{name}: the window is {e} from the exact alpha");
    }
}
