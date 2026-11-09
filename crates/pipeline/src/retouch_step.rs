//! Fork feature (`fork/retouch-foundation.md`): the portrait retouching step.
//!
//! It runs in the scene-linear stage ([`crate::lin_cpu`]), after white balance, spots and red eye and before noise
//! reduction; the global tone and colour, the local masks, detail and effects all come after it, in the per-pixel
//! finish. The retouch stage runs first, then the background stage. The geometry stage (face reshape) has its slot
//! reserved in `lightcraft_retouch::Stage` and is not run yet.
//!
//! Skipped entirely, touching no pixel and hashing a 0 into the stage key, while the block is off or the Portrait
//! tab's eye is off ([`active`]).

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{Face, Input, KeyMap, MAX_FACES, NormRect, Reads, Stage, View};
use lightcraft_geom::{Affine, Orientation, Point};
use lightcraft_raster::Rgb32f;

use crate::{PixelWindow, Plan, SourceInfo};

/// Whether the step changes anything for `s`.
pub fn active(s: &DevelopSettings) -> bool {
    lightcraft_develop::retouch::active(s)
}

/// What the step adds to the scene-linear stage's cache key: 0 while it is off; else the settings and the input
/// (faces, persons, masks), so a render made before the faces were found is never reused after.
pub fn key(s: &DevelopSettings, info: &SourceInfo) -> u64 {
    if !active(s) {
        return 0;
    }
    crate::hash_of((format!("{:?}", s.retouch), info.retouch.as_ref().map(|i| i.fingerprint()))) | 1
}

/// A photo's lens corrections and transform, for per-face settings kept in the other frame than the faces'
/// ([`lightcraft_develop::retouch::KeyFrame`]): a point after them → the point before them, both before the user's
/// rotation.
pub struct FrameKeyMap {
    frame: crate::geometry::Frame,
    orient: Orientation,
}

impl FrameKeyMap {
    /// The map of `frame` (any frame of the photo's settings: the warp does not depend on the crop or the size).
    pub fn new(frame: &crate::geometry::Frame, orient: Orientation) -> FrameKeyMap {
        FrameKeyMap { frame: frame.for_mask(true).into_owned(), orient }
    }
}

impl KeyMap for FrameKeyMap {
    fn edit_to_photo(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (ox, oy) = self.orient.map(x, y, 1.0, 1.0);
        let p = self.frame.norm_to_mask(Point::new(ox, oy));
        let (ux, uy) = self.orient.inverse().map(p.x, p.y, 1.0, 1.0);
        (ux.is_finite() && uy.is_finite()).then_some((ux, uy))
    }
}

/// The plan's output pixels ↔ normalised photo coordinates (those of spots and faces).
struct FrameView {
    to_norm: Affine,
    to_out: Affine,
    keys: FrameKeyMap,
}

impl FrameView {
    fn new(frame: &crate::geometry::Frame, w: usize, h: usize, orient: Orientation) -> FrameView {
        FrameView { to_norm: frame.out_to_norm(w, h), to_out: frame.norm_to_out(w, h), keys: FrameKeyMap::new(frame, orient) }
    }
}

impl View for FrameView {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        let p = self.to_norm.apply(Point::new(x, y));
        (p.x, p.y)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        let p = self.to_out.apply(Point::new(nx, ny));
        (p.x.is_finite() && p.y.is_finite()).then_some((p.x, p.y))
    }

    fn key_map(&self) -> Option<&dyn KeyMap> {
        Some(&self.keys)
    }
}

/// The plan's output pixels ↔ normalised coordinates on the photo before lens corrections and transform (faces
/// from a people parse, [`Input::on_photo`]): through the frame's warp, as local masks drawn on the photo are.
struct PhotoView {
    to_photo: crate::geometry::MaskMap,
    to_out: Box<dyn Fn(Point) -> Option<Point> + Sync>,
    /// The frame Refine strokes are in (after lens corrections and transform, as [`FrameView`]).
    edit: FrameView,
}

