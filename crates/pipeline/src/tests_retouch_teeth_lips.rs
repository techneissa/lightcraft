//! Teeth and Lips in whole renders (`lightcraft_retouch::teeth_lips`): zoom never changes the retouch.

use std::sync::Arc;

use lightcraft_develop::retouch::{Face, Input, NormRect, Part, PartMask};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_raster::Rgb32f;

use crate::{PixelWindow, RenderRequest, SourceInfo, render};

/// A mouth on skin, `w × h`: lips shaded from dark at the left to light at the right (so their main colour depends on
/// all of them) around yellowish teeth, with Lips and Teeth masks as a people parse gives them.
fn mouth_photo(w: usize, h: usize) -> (Rgb32f, SourceInfo) {
    let lips = |nx: f64, ny: f64| (((nx - 0.5) / 0.2).powi(2) + ((ny - 0.5) / 0.08).powi(2)) < 1.0;
    let teeth = |nx: f64, ny: f64| (((nx - 0.5) / 0.12).powi(2) + ((ny - 0.5) / 0.035).powi(2)) < 1.0;
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        let shade = (0.5 + (nx - 0.3) * 2.0) as f32;
        if teeth(nx, ny) {
            [0.6, 0.55, 0.35]
        } else if lips(nx, ny) {
            [0.45 * shade, 0.12 * shade, 0.12 * shade]
        } else {
            [0.5, 0.36, 0.28]
        }
    });
    let bounds = NormRect::new(0.25, 0.1, 0.75, 0.9);
    let mask = |f: &dyn Fn(f64, f64) -> bool| {
        let n = 128;
        let plane = lightcraft_raster::Plane::from_fn(n, n, |x, y| {
            let (nx, ny) = (bounds.x0 + (x as f64 + 0.5) / n as f64 * bounds.width(), bounds.y0 + (y as f64 + 0.5) / n as f64 * bounds.height());
            if f(nx, ny) { 1.0 } else { 0.0 }
        });
        PartMask { plane, area: bounds }
    };
    let face = Face { masks: vec![(Part::Lips, mask(&lips)), (Part::Teeth, mask(&teeth))], ..Face::boxed(bounds) };
    (src, SourceInfo { retouch: Some(Arc::new(Input::new(vec![face]))), ..SourceInfo::default() })
}

/// Zoom never changes the retouch: a zoomed render of a window holding the left part of the lips and teeth gives
/// the pixels the whole frame's render has there (the lips' main colour and the teeth's level come from all of them).
#[test]
fn a_window_on_part_of_the_mouth_gives_the_whole_frames_retouch() {
    let (w, h) = (480, 360);
    let (src, info) = mouth_photo(w, h);
    let mut s = DevelopSettings::default();
    s.detail.nr_color = 0.0;
    let mut on = s.clone();
    for (id, v) in [("teethWhitening", 100.0), ("lipColorAmount", 100.0), ("lipColorR", 110.0), ("lipColorG", 30.0), ("lipColorB", 120.0)] {
        controls::set(&mut on, &format!("retouch.teethLips.{id}"), v);
    }
    let req = RenderRequest::fit(w, h);
    let win = PixelWindow { x: 140, y: 150, w: 70, h: 60 };
    let full = render(&src, &info, &on, &req).image;
    let plain = render(&src, &SourceInfo::default(), &s, &req).image;
    let zoomed = render(&src, &info, &on, &RenderRequest { window: Some(win), ..req }).image;
    assert_eq!((zoomed.width, zoomed.height), (win.w, win.h));
    let (mut worst, mut changed) = (0u8, 0);
    for y in 0..win.h {
        for x in 0..win.w {
            let (a, b, c) = (full.get(win.x + x, win.y + y), zoomed.get(x, y), plain.get(win.x + x, win.y + y));
            worst = worst.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
            changed += usize::from(a != c);
        }
    }
    assert!(changed > win.w * win.h / 4, "the retouch reaches the window ({changed} changed)");
    assert!(worst <= 1, "the zoomed window differs from the whole frame by {worst}");
}
