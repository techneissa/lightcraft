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
use lightcraft_develop::retouch::{Input, KeyMap, Stage, View};
use lightcraft_geom::{Affine, Orientation, Point};
use lightcraft_raster::Rgb32f;

use crate::{Plan, SourceInfo};

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
    keys: FrameKeyMap,
}

impl PhotoView {
    fn new(frame: &crate::geometry::Frame, w: usize, h: usize, orient: Orientation) -> PhotoView {
        let keys = FrameKeyMap::new(frame, orient);
        let frame = frame.for_mask(true);
        PhotoView { to_photo: frame.out_to_mask(w, h), to_out: Box::new(frame.mask_to_out(w, h)), keys }
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
        Some(&self.keys)
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
        controls::set(&mut s, "retouch.eyes.amount", 40.0);
        assert!(active(&s) && crate::lin_needs_cpu(&s, &SourceInfo::default()));
        let none = key(&s, &SourceInfo::default());
        assert_ne!(none, 0);
        assert_ne!(key(&s, &faces(1)), none, "faces found later redo the stage");
        assert_ne!(key(&s, &faces(1)), key(&s, &faces(2)));
        let mut more = s.clone();
        controls::set(&mut more, "retouch.eyes.amount", 41.0);
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
        controls::set(&mut s, "retouch.skinTone.amount", 100.0);
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
        controls::set(&mut off, "retouch.skinTone.amount", 0.0);
        let plain = render(&src, &SourceInfo::default(), &off, &req).image;
        let done = render(&src, &skin_on_photo(area), &s, &req).image;
        let (mut red, mut red_lit, mut grey_lit) = (0, 0, 0);
        for (a, b) in plain.data.iter().zip(&done.data) {
            let is_red = a[0] > a[1].saturating_add(60);
            let is_grey = a[0].abs_diff(a[1]) < 4;
            red += usize::from(is_red);
            red_lit += usize::from(is_red && b[0] > a[0]);
            grey_lit += usize::from(is_grey && b != a);
        }
        assert!(red > 500, "{red} red pixels");
        assert!(red_lit * 100 >= red * 97, "the warped patch is lit: {red_lit} of {red}");
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
        controls::set(&mut off, "retouch.skinTone.amount", 0.0);
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
}