impl PhotoView {
    fn new(frame: &crate::geometry::Frame, w: usize, h: usize, orient: Orientation) -> PhotoView {
        let edit = FrameView::new(frame, w, h, orient);
        let frame = frame.for_mask(true);
        PhotoView { to_photo: frame.out_to_mask(w, h), to_out: Box::new(frame.mask_to_out(w, h)), edit }
    }
}

impl View for PhotoView {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        let p = self.to_photo.apply(Point::new(x, y));
        (p.x, p.y)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        (self.to_out)(Point::new(nx, ny)).filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| (p.x, p.y))
    }

    fn key_map(&self) -> Option<&dyn KeyMap> {
        Some(&self.edit.keys)
    }

    fn to_edit(&self, x: f64, y: f64) -> (f64, f64) {
        self.edit.to_norm(x, y)
    }

    fn edit_to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        self.edit.to_px(nx, ny)
    }
}

/// Retouch `img` (the plan's output pixels) in place: the retouch stage, then the background stage.
pub fn apply(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>) {
    let s = &*p.settings;
    if !active(s) {
        return;
    }
    let none = Input::default();
    let input: &Input = info.retouch.as_deref().unwrap_or(&none);
    let o = s.orientation;
    let view: Box<dyn View> =
        if input.on_photo { Box::new(PhotoView::new(&p.frame, p.w, p.h, o)) } else { Box::new(FrameView::new(&p.frame, p.w, p.h, o)) };
    for stage in [Stage::Retouch, Stage::Background] {
        lightcraft_develop::retouch::run::apply(img, &s.retouch, input, view.as_ref(), stage);
    }
}

/// The window a zoomed render of `win` must render so retouching gives the same pixels there as the whole frame
/// does: `win` grown to hold everything retouching reads for each face whose reads reach into it (the face's box
/// and the areas of the masks the features on read, grown by their [`lightcraft_develop::retouch::Feature::REACH`]
/// times the face's size). The result is cut back to `win` ([`crate::Plan::keep`]), so zoom never changes a
/// retouch. `frame` is the whole frame, `full_w × full_h` its output size.
pub fn window_for_reads(
    s: &DevelopSettings,
    info: &SourceInfo,
    frame: &crate::geometry::Frame,
    full_w: usize,
    full_h: usize,
    win: PixelWindow,
) -> PixelWindow {
    if !active(s) {
        return win;
    }
    let (Some(input), Some(reads)) = (info.retouch.as_deref(), s.retouch.reads()) else { return win };
    let o = s.orientation;
    let view: Box<dyn View> =
        if input.on_photo { Box::new(PhotoView::new(frame, full_w, full_h, o)) } else { Box::new(FrameView::new(frame, full_w, full_h, o)) };
    // where Refine Add strokes widen masks: their points' box grown by their radius, in pixels
    let add = reads.add.and_then(|(b, r)| add_box(b, r, view.as_ref()));
    let (fw, fh) = (full_w as f64, full_h as f64);
    let [wx0, wy0, wx1, wy1] = [win.x as f64, win.y as f64, (win.x + win.w) as f64, (win.y + win.h) as f64];
    let (mut x0, mut y0, mut x1, mut y1) = (wx0, wy0, wx1, wy1);
    for face in input.faces.iter().take(MAX_FACES) {
        let Some([rx0, ry0, rx1, ry1]) = read_box(face, &reads, add, view.as_ref()) else { continue };
        let (rx0, ry0, rx1, ry1) = (rx0.max(0.0), ry0.max(0.0), rx1.min(fw), ry1.min(fh));
        if rx0 < wx1 && rx1 > wx0 && ry0 < wy1 && ry1 > wy0 {
            (x0, y0, x1, y1) = (x0.min(rx0), y0.min(ry0), x1.max(rx1), y1.max(ry1));
        }
    }
    let (x, y) = (x0.floor().max(0.0) as usize, y0.floor().max(0.0) as usize);
    let (r, b) = ((x1.ceil().min(fw) as usize).max(x + 1), (y1.ceil().min(fh) as usize).max(y + 1));
    PixelWindow { x, y, w: r - x, h: b - y }.clamped(full_w, full_h)
}

