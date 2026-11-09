use std::sync::Arc;

use lightcraft_develop::retouch::reshape::{MESH, sketch_mesh};
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, controls};
use lightcraft_geom::Point;
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
    assert!(warp.reshape_only());
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

/// Upright and Auto straighten read the photo without its Face Reshape: their analysis is cached by the optics and
/// orientation only, so a reshaped analysis image would go stale after a slider change (and the uncached Auto
/// straighten would disagree with it).
#[test]
fn upright_and_straighten_read_the_unreshaped_photo() {
    let src = lines(300, 200);
    let plain = crate::upright::analysis_plane(&src, &info(), &DevelopSettings::default());
    for (id, v) in [("retouch.reshape.slimFace", 100.0), ("retouch.reshape.chin", -100.0)] {
        let s = reshaped(id, v);
        assert!(field(&info(), &s, &crate::frame_for(&src, &info(), &s, false)).is_some(), "{id} reshapes the render");
        let got = crate::upright::analysis_plane(&src, &info(), &s);
        assert!(got.data == plain.data, "{id}: the analysis image moved with the face");
    }
}

/// Fine texture (pores a few pixels across) on a face of 360 px: the high-frequency energy inside the reshaped face
/// stays within a set ratio of the original's, with every slider at ±100.
#[test]
fn texture_is_kept_inside_a_warped_face() {
    let (w, h) = (1200, 800);
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let (x, y) = (x as f32, y as f32);
        let t = (x * 1.13 + y * 0.31).sin() + (x * 0.27 - y * 0.97).sin() + (x * 0.71 + y * 0.83).sin() + (x * -0.52 + y * 0.64).sin();
        let v = 0.3 + 0.04 * t;
        [v, v * 0.8, v * 0.7]
    });
    let energy = |img: &Rgb32f| {
        let (x0, x1) = ((FACE.x0 * w as f64) as usize + 20, (FACE.x1 * w as f64) as usize - 20);
        let (y0, y1) = ((FACE.y0 * h as f64) as usize + 20, (FACE.y1 * h as f64) as usize - 20);
        let g = |x: usize, y: usize| img.get(x, y)[1];
        let mut e = 0.0f64;
        for y in y0..y1 {
            for x in x0..x1 {
                let lap = 4.0 * g(x, y) - g(x - 1, y) - g(x + 1, y) - g(x, y - 1) - g(x, y + 1);
                e += f64::from(lap * lap);
            }
        }
        e
    };
    let req = RenderRequest::fit(w, h);
    let original = energy(&crate::plan(&src, &info(), &DevelopSettings::default(), &req).frame.sample(&src, w, h));
    for v in [-100.0, 100.0] {
        let mut s = DevelopSettings::default();
        for spec in lightcraft_develop::retouch::reshape::Reshape::CONTROLS {
            controls::set(&mut s, spec.spec.id, v);
        }
        let p = crate::plan(&src, &info(), &s, &req);
        assert!(p.frame.warp.as_ref().is_some_and(|w| w.reshape.is_some()));
        let ratio = energy(&p.frame.sample(&src, w, h)) / original;
        assert!((0.9..=1.1).contains(&ratio), "every slider at {v}: the texture's energy is {ratio:.3} of the original's");
    }
}

/// A face at the photo's edge, every slider at ±100: each point of the reshaped photo maps back to the output
/// (`Warp::from_source`, which the Portrait features use for faces, landmarks and masks), and back again to itself.
#[test]
fn the_warp_inverts_near_the_photos_edge() {
    let (w, h) = (300, 200);
    let src = lines(w, h);
    for b in [NormRect::new(-0.08, 0.1, 0.22, 0.6), NormRect::new(0.75, 0.45, 1.05, 0.98), NormRect::new(0.3, -0.1, 0.6, 0.35)] {
        let face = Face { landmarks: sketch_mesh(b), ..Face::boxed(b) };
        let info = SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face.clone()]))), ..SourceInfo::default() };
        for v in [-100.0, 100.0] {
            let mut s = DevelopSettings::default();
            for spec in lightcraft_develop::retouch::reshape::Reshape::CONTROLS {
                controls::set(&mut s, spec.spec.id, v);
            }
            let frame = crate::frame_for(&src, &info, &s, true);
            let warp = frame.warp.as_ref().expect("a warp");
            assert!(warp.reshape.is_some());
            let (ow, oh) = (w as f64, h as f64);
            let mut checked = 0;
            for j in 0..=40 {
                for i in 0..=40 {
                    let t = Point::new(ow * i as f64 / 40.0, oh * j as f64 / 40.0);
                    let s = warp.to_source(t, 1);
                    let back = warp.from_source(s).unwrap_or_else(|| panic!("{b:?} at {v}: {s:?} (from {t:?}) does not map back"));
                    let again = warp.to_source(back, 1);
                    assert!((again.x - s.x).abs() < 0.02 && (again.y - s.y).abs() < 0.02, "{b:?} at {v}: {s:?} → {back:?} → {again:?}");
                    checked += 1;
                }
            }
            assert!(checked > 1000);
            // every landmark of the face finds its place in the output
            let to_out = frame.for_mask(true).mask_to_out(w, h);
            for (k, (x, y)) in face.landmarks.iter().enumerate() {
                let on = (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y);
                assert!(!on || to_out(Point::new(*x, *y)).is_some(), "{b:?} at {v}: landmark {k} lost");
            }
        }
    }
}

/// Face Reshape alone is a geometry change only: the per-pixel retouch step stays off (its key is 0), so the GPU keeps
/// the scene-linear stage; with another Portrait setting on, the step runs as before.
#[test]
fn face_reshape_alone_leaves_the_retouch_step_off() {
    let src = lines(300, 200);
    let s = reshaped("retouch.reshape.slimFace", 60.0);
    assert!(lightcraft_develop::retouch::active(&s), "the faces are still searched for");
    assert!(!crate::retouch_step::active(&s) && !crate::lin_needs_cpu(&s, &info()));
    assert_eq!(crate::retouch_step::key(&s, &info()), 0);
    assert!(field(&info(), &s, &crate::frame_for(&src, &info(), &s, true)).is_some(), "and the face is reshaped");
    let mut both = s.clone();
    controls::set(&mut both, "retouch.skin.smoothness", 30.0);
    assert!(crate::retouch_step::active(&both) && crate::lin_needs_cpu(&both, &info()));
    // a face's own settings count too
    let mut own = DevelopSettings::default();
    let face = Face { landmarks: sketch_mesh(FACE), ..Face::boxed(FACE) };
    if let Some(f) = own.retouch.override_mut(std::slice::from_ref(&face), 0) {
        f.reshape.chin = 50.0;
        f.skin.smoothness = 20.0;
    }
    assert!(crate::retouch_step::active(&own));
}
