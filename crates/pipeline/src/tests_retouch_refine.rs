//! The Refine brush in whole renders (`lightcraft_retouch::refine`, issue #99): zoom never changes it, and a draft
//! and a bigger render agree under a crop and a lens warp, with faces before the warp and strokes after it.

use std::sync::Arc;

use lightcraft_develop::retouch::{Face, Input, NormRect, Part, PartMask, RefineMode, RefineStroke};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_raster::Rgb32f;

use crate::{PixelWindow, RenderRequest, SourceInfo, render};

fn stroke(mode: RefineMode, points: &[(f64, f64)], size: f64) -> RefineStroke {
    RefineStroke { mode, points: points.to_vec(), size, feather: 30.0, ..RefineStroke::default() }
}

/// Textured skin, `w × h`, and one face whose Facial Skin mask covers the left half of its box only.
fn half_skin(w: usize, h: usize, on_photo: bool) -> (Rgb32f, SourceInfo) {
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let t = 1.0 + 0.08 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0 + 0.1 * ((x as f32 / 3.1).sin() * (y as f32 / 2.9).sin());
        [0.5 * t, 0.36 * t, 0.28 * t]
    });
    let bounds = NormRect::new(0.2, 0.15, 0.8, 0.85);
    let plane = lightcraft_raster::Plane::from_fn(64, 64, |x, _| if x < 32 { 1.0 } else { 0.0 });
    let face = Face { masks: vec![(Part::FacialSkin, PartMask { plane, area: bounds })], ..Face::boxed(bounds) };
    let input = if on_photo { Input::new_on_photo(vec![face]) } else { Input::new(vec![face]) };
    (src, SourceInfo { retouch: Some(Arc::new(input)), ..SourceInfo::default() })
}

fn refined() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.detail.nr_color = 0.0;
    controls::set(&mut s, "retouch.skin.smoothness", 100.0);
    s.retouch.refine.push("skin", stroke(RefineMode::Reduce, &[(0.3, 0.4), (0.35, 0.6)], 0.06));
    s.retouch.refine.push("skin", stroke(RefineMode::Add, &[(0.62, 0.45), (0.66, 0.6)], 0.07));
    s
}

/// A zoomed window over the Reduce stroke's edge, and one over the Add stroke, give the whole frame's pixels.
#[test]
fn a_zoomed_window_gives_the_whole_frames_refine() {
    let (w, h) = (480, 360);
    let (src, info) = half_skin(w, h, false);
    let s = refined();
    let mut plain = s.clone();
    controls::set(&mut plain, "retouch.skin.smoothness", 0.0);
    let req = RenderRequest::fit(w, h);
    let full = render(&src, &info, &s, &req).image;
    let base = render(&src, &info, &plain, &req).image;
    for win in [PixelWindow { x: 150, y: 150, w: 40, h: 40 }, PixelWindow { x: 300, y: 160, w: 30, h: 40 }] {
        let zoomed = render(&src, &info, &s, &RenderRequest { window: Some(win), ..req }).image;
        let (mut worst, mut changed) = (0u8, 0);
        for y in 0..win.h {
            for x in 0..win.w {
                let (a, b, c) = (full.get(win.x + x, win.y + y), zoomed.get(x, y), base.get(win.x + x, win.y + y));
                worst = worst.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
                changed += usize::from(a != c);
            }
        }
        assert!(changed > win.w * win.h / 8, "{win:?}: the retouch reaches the window ({changed} changed)");
        assert!(worst <= 1, "{win:?}: the zoomed window differs from the whole frame by {worst}");
    }
}

/// Faces from a people parse (before the lens warp), strokes after it, a crop and a strong distortion: a draft and
/// a render four times its size smooth the same share of the picture, and the Add stroke smooths skin the mask
/// left out in both.
#[test]
fn a_draft_and_a_bigger_render_agree() {
    let mut shares = Vec::new();
    for (rw, rh) in [(150, 100), (600, 400)] {
        let (src, info) = half_skin(600, 400, true);
        let mut s = refined();
        s.optics.distortion = 60.0;
        s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.05, y0: 0.05, x1: 0.95, y1: 0.95 };
        let mut no_add = s.clone();
        no_add.retouch.refine.0.iter_mut().for_each(|l| l.strokes.retain(|st| st.mode != RefineMode::Add));
        let mut off = s.clone();
        controls::set(&mut off, "retouch.skin.smoothness", 0.0);
        let req = RenderRequest { apply_crop: true, ..RenderRequest::fit(rw, rh) };
        let plain = render(&src, &info, &off, &req).image;
        let done = render(&src, &info, &s, &req).image;
        let without = render(&src, &info, &no_add, &req).image;
        let n = plain.data.len() as f64;
        let changed = plain.data.iter().zip(&done.data).filter(|(a, b)| a != b).count() as f64 / n;
        let by_add = without.data.iter().zip(&done.data).filter(|(a, b)| a != b).count() as f64 / n;
        assert!(by_add > 0.01, "{rw}×{rh}: Add smooths past the mask ({by_add})");
        shares.push((changed, by_add));
    }
    let ((a0, b0), (a1, b1)) = (shares[0], shares[1]);
    assert!((a0 - a1).abs() < 0.03 && (b0 - b1).abs() < 0.02, "the smoothed shares: {shares:?}");
}
