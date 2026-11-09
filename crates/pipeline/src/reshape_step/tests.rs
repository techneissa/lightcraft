use std::sync::Arc;

use lightcraft_develop::retouch::reshape::{MESH, sketch_mesh};
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, controls};
use lightcraft_raster::{Plane, Rgb32f};

use super::*;
use crate::{PixelWindow, RenderRequest, render};

const FACE: NormRect = NormRect::new(0.35, 0.2, 0.65, 0.7);

/// One face with the sketched mesh, on the photo (as the people parse gives it).
fn info() -> SourceInfo {
    let face = Face { landmarks: sketch_mesh(FACE), ..Face::boxed(FACE) };
    SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() }
}

fn reshaped(id: &str, v: f64) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    controls::set(&mut s, id, v);
    s
}

/// A photo with fine structure everywhere: thin dark lines on a soft gradient.
fn lines(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let line = ((u * 40.0).fract() < 0.08) || ((v * 30.0).fract() < 0.08);
        let g = 0.2 + 0.4 * u;
        if line { [0.03, 0.03, 0.05] } else { [g, 0.3, 0.5 - 0.3 * v] }
    })
}

/// Off is exact: at 0 (with faces, and another Portrait setting on), with the Portrait tab's eye off, or without a
/// face, the frame has no reshape and the render is the render without it, byte for byte.
#[test]
fn off_is_the_frame_and_the_render_without_it() {
    let src = lines(300, 200);
    let mut zero = reshaped("retouch.skin.smoothness", 40.0);
    controls::set(&mut zero, "retouch.reshape.slimFace", 0.0);
    let mut eye_off = reshaped("retouch.reshape.slimFace", 80.0);
    eye_off.set_section_enabled("portrait", false);
    for (s, info) in [(&zero, info()), (&eye_off, info()), (&reshaped("retouch.reshape.slimFace", 80.0), SourceInfo::default())] {
        assert!(field(&info, s, &crate::frame_for(&src, &info, s, true)).is_none());
        assert!(crate::frame_for(&src, &info, s, true).warp.is_none());
    }
    let req = RenderRequest::fit(300, 200);
    let plain = render(&src, &SourceInfo::default(), &DevelopSettings::default(), &req).image;
    assert_eq!(render(&src, &info(), &eye_off, &req).image, plain);
}

/// The reshape moves the face and nothing far from it: every pixel outside the warp's area keeps its bytes, so a
/// straight line there stays straight; the face itself changes. The GPU leaves its sampling to the CPU.
#[test]
fn only_the_face_moves() {
    let (w, h) = (300, 200);
    let src = lines(w, h);
    let mut s = reshaped("retouch.reshape.slimFace", 100.0);
    controls::set(&mut s, "retouch.reshape.chin", 100.0);
    controls::set(&mut s, "retouch.reshape.eyeSize", 100.0);
    let req = RenderRequest::fit(w, h);
    let p = crate::plan(&src, &info(), &s, &req);
    let warp = p.frame.warp.as_ref().expect("a warp");
    assert!(warp.cpu_only());
    let areas = warp.reshape.as_ref().expect("a reshape").areas();
    // the same render with every Portrait setting at 0 but the faces found: no reshape
    let plain = render(&src, &info(), &DevelopSettings::default(), &req).image;
    let done = render(&src, &info(), &s, &req).image;
    let (mut far, mut near_changed) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            let i = y * w + x;
            let (a, b) = (plain.data.get(i), done.data.get(i));
            let near = areas.iter().any(|r| nx > r[0] - 0.01 && nx < r[2] + 0.01 && ny > r[1] - 0.01 && ny < r[3] + 0.01);
            if near {
                near_changed += usize::from(a != b);
            } else {
                far += 1;
                assert_eq!(a, b, "pixel {x},{y} away from the face changed");
            }
        }
    }
    assert!(far > w * h / 3, "{far}");
    assert!(near_changed > 500, "the face moved: {near_changed}");
}

/// A red disc on the photo at the cheek's edge, and a local mask drawn on the photo over the same disc: under Slim
/// Face both move inward together, so the mask still covers the disc (a mask that did not follow would miss it).
#[test]
fn masks_on_the_photo_follow_the_reshape() {
    let (w, h) = (400, 300);
    let cheek = sketch_mesh(FACE)[MESH + 93];
    let r = 0.03; // of the long side
    let long = w.max(h) as f64;
    let in_disc = |nx: f64, ny: f64| ((nx - cheek.0) * w as f64).hypot((ny - cheek.1) * h as f64) < r * long;
    let src =
        Rgb32f::from_fn(w, h, |x, y| if in_disc((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64) { [0.6, 0.05, 0.05] } else { [0.2; 3] });
    let shape = MaskShape::Radial { center: lightcraft_geom::Point::new(cheek.0, cheek.1), rx: r, ry: r, angle: 0.0, feather: 0.0, invert: false };
    let mask = Mask { on_photo: true, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], ..Mask::default() };
    let s = reshaped("retouch.reshape.slimFace", 100.0);
    let p = crate::plan(&src, &info(), &s, &RenderRequest::fit(w, h));
    let still = crate::geometry::Frame::new(w, h, &s, true);
    let sampled = p.frame.sample(&src, w, h);
    let misses = |frame: &crate::geometry::Frame| {
        let alpha = crate::masks::evaluate_one(&mask, frame, w, h, &sampled, &Plane::new(w, h), 0.0, None);
        sampled.data.iter().zip(&alpha.data).filter(|(c, a)| (c[0] > 0.4) != (**a > 0.5)).count()
    };
    let red = sampled.data.iter().filter(|c| c[0] > 0.4).count();
    let moved = still.sample(&src, w, h).data.iter().zip(&sampled.data).filter(|(a, b)| (a[0] > 0.4) != (b[0] > 0.4)).count();
    assert!(red > 300 && moved > 40, "the disc is there and moved: {red} red, {moved} changed");
    let (follow, stale) = (misses(&p.frame), misses(&still));
    assert!(follow * 5 <= stale && follow <= red / 20, "the mask follows the disc: {follow} misses, {stale} without the reshape");
}