/// Pixels of a face's box and masks' areas are followed along each edge at this many points (a lens or transform
/// warp bends them).
const EDGE_STEPS: usize = 16;

/// The pixels Refine Add strokes can widen a mask to: the box `b` of their points (the frame [`View::to_edit`] speaks)
/// grown by `r` long edges of that frame. `None` when it maps nowhere.
fn add_box(b: [f64; 4], r: f64, view: &dyn View) -> Option<[f64; 4]> {
    let corners = [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])];
    let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (x, y) in corners {
        let (px, py) = view.edit_to_px(x, y).filter(|(x, y)| x.is_finite() && y.is_finite())?;
        out = [out[0].min(px), out[1].min(py), out[2].max(px), out[3].max(py)];
    }
    let (o, ex, ey) = (view.edit_to_px(0.0, 0.0)?, view.edit_to_px(1.0, 0.0)?, view.edit_to_px(0.0, 1.0)?);
    let len = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let grow = r * len(o, ex).max(len(o, ey)) + 2.0;
    grow.is_finite().then_some([out[0] - grow, out[1] - grow, out[2] + grow, out[3] + grow])
}

/// What retouching reads for `face`, in `view`'s pixels: `[x0, y0, x1, y1]`, unclamped; `None` for a face that is
/// not a box or maps nowhere. `add`: where Refine Add strokes widen masks ([`add_box`]); a face's masks may reach
/// there.
fn read_box(face: &Face, reads: &Reads, add: Option<[f64; 4]>, view: &dyn View) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    add_rect(&mut b, &face.bounds, view);
    // the face's size, as the step sees it: the longer side of its box's bounding box
    let size = (b[2] - b[0]).max(b[3] - b[1]);
    if !(size.is_finite() && size > 0.0) {
        return None;
    }
    for p in &reads.parts {
        if let Some(m) = face.mask(*p) {
            add_rect(&mut b, &m.area, view);
        }
    }
    if let Some(a) = add {
        b = [b[0].min(a[0]), b[1].min(a[1]), b[2].max(a[2]), b[3].max(a[3])];
    }
    // (and a pixel for rounding)
    let grow = reads.reach * size + 2.0;
    grow.is_finite().then_some([b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow])
}

/// Grow the pixel box `b` to hold normalised rectangle `r` as `view` maps it.
fn add_rect(b: &mut [f64; 4], r: &NormRect, view: &dyn View) {
    if !r.is_valid() {
        return;
    }
    for i in 0..=EDGE_STEPS {
        let t = i as f64 / EDGE_STEPS as f64;
        let (u, v) = (r.x0 + t * r.width(), r.y0 + t * r.height());
        for (nx, ny) in [(u, r.y0), (u, r.y1), (r.x0, v), (r.x1, v)] {
            if let Some((px, py)) = view.to_px(nx, ny).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                *b = [b[0].min(px), b[1].min(py), b[2].max(px), b[3].max(py)];
            }
        }
    }
}

