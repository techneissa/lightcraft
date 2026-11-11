use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{FEATURES, Face, Input, NormRect, Part, PartMask};
use lightcraft_raster::{Plane, Rgb32f};

use crate::{Overlay, PixelWindow, RenderRequest, SourceInfo, render};

const W: usize = 320;
const H: usize = 240;
/// The face's box and its mouth (where its teeth are), on the photo.
const FACE: NormRect = NormRect { x0: 0.3, y0: 0.15, x1: 0.6, y1: 0.75 };
const TEETH: NormRect = NormRect { x0: 0.4, y0: 0.58, x1: 0.5, y1: 0.64 };

/// A grey photo with a skin-coloured face and yellowish teeth, the face with its Facial Skin and Teeth masks.
fn face_photo() -> (Rgb32f, SourceInfo) {
    let inside = |r: &NormRect, nx: f64, ny: f64| nx >= r.x0 && nx < r.x1 && ny >= r.y0 && ny < r.y1;
    let src = Rgb32f::from_fn(W, H, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / W as f64, (y as f64 + 0.5) / H as f64);
        if inside(&TEETH, nx, ny) {
            [0.55, 0.48, 0.28]
        } else if inside(&FACE, nx, ny) {
            [0.45, 0.3, 0.22]
        } else {
            [0.18, 0.18, 0.18]
        }
    });
    let mask = |area: NormRect| PartMask { plane: Plane::filled(8, 8, 1.0), area };
    let face = Face { masks: vec![(Part::FacialSkin, mask(FACE)), (Part::Teeth, mask(TEETH))], ..Face::boxed(FACE) };
    (src, SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() })
}

/// The overlay of control `id`.
fn overlay(id: &str, face: u8) -> Overlay {
    let feature = FEATURES.iter().position(|f| id.starts_with(&format!("retouch.{}.", f.key))).unwrap_or(255);
    let control = FEATURES.get(feature).and_then(|f| (f.specs)().iter().position(|s| s.id == id)).unwrap_or(255);
    Overlay::RetouchEffect { feature: feature as u8, control: control as u8, face }
}

/// The map of control `id` at `value` for every face, through a render's plan.
fn map(src: &Rgb32f, info: &SourceInfo, id: &str, value: f64) -> Plane {
    let mut s = DevelopSettings::default();
    s.retouch.global.set(id, value);
    let req = RenderRequest { overlay: overlay(id, Overlay::ALL_FACES), ..RenderRequest::fit(W, H) };
    let plan = crate::plan(src, info, &s, &req);
    let Overlay::RetouchEffect { feature, control, face } = req.overlay else { return Plane::filled(1, 1, 0.0) };
    super::effect(src, None, info, &plan, feature, control, face).unwrap_or_else(|| Plane::filled(1, 1, 0.0))
}

fn at(p: &Plane, nx: f64, ny: f64) -> f32 {
    p.data[(ny * p.height as f64) as usize * p.width + (nx * p.width as f64) as usize]
}

/// Issue #208: Teeth Whitening's map lights the teeth and nothing else, grows with the value, and is 0 everywhere at 0.
#[test]
fn the_map_shows_where_and_how_much_the_slider_changes() {
    let (src, info) = face_photo();
    let id = "retouch.teethLips.teethWhitening";
    let maps: Vec<Plane> = [0.0, 25.0, 50.0, 100.0].iter().map(|v| map(&src, &info, id, *v)).collect();
    assert_eq!((maps[0].width, maps[0].height), (W, H));
    assert!(maps[0].data.iter().all(|v| *v == 0.0), "at 0 nothing changes");
    let teeth = |m: &Plane| at(m, 0.45, 0.61);
    for m in &maps[1..] {
        assert_eq!(at(m, 0.35, 0.3), 0.0, "the skin, which Teeth Whitening leaves alone");
        assert_eq!(at(m, 0.1, 0.5), 0.0, "the background");
        assert!(teeth(m) > 0.05, "the teeth light up: {}", teeth(m));
    }
    assert!(teeth(&maps[1]) < teeth(&maps[2]) && teeth(&maps[2]) < teeth(&maps[3]), "brighter as the value goes up");
    let sum = |m: &Plane| m.data.iter().map(|v| f64::from(*v)).sum::<f64>();
    assert!(sum(&maps[1]) < sum(&maps[3]));
    // (the slider of another feature at the same value: its own change, not Teeth Whitening's)
    let mut s = DevelopSettings::default();
    s.retouch.global.set(id, 100.0);
    s.retouch.global.set("retouch.skinTone.faceLight", 60.0);
    let req = RenderRequest { overlay: overlay(id, Overlay::ALL_FACES), ..RenderRequest::fit(W, H) };
    let plan = crate::plan(&src, &info, &s, &req);
    let Overlay::RetouchEffect { feature, control, face } = req.overlay else { return };
    let m = super::effect(&src, None, &info, &plan, feature, control, face).unwrap_or_else(|| Plane::filled(1, 1, 0.0));
    assert!(at(&m, 0.45, 0.61) > 0.05 && at(&m, 0.1, 0.5) == 0.0, "Teeth Whitening's map with Face Light on too");
}

