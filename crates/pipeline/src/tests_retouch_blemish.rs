//! Blemishes in whole renders (`lightcraft_retouch::blemish`): zoom never changes the repair.

use std::sync::Arc;

use lightcraft_develop::retouch::blemish::with_spots;
use lightcraft_develop::retouch::{Face, Input, NormRect, Part, PartMask};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_raster::{Plane, Rgb32f};

use crate::{PixelWindow, RenderRequest, SourceInfo, render};

/// Red pimples, in fractions of the photo.
const PIMPLES: [(f64, f64); 3] = [(0.4, 0.4), (0.6, 0.45), (0.5, 0.65)];

/// A face of skin with pores and red pimples, `w × h`, its Facial Skin mask an ellipse filling the face's box, and
/// its spots found as the engine finds them (once, on the photo).
fn skin_photo(w: usize, h: usize) -> (Rgb32f, SourceInfo) {
    let bounds = NormRect::new(0.25, 0.1, 0.75, 0.9);
    let size = (bounds.width() * w as f64).max(bounds.height() * h as f64);
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let pore = 1.0 + 0.05 * (((x * 7919 + y * 104_729) % 97) as f32 / 97.0 - 0.5);
        let mut p = [0.42 * pore, 0.28 * pore, 0.2 * pore];
        for (px, py) in PIMPLES {
            let d = ((x as f64 + 0.5 - px * w as f64).powi(2) + (y as f64 + 0.5 - py * h as f64).powi(2)).sqrt() / size;
            let g = (-(d / 0.008).powi(2)).exp() as f32;
            p = [p[0] * (1.0 - 0.06 * g), p[1] * (1.0 - 0.45 * g), p[2] * (1.0 - 0.35 * g)];
        }
        p
    });
    let plane = Plane::from_fn(128, 128, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / 64.0 - 1.0, (y as f32 + 0.5) / 64.0 - 1.0);
        ((1.0 - (u * u + v * v).sqrt()) * 30.0).clamp(0.0, 1.0)
    });
    let face = Face { masks: vec![(Part::FacialSkin, PartMask { plane, area: bounds })], ..Face::boxed(bounds) };
    let faces = with_spots(&src, vec![face]);
    (src, SourceInfo { retouch: Some(Arc::new(Input::new(faces))), ..SourceInfo::default() })
}

/// Zoom never changes the repair: a zoomed render of a window cutting through a pimple (its ring of skin samples and
/// its donor patch outside the window) gives the pixels the whole frame's render has there.
#[test]
fn a_window_on_part_of_a_spot_gives_the_whole_frames_repair() {
    let (w, h) = (640, 480);
    let (src, info) = skin_photo(w, h);
    let spots = info.retouch.as_ref().and_then(|i| i.faces.first()).and_then(|f| f.spots.as_ref()).map_or(0, |s| s.list.len());
    assert!(spots >= PIMPLES.len(), "the pimples are found ({spots} spots)");
    let mut s = DevelopSettings::default();
    s.detail.nr_color = 0.0;
    let mut on = s.clone();
    controls::set(&mut on, "retouch.blemish.amount", 100.0);
    let req = RenderRequest::fit(w, h);
    // the right half of the first pimple, at (256, 192)
    let win = PixelWindow { x: 256, y: 186, w: 10, h: 12 };
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
    assert!(changed > win.w * win.h / 4, "the repair reaches the window ({changed} changed)");
    assert!(worst <= 1, "the zoomed window differs from the whole frame by {worst}");
}