/// The faces of a photo for its [`SourceInfo`] (shared: one detection serves every render of the photo).
pub type Faces = Arc<Input>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OutputDepth, RenderRequest, render};
    use lightcraft_develop::controls;
    use lightcraft_develop::retouch::{Face, NormRect, Scene};

    fn scene() -> Rgb32f {
        lightcraft_scenes::demo_library()[0].render(240, 160)
    }

    fn faces(n: usize) -> SourceInfo {
        let faces = (0..n)
            .map(|i| {
                let (x, y) = ((i % 10) as f64 / 10.0, (i / 10) as f64 / 6.0);
                Face { landmarks: vec![(x + 0.03, y + 0.05); 5], ..Face::boxed(NormRect::new(x, y, x + 0.08, y + 0.12)) }
            })
            .collect();
        SourceInfo { retouch: Some(Arc::new(Input::new(faces))), ..SourceInfo::default() }
    }

    /// A block that is there but off (a face with its own settings, all at 0), with faces found: the render is the
    /// render without retouching, byte for byte, and the stage key is the same.
    #[test]
    fn an_all_off_block_renders_byte_identical() {
        let src = scene();
        let plain = DevelopSettings::default();
        let mut off = plain.clone();
        let face = Face::boxed(NormRect::new(0.4, 0.2, 0.6, 0.5));
        off.retouch.override_mut(std::slice::from_ref(&face), 0);
        assert!(!off.retouch.is_default() && off.retouch.is_off());
        // a feature on, but the Portrait tab's eye off
        let mut eye_off = plain.clone();
        controls::set(&mut eye_off, "retouch.skin.smoothness", 60.0);
        eye_off.set_section_enabled("portrait", false);
        for depth in [OutputDepth::U8, OutputDepth::F32Linear] {
            let req = RenderRequest { depth, ..RenderRequest::fit(120, 120) };
            let base = render(&src, &SourceInfo::default(), &plain, &req);
            for (s, info) in [(&off, faces(1)), (&off, SourceInfo::default()), (&eye_off, faces(3))] {
                let r = render(&src, &info, s, &req);
                assert_eq!(r.image, base.image, "{depth:?}");
                assert_eq!(r.deep.map(|d| format!("{:?}", d.samples)), base.deep.as_ref().map(|d| format!("{:?}", d.samples)));
                assert_eq!(key(s, &info), 0);
                let (a, b) = (crate::plan(&src, &info, s, &req), crate::plan(&src, &SourceInfo::default(), &plain, &req));
                assert_eq!((a.pre_nr_key, a.lin_key), (b.pre_nr_key, b.lin_key), "the cache keys of every other photo stay");
            }
        }
        assert!(!crate::lin_needs_cpu(&off, &SourceInfo::default()));
    }

    #[test]
    fn an_active_block_keys_its_settings_and_its_faces() {
        let mut s = DevelopSettings::default();
        controls::set(&mut s, "retouch.eyes.whites", 40.0);
        assert!(active(&s) && crate::lin_needs_cpu(&s, &SourceInfo::default()));
        let none = key(&s, &SourceInfo::default());
        assert_ne!(none, 0);
        assert_ne!(key(&s, &faces(1)), none, "faces found later redo the stage");
        assert_ne!(key(&s, &faces(1)), key(&s, &faces(2)));
        let mut more = s.clone();
        controls::set(&mut more, "retouch.eyes.whites", 41.0);
        assert_ne!(key(&more, &faces(1)), key(&s, &faces(1)));
    }

    #[test]
    fn no_panic_with_no_faces_fifty_faces_or_a_one_pixel_photo() {
        let mut s = DevelopSettings::default();
        for spec in lightcraft_develop::retouch::specs() {
            controls::set(&mut s, spec.id, spec.max);
        }
        for (src, size) in [(scene(), 96), (Rgb32f::filled(1, 1, [0.3, 0.2, 0.1]), 1), (Rgb32f::filled(1, 1, [0.3; 3]), 8)] {
            for info in [SourceInfo::default(), faces(0), faces(50)] {
                let r = render(&src, &info, &s, &RenderRequest::fit(size, size));
                assert!(r.image.width >= 1 && r.image.height >= 1);
            }
        }
    }

    /// Faces are in the photo's coordinates before the crop; the step sees them in the cropped output's pixels.
    #[test]
    fn faces_map_into_the_rendered_pixels() {
        let src = Rgb32f::filled(200, 100, [0.2; 3]);
        let mut s = DevelopSettings::default();
        s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.25, y0: 0.25, x1: 0.75, y1: 0.75 };
        let info = SourceInfo { retouch: Some(Arc::new(Input::new(vec![Face::boxed(NormRect::new(0.4, 0.4, 0.6, 0.6))]))), ..SourceInfo::default() };
        let p = crate::plan(&src, &info, &s, &RenderRequest::fit(100, 100));
        assert_eq!((p.w, p.h), (100, 50));
        let view = FrameView::new(&p.frame, p.w, p.h, Orientation::Normal);
        let input = info.retouch.as_deref().unwrap();
        let scene = Scene::new(input, &view, p.w, p.h);
        let b = scene.faces[0].bounds;
        for (got, want) in b.iter().zip([30.0, 15.0, 70.0, 35.0]) {
            assert!((got - want).abs() < 1e-6, "{b:?}");
        }
        assert!((scene.faces[0].size - 40.0).abs() < 1e-6);
    }

    /// A face with a Facial Skin mask over `area` (all 1), on the photo before lens corrections and transform.
    fn skin_on_photo(area: NormRect) -> SourceInfo {
        use lightcraft_develop::retouch::{Part, PartMask};
        let mask = PartMask { plane: lightcraft_raster::Plane::filled(4, 4, 1.0), area };
        let face = Face { masks: vec![(Part::FacialSkin, mask)], ..Face::boxed(area) };
        SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() }
    }

    fn lit() -> DevelopSettings {
        let mut s = DevelopSettings::default();
        controls::set(&mut s, "retouch.skinTone.faceLight", 100.0);
        s
    }

    /// Without a lens or transform warp, faces on the photo land where faces after the warp do.
    #[test]
    fn faces_on_the_photo_render_as_faces_after_the_warp_without_a_warp() {
        let src = scene();
        let area = NormRect::new(0.3, 0.2, 0.6, 0.7);
        let on_photo = skin_on_photo(area);
        let mut after = on_photo.clone();
        let faces = on_photo.retouch.as_ref().unwrap().faces.clone();
        after.retouch = Some(Arc::new(Input::new(faces)));
        let req = RenderRequest::fit(160, 160);
        let (a, b) = (render(&src, &on_photo, &lit(), &req), render(&src, &after, &lit(), &req));
        assert_eq!(a.image, b.image);
        assert_ne!(a.image, render(&src, &SourceInfo::default(), &lit(), &req).image, "the mask arrived");
    }

    /// With a lens distortion, a mask made on the photo still covers the photo's content it was made on: the step
    /// maps it through the warp. Pixels away from the mask are unchanged.
    #[test]
    fn faces_on_the_photo_follow_the_lens_warp() {
        let (w, h) = (300, 200);
        let area = NormRect::new(0.1, 0.1, 0.3, 0.35);
        let inside = |x: usize, y: usize| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            nx > area.x0 && nx < area.x1 && ny > area.y0 && ny < area.y1
        };
        // a red patch exactly where the mask is, on the photo
        let src = Rgb32f::from_fn(w, h, |x, y| if inside(x, y) { [0.5, 0.05, 0.05] } else { [0.1, 0.1, 0.1] });
        let mut s = lit();
        s.optics.distortion = 80.0;
        let req = RenderRequest::fit(w, h);
        let p = crate::plan(&src, &SourceInfo::default(), &s, &req);
        assert!(p.frame.for_mask(true).mask_warp().is_some(), "the distortion warps the picture");
        let mut off = s.clone();
        controls::set(&mut off, "retouch.skinTone.faceLight", 0.0);
        let plain = render(&src, &SourceInfo::default(), &off, &req).image;
        let done = render(&src, &skin_on_photo(area), &s, &req).image;
        let is_red = |a: [u8; 4]| a[0] > a[1].saturating_add(60);
        // Face Light's soft edge falls to nothing at the mask's border: its inner part is the red pixels at least an
        // edge's width (LIGHT_EDGE of the face's size, the patch's longer side) from anything not red
        let edge = (lightcraft_develop::retouch::skin_tone::LIGHT_EDGE * (area.width() * w as f64).max(area.height() * h as f64)).ceil() as usize;
        let (pw, ph) = (plain.width, plain.height);
        let deep = |x: usize, y: usize| {
            (y.saturating_sub(edge)..=(y + edge).min(ph - 1))
                .all(|yy| (x.saturating_sub(edge)..=(x + edge).min(pw - 1)).all(|xx| is_red(plain.get(xx, yy))))
        };
        let (mut red, mut inner, mut inner_lit, mut grey_lit) = (0, 0, 0, 0);
        for y in 0..ph {
            for x in 0..pw {
                let (a, b) = (plain.get(x, y), done.get(x, y));
                let is_grey = a[0].abs_diff(a[1]) < 4;
                red += usize::from(is_red(a));
                if is_red(a) && deep(x, y) {
                    inner += 1;
                    inner_lit += usize::from(b[0] > a[0]);
                }
                grey_lit += usize::from(is_grey && b != a);
            }
        }
        assert!(inner > 500, "{inner} red pixels an edge's width inside the patch");
        assert!(inner_lit * 100 >= inner * 97, "the warped patch is lit: {inner_lit} of {inner}");
        assert!(grey_lit * 100 <= red * 3, "and little else (its antialiased edge): {grey_lit}");
    }

    /// An on-photo face under a crop and a lens distortion: a draft and a render four times its size light the same
    /// share of the picture, all of the patch the mask was made on, and little else.
    #[test]
    fn a_draft_and_a_bigger_render_agree_with_a_crop_and_a_lens_distortion() {
        let (w, h) = (600, 400);
        let area = NormRect::new(0.15, 0.2, 0.35, 0.45);
        let inside = |x: usize, y: usize| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            nx > area.x0 && nx < area.x1 && ny > area.y0 && ny < area.y1
        };
        let src = Rgb32f::from_fn(w, h, |x, y| if inside(x, y) { [0.5, 0.05, 0.05] } else { [0.1, 0.1, 0.1] });
        let mut s = lit();
        s.optics.distortion = 80.0;
        s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.1, y0: 0.1, x1: 0.8, y1: 0.85 };
        let mut off = s.clone();
        controls::set(&mut off, "retouch.skinTone.faceLight", 0.0);
        let mut shares = Vec::new();
        for (rw, rh) in [(150, 100), (600, 400)] {
            let req = RenderRequest { apply_crop: true, ..RenderRequest::fit(rw, rh) };
            let plain = render(&src, &SourceInfo::default(), &off, &req).image;
            let done = render(&src, &skin_on_photo(area), &s, &req).image;
            let (mut red, mut red_lit, mut grey_lit, mut changed) = (0, 0, 0, 0);
            let ow = plain.width;
            let reddish = |i: Option<usize>| i.and_then(|i| plain.data.get(i)).is_some_and(|a| a[0] > a[1].saturating_add(60));
            for (i, (a, b)) in plain.data.iter().zip(&done.data).enumerate() {
                // (the patch's inside: a pixel of its edge, part grey in a small render, may have its centre past
                // the mask)
                let around = [i.checked_sub(1), Some(i + 1), i.checked_sub(ow), Some(i + ow)];
                let is_red = reddish(Some(i)) && around.iter().all(|n| reddish(*n));
                red += usize::from(is_red);
                red_lit += usize::from(is_red && b[0] > a[0]);
                grey_lit += usize::from(a[0].abs_diff(a[1]) < 4 && b != a);
                changed += usize::from(a != b);
            }
            assert!(red > 100, "{red} red pixels at {rw}×{rh}");
            assert!(red_lit * 100 >= red * 99, "the warped, cropped patch is lit at {rw}×{rh}: {red_lit} of {red}");
            assert!(grey_lit * 100 <= red * 8, "and little else at {rw}×{rh}: {grey_lit}");
            shares.push(changed as f64 / plain.data.len() as f64);
        }
        assert!((shares[0] - shares[1]).abs() < 0.02, "the lit share: {shares:?}");
    }

    /// A synthetic face (skin, a bluish dark circle under each eye, whites, irises) with YuNet's five points and the
    /// Facial Skin, Eye Whites and Iris masks, as a people parse gives them; the photo is `w × h`.
    fn face_photo(w: usize, h: usize) -> (Rgb32f, SourceInfo) {
        use lightcraft_develop::retouch::{Part, PartMask};
        const EYES: [(f64, f64); 2] = [(0.4, 0.4), (0.6, 0.4)];
        let ell = |nx: f64, ny: f64, c: (f64, f64), rx: f64, ry: f64| {
            let d = (((nx - c.0) / rx).powi(2) + ((ny - c.1) / ry).powi(2)).sqrt();
            ((1.1 - d) / 0.2).clamp(0.0, 1.0) as f32
        };
        let whites = move |nx: f64, ny: f64| EYES.iter().map(|c| ell(nx, ny, *c, 0.045, 0.03)).fold(0.0, f32::max);
        let iris = move |nx: f64, ny: f64| EYES.iter().map(|c| ell(nx, ny, *c, 0.018, 0.024)).fold(0.0, f32::max);
        let circle = move |nx: f64, ny: f64| EYES.iter().map(|c| ell(nx, ny, (c.0, c.1 + 0.05), 0.05, 0.03)).fold(0.0, f32::max);
        let src = Rgb32f::from_fn(w, h, |x, y| {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            // a little texture, so smoothing has something to do
            let tex = 1.0 + 0.06 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0 + 0.1 * ((x as f32 / 3.2).sin() * (y as f32 / 3.2).sin());
            let dc = circle(nx, ny);
            let skin = [0.5 * (1.0 - 0.35 * dc) * tex, 0.36 * (1.0 - 0.4 * dc) * tex, 0.28 * (1.0 - 0.2 * dc) * tex];
            let mix = |a: [f32; 3], b: [f32; 3], t: f32| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
            mix(mix(skin, [0.62, 0.5, 0.4], whites(nx, ny)), [0.12, 0.08, 0.05], iris(nx, ny))
        });
        let bounds = NormRect::new(0.28, 0.2, 0.72, 0.8);
        let mask = |f: &dyn Fn(f64, f64) -> f32| {
            let n = 96;
            let plane = lightcraft_raster::Plane::from_fn(n, n, |x, y| {
                f(bounds.x0 + (x as f64 + 0.5) / n as f64 * bounds.width(), bounds.y0 + (y as f64 + 0.5) / n as f64 * bounds.height())
            });
            PartMask { plane, area: bounds }
        };
        let face = Face {
            landmarks: vec![EYES[0], EYES[1], (0.5, 0.52), (0.44, 0.65), (0.56, 0.65)],
            masks: vec![
                (Part::FacialSkin, mask(&|x, y| 1.0 - whites(x, y).max(iris(x, y)))),
                (Part::EyeWhites, mask(&|x, y| whites(x, y) * (1.0 - iris(x, y)))),
                (Part::Iris, mask(&iris)),
            ],
            ..Face::boxed(bounds)
        };
        (src, SourceInfo { retouch: Some(Arc::new(Input::new(vec![face]))), ..SourceInfo::default() })
    }

    /// Zoom never changes a retouch: a zoomed render of a window holding part of one eye (or part of a cheek) gives
    /// the pixels the whole frame's render has there.
    #[test]
    fn a_zoomed_window_gives_the_whole_frames_retouch() {
        let (w, h) = (480, 360);
        let (src, info) = face_photo(w, h);
        let mut s = DevelopSettings::default();
        s.detail.nr_color = 0.0;
        let mut eyes = s.clone();
        for id in ["whites", "iris", "darkCircles", "eyeBags"] {
            controls::set(&mut eyes, &format!("retouch.eyes.{id}"), 100.0);
        }
        let mut skin = s.clone();
        controls::set(&mut skin, "retouch.skin.smoothness", 100.0);
        controls::set(&mut skin, "retouch.skin.shine", 100.0);
        let mut tone = s.clone();
        controls::set(&mut tone, "retouch.skinTone.evenTone", 100.0);
        controls::set(&mut tone, "retouch.skinTone.redness", 100.0);
        controls::set(&mut tone, "retouch.skinTone.faceLight", 50.0);
        let req = RenderRequest::fit(w, h);
        // (the left eye's outer half and the skin under it; a patch of cheek)
        let eye_win = crate::PixelWindow { x: 170, y: 136, w: 30, h: 40 };
        let cheek_win = crate::PixelWindow { x: 200, y: 220, w: 40, h: 30 };
        // (Skin Tone: a window over part of a cheek, under the dark circle Even Tone evens)
        let tone_win = crate::PixelWindow { x: 180, y: 150, w: 30, h: 30 };
        for (name, st, win) in [("eyes", &eyes, eye_win), ("skin", &skin, cheek_win), ("skin tone", &tone, tone_win)] {
            let full = render(&src, &info, st, &req).image;
            let plain = render(&src, &SourceInfo::default(), &s, &req).image;
            let zoomed = render(&src, &info, st, &RenderRequest { window: Some(win), ..req }).image;
            assert_eq!((zoomed.width, zoomed.height), (win.w, win.h));
            let (mut worst, mut changed) = (0u8, 0);
            for y in 0..win.h {
                for x in 0..win.w {
                    let (a, b, c) = (full.get(win.x + x, win.y + y), zoomed.get(x, y), plain.get(win.x + x, win.y + y));
                    worst = worst.max((0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0));
                    changed += usize::from(a != c);
                }
            }
            assert!(changed > win.w * win.h / 4, "{name}: the retouch reaches the window ({changed} changed)");
            assert!(worst <= 1, "{name}: the zoomed window differs from the whole frame by {worst}");
        }
    }

    /// The window grows only for a face whose reads reach it, and not at all with retouching off.
    #[test]
    fn the_window_grows_for_a_retouched_face_it_touches() {
        let (w, h) = (480, 360);
        let (src, info) = face_photo(w, h);
        let mut s = DevelopSettings::default();
        let win = crate::PixelWindow { x: 170, y: 136, w: 30, h: 40 };
        let far = crate::PixelWindow { x: 0, y: 0, w: 20, h: 20 };
        let frame = crate::plan(&src, &info, &s, &RenderRequest::fit(w, h)).frame;
        assert_eq!(window_for_reads(&s, &info, &frame, w, h, win), win, "off: no growth");
        controls::set(&mut s, "retouch.eyes.whites", 50.0);
        let grown = window_for_reads(&s, &info, &frame, w, h, win);
        assert!(grown.x < win.x && grown.w > win.w + 100, "{grown:?}");
        assert_eq!(window_for_reads(&s, &SourceInfo::default(), &frame, w, h, win), win, "no faces");
        // a small face in a corner: a window across the frame from it keeps its size, one on it grows
        let corner = Face::boxed(NormRect::new(0.8, 0.8, 0.9, 0.9));
        let small = SourceInfo { retouch: Some(Arc::new(Input::new(vec![corner]))), ..SourceInfo::default() };
        assert_eq!(window_for_reads(&s, &small, &frame, w, h, far), far, "a window away from the face");
        let on = crate::PixelWindow { x: 400, y: 300, w: 10, h: 10 };
        let g = window_for_reads(&s, &small, &frame, w, h, on);
        assert!(g.x < 384 && g.y < 288 && g.x + g.w >= 432 && g.y + g.h >= 324, "the corner face's reads: {g:?}");
    }
}
