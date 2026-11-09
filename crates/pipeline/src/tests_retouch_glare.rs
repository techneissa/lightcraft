//! Glasses Glare's kept repair (`fork/glare-repair.md`) through the pipeline's retouch step: it lands on the lenses
//! under a zoomed window and under Face Reshape's warp, and every pixel it changes lies in the step's write boxes.

use std::sync::Arc;

use lightcraft_develop::retouch::input::Patch;
use lightcraft_develop::retouch::reshape::sketch_mesh;
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_geom::Point;
use lightcraft_raster::{Plane, Rgb32f};

use crate::{PixelWindow, RenderRequest, SourceInfo, retouch_step};

const W: usize = 300;
const H: usize = 200;

/// A repair over `area` that halves every channel in a disc about its middle (alpha 1 there, 0 past the disc).
fn halving(area: NormRect) -> Patch {
    let n = 32;
    let alpha = Plane::from_fn(n, n, |x, y| {
        let r = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
        (12.0 - r).clamp(0.0, 1.0)
    });
    Patch { area, gain: Rgb32f::filled(n, n, [0.5; 3]), alpha, stamp: 9 }
}

/// A flat photo with one small face (on the photo, as a people parse gives it) holding a repair over `area`.
fn photo(face_box: NormRect, area: NormRect, landmarks: bool) -> (Arc<Rgb32f>, SourceInfo) {
    let src = Rgb32f::filled(W, H, [0.5, 0.4, 0.3]);
    let mut face = Face::boxed(face_box);
    if landmarks {
        face.landmarks = sketch_mesh(face_box);
    }
    face.patch = Some(Arc::new(halving(area)));
    (Arc::new(src), SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() })
}

fn glare_on() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    controls::set(&mut s, "retouch.glare.amount", 100.0);
    s
}

/// What the retouch step does to a render of `req`: the image before and after, the step's write boxes, the output
/// pixel the photo point `at` lands on, and the render's width.
struct Run {
    before: Rgb32f,
    after: Rgb32f,
    boxes: Option<Vec<crate::spots_keep::Rect>>,
    at: Option<(f64, f64)>,
    w: usize,
    /// The full-frame pixel the render's top left is (a zoomed render is grown to hold what retouching reads).
    origin: (usize, usize),
}

fn run(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, at: (f64, f64)) -> Run {
    let p = crate::plan(src, info, s, req);
    let mut img = p.frame.sample(src, p.w, p.h);
    crate::lin_cpu_pre_retouch(&mut img, info, &p);
    let before = img.clone();
    retouch_step::apply(&mut img, info, &p);
    let to_out = p.frame.for_mask(true).mask_to_out(p.w, p.h);
    let origin = match (req.window, p.keep) {
        (Some(win), Some(k)) => (win.x - k.x, win.y - k.y),
        (Some(win), None) => (win.x, win.y),
        _ => (0, 0),
    };
    Run { before, after: img, boxes: retouch_step::write_boxes(info, &p), at: to_out(Point::new(at.0, at.1)).map(|q| (q.x, q.y)), w: p.w, origin }
}

impl Run {
    fn changed(&self) -> Vec<usize> {
        (0..self.before.data.len()).filter(|i| self.before.data[*i] != self.after.data[*i]).collect()
    }

    /// Every changed pixel lies in a write box.
    fn inside_boxes(&self) -> bool {
        let boxes = self.boxes.clone().unwrap_or_else(|| vec![[0, 0, self.w, self.before.height]]);
        self.changed().iter().all(|i| {
            let (x, y) = (i % self.w, i / self.w);
            boxes.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
        })
    }

    /// The factor the step gave the pixel at `at` of the render (the repair halves it where its alpha is 1).
    fn factor_at(&self, at: (f64, f64)) -> f32 {
        let i = (at.1.floor().max(0.0) as usize).min(self.before.height - 1) * self.w + (at.0.floor().max(0.0) as usize).min(self.w - 1);
        self.after.data[i][0] / self.before.data[i][0]
    }

    /// The factor at the full-frame pixel `(x, y)`.
    fn factor_at_frame(&self, x: usize, y: usize) -> f32 {
        self.factor_at(((x - self.origin.0) as f64, (y - self.origin.1) as f64))
    }
}

/// The repair sits over the lenses (the middle of its area) under a window zoomed in on them as in the whole frame,
/// and changes nothing outside the step's boxes even where it reaches past the face's box and its reach.
#[test]
fn a_kept_repair_lands_on_the_lenses_in_a_zoomed_window() {
    // a face box of 30 × 20 pixels; the repair's area is 120 × 60: far past the box and half a face of reach
    let (src, info) = photo(NormRect::new(0.45, 0.35, 0.55, 0.45), NormRect::new(0.3, 0.25, 0.7, 0.55), false);
    let s = glare_on();
    let middle = (0.5, 0.4);
    let whole = RenderRequest::fit(W, H);
    let full = run(&src, &info, &s, &whole, middle);
    assert!(!full.changed().is_empty(), "the repair shows");
    assert!(full.inside_boxes(), "the whole frame: a pixel changed outside the step's boxes {:?}", full.boxes);
    assert!((full.factor_at((150.0, 80.0)) - 0.5).abs() < 1e-3);
    // zoomed in on the middle of the lenses, and on the edge of the area
    for win in [PixelWindow { x: 130, y: 70, w: 40, h: 30 }, PixelWindow { x: 100, y: 50, w: 30, h: 30 }] {
        let zoomed = RenderRequest { window: Some(win), ..whole };
        let z = run(&src, &info, &s, &zoomed, middle);
        assert!(z.inside_boxes(), "window {win:?}: a pixel changed outside the step's boxes {:?}", z.boxes);
        // the same pixels as the whole frame's, inside the window
        for (x, y) in [(win.x + win.w / 2, win.y + win.h / 2), (win.x + 3, win.y + 3)] {
            let (a, b) = (z.factor_at_frame(x, y), full.factor_at_frame(x, y));
            assert!((a - b).abs() < 1e-3, "window {win:?}, pixel ({x}, {y}): {a} zoomed against {b} whole");
        }
    }
}

/// Under Face Reshape (a warp of the frame) the repair follows its lenses: the pixel the photo point of the lenses
/// lands on is the one it halves, and nothing changes outside the step's boxes.
#[test]
fn a_kept_repair_follows_its_lenses_under_face_reshape() {
    // (a small face box again: the repair's area reaches past it)
    let face_box = NormRect::new(0.44, 0.34, 0.56, 0.46);
    let (src, info) = photo(face_box, NormRect::new(0.4, 0.3, 0.6, 0.5), true);
    let mut s = glare_on();
    controls::set(&mut s, "retouch.reshape.slimFace", 80.0);
    controls::set(&mut s, "retouch.reshape.chin", 60.0);
    let req = RenderRequest::fit(W, H);
    assert!(crate::plan(&src, &info, &s, &req).frame.warp.is_some(), "the reshape warps the frame");
    let middle = (0.5, 0.4);
    let r = run(&src, &info, &s, &req, middle);
    let at = r.at.expect("the lenses' middle maps into the frame");
    assert!(!r.changed().is_empty());
    assert!(r.inside_boxes(), "a pixel changed outside the step's boxes {:?}", r.boxes);
    assert!((r.factor_at(at) - 0.5).abs() < 2e-2, "the pixel the lenses landed on has factor {}", r.factor_at(at));
    // and the unchanged corner of the frame stays as it was
    assert_eq!(r.before.data[0], r.after.data[0]);
}