/// A face whose own settings differ: the picked face's map is its own change, and "every face" leaves it out.
#[test]
fn a_picked_faces_map_is_its_own_change() {
    let (src, info) = face_photo();
    let id = "retouch.teethLips.teethWhitening";
    let input = info.retouch.clone().unwrap_or_default();
    let mut s = DevelopSettings::default();
    s.retouch.override_mut(&input.faces, 0).map(|f| f.set(id, 80.0));
    let effect = |face: u8| {
        let req = RenderRequest { overlay: overlay(id, face), ..RenderRequest::fit(W, H) };
        let plan = crate::plan(&src, &info, &s, &req);
        let Overlay::RetouchEffect { feature, control, face } = req.overlay else { return 0.0 };
        super::effect(&src, None, &info, &plan, feature, control, face).map_or(0.0, |m| at(&m, 0.45, 0.61))
    };
    assert!(effect(0) > 0.05, "face 0's own Teeth Whitening");
    assert_eq!(effect(Overlay::ALL_FACES), 0.0, "the slider for every face is at 0");
}

/// A face with a face mesh, for Face Reshape.
fn meshed() -> SourceInfo {
    let r = FACE;
    let five = vec![(r.x0 + 0.3 * r.width(), r.y0 + 0.45 * r.height()); 5];
    let at = |(fx, fy): (f64, f64)| (r.x0 + fx * r.width(), r.y0 + fy * r.height());
    let mesh = (0..478).map(|i| at(lightcraft_develop::retouch::wrinkles::areas::frontal(i)));
    let face = Face { landmarks: five.into_iter().chain(mesh).collect(), ..Face::boxed(r) };
    SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() }
}

/// Face Reshape's map is where the face moves and how far: 0 at 0, 0 far from the face, more as the value grows.
#[test]
fn reshape_shows_where_the_face_moves() {
    let (src, _) = face_photo();
    let info = meshed();
    let id = "retouch.reshape.slimFace";
    let maps: Vec<Plane> = [0.0, 30.0, 100.0].iter().map(|v| map(&src, &info, id, *v)).collect();
    assert!(maps[0].data.iter().all(|v| *v == 0.0), "at 0 nothing moves");
    let peak = |m: &Plane| m.data.iter().copied().fold(0.0f32, f32::max);
    assert!(peak(&maps[1]) > 0.05, "the face moves: {}", peak(&maps[1]));
    assert!(peak(&maps[1]) < peak(&maps[2]), "further at 100");
    assert_eq!(at(&maps[2], 0.05, 0.05), 0.0, "far from the face nothing moves");
}

/// The overlay's parts and key round trip, and differ per feature, control and face.
#[test]
fn the_overlay_round_trips_and_keys_each_slider_apart() {
    let o = Overlay::RetouchEffect { feature: 3, control: 2, face: 7 };
    let (k, v) = o.to_parts();
    assert_eq!(Overlay::from_parts(k, v), o);
    assert_ne!(o.key(), Overlay::RetouchEffect { feature: 3, control: 2, face: 8 }.key());
    assert_ne!(o.key(), Overlay::RetouchEffect { feature: 3, control: 1, face: 7 }.key());
    assert_ne!(o.key(), Overlay::RetouchEffect { feature: 4, control: 2, face: 7 }.key());
}

/// The drawn view: grey where nothing changes, the glow on the teeth; and a window of it (the zoomed loupe's) is the
/// whole frame's inside it.
#[test]
fn the_view_glows_where_the_slider_works_and_a_window_matches_the_whole() {
    let (src, info) = face_photo();
    let id = "retouch.teethLips.teethWhitening";
    let mut s = DevelopSettings::default();
    s.retouch.global.set(id, 100.0);
    let (w, h) = (640, 480);
    let req = RenderRequest { overlay: overlay(id, Overlay::ALL_FACES), ..RenderRequest::fit(w, h) };
    let whole = render(&src, &info, &s, &req).image;
    let px = |nx: f64, ny: f64| whole.get((nx * whole.width as f64) as usize, (ny * whole.height as f64) as usize);
    let grey = px(0.1, 0.5);
    assert!(grey[0] == grey[1] && grey[1] == grey[2] && grey[0] < 128, "the background dark and grey: {grey:?}");
    let teeth = px(0.45, 0.61);
    assert!(teeth[0] > teeth[2] + 40, "the teeth glow: {teeth:?}");
    let win = PixelWindow { x: 200, y: 200, w: 240, h: 200 };
    let part = render(&src, &info, &s, &RenderRequest { window: Some(win), ..req }).image;
    assert_eq!((part.width, part.height), (win.w, win.h));
    for y in 0..win.h {
        for x in 0..win.w {
            let (a, b) = (part.get(x, y), whole.get(win.x + x, win.y + y));
            assert!(a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 1), "pixel ({x}, {y}) of the window: {a:?} vs {b:?}");
        }
    }
}