/// The retouch step's faces follow the reshape: a landmark maps to where its part of the face went (Chin + moves
/// the chin down), the way the step maps them for every Portrait feature.
#[test]
fn faces_follow_the_reshape() {
    let (w, h) = (300, 200);
    let src = lines(w, h);
    let chin = sketch_mesh(FACE)[MESH + 152];
    let at = |s: &DevelopSettings| {
        let p = crate::plan(&src, &info(), s, &RenderRequest::fit(w, h));
        let to_out = p.frame.for_mask(true).mask_to_out(w, h);
        to_out(lightcraft_geom::Point::new(chin.0, chin.1)).expect("on the picture")
    };
    let (before, longer) = (at(&reshaped("retouch.skin.smoothness", 10.0)), at(&reshaped("retouch.reshape.chin", 100.0)));
    let face_h = FACE.height() * h as f64;
    assert!(longer.y - before.y > 0.03 * face_h, "{before:?} → {longer:?}");
    assert!((longer.x - before.x).abs() < 0.5);
}

/// A face's own Face Reshape moves that face only.
#[test]
fn a_face_with_its_own_settings_moves_alone() {
    let (left, right) = (NormRect::new(0.05, 0.2, 0.35, 0.7), NormRect::new(0.6, 0.2, 0.9, 0.7));
    let faces: Vec<Face> = [left, right].iter().map(|b| Face { landmarks: sketch_mesh(*b), ..Face::boxed(*b) }).collect();
    let mut s = DevelopSettings::default();
    if let Some(own) = s.retouch.override_mut(&faces, 1) {
        own.reshape.slim_face = 100.0;
    }
    let info = SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(faces))), ..SourceInfo::default() };
    let src = lines(300, 200);
    let f = field(&info, &s, &crate::frame_for(&src, &info, &s, true)).expect("the right face moves");
    let areas = f.areas();
    assert_eq!(areas.len(), 1);
    assert!(areas.iter().all(|a| a[0] > 0.45), "{areas:?}");
}

/// Preview equals export: a draft of a small copy of the photo and a render of the photo four times its size read
/// the reshaped face from the same places, and look the same once the big one is scaled down.
#[test]
fn a_draft_and_a_bigger_render_agree() {
    let s = reshaped("retouch.reshape.slimFace", 100.0);
    let mut s = s;
    controls::set(&mut s, "retouch.reshape.eyeSize", 80.0);
    let (small, big) = (lines(300, 200), lines(1200, 800));
    let ps = crate::plan(&small, &info(), &s, &RenderRequest::fit(300, 200));
    let pb = crate::plan(&big, &info(), &s, &RenderRequest::fit(1200, 800));
    let (ms, mb) = (ps.frame.out_to_mask(300, 200), pb.frame.out_to_mask(1200, 800));
    for j in 0..30 {
        for i in 0..30 {
            let (u, v) = (0.3 + 0.4 * i as f64 / 30.0, 0.15 + 0.6 * j as f64 / 30.0);
            let a = ms.apply(lightcraft_geom::Point::new(u * 300.0, v * 200.0));
            let b = mb.apply(lightcraft_geom::Point::new(u * 1200.0, v * 800.0));
            assert!((a.x - b.x).abs() < 3e-4 && (a.y - b.y).abs() < 3e-4, "{u},{v}: {a:?} vs {b:?}");
        }
    }
}

/// The zoomed loupe renders a window of the frame: its pixels are the full render's pixels there.
#[test]
fn a_window_matches_the_whole_render() {
    let (w, h) = (300, 200);
    let src = lines(w, h);
    let s = reshaped("retouch.reshape.slimFace", 100.0);
    let req = RenderRequest::fit(w, h);
    let whole = render(&src, &info(), &s, &req).image;
    let win = PixelWindow { x: 90, y: 30, w: 120, h: 140 };
    let part = render(&src, &info(), &s, &RenderRequest { window: Some(win), ..req }).image;
    assert_eq!((part.width, part.height), (win.w, win.h));
    let mut worst = 0u8;
    for y in 0..win.h {
        for x in 0..win.w {
            let (a, b) = (part.data.get(y * win.w + x), whole.data.get((y + win.y) * w + x + win.x));
            if let (Some(a), Some(b)) = (a, b) {
                worst = worst.max(a.iter().zip(b).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0));
            }
        }
    }
    assert!(worst <= 1, "{worst}");
}
