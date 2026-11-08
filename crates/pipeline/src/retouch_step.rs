//! Fork feature (`fork/retouch-foundation.md`): the portrait retouching step.
//!
//! It runs in the scene-linear stage ([`crate::lin_cpu`]), after white balance, spots and red eye and before noise
//! reduction; the global tone and colour, the local masks, detail and effects all come after it, in the per-pixel
//! finish. The retouch stage runs first, then the background stage. The geometry stage (face reshape) has its slot
//! reserved in `lightcraft_retouch::Stage` and is not run yet.
//!
//! Skipped entirely, touching no pixel and hashing a 0 into the stage key, while the block is off or the Portrait
//! section's eye is off ([`active`]).

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{Input, Stage, View};
use lightcraft_geom::{Affine, Point};
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

/// The plan's output pixels ↔ normalised photo coordinates (those of spots and faces).
struct FrameView {
    to_norm: Affine,
    to_out: Affine,
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
}

/// Retouch `img` (the plan's output pixels) in place: the retouch stage, then the background stage.
pub fn apply(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>) {
    let s = &*p.settings;
    if !active(s) {
        return;
    }
    let none = Input::default();
    let input: &Input = info.retouch.as_deref().unwrap_or(&none);
    let view = FrameView { to_norm: p.frame.out_to_norm(p.w, p.h), to_out: p.frame.norm_to_out(p.w, p.h) };
    for stage in [Stage::Retouch, Stage::Background] {
        lightcraft_develop::retouch::run::apply(img, &s.retouch, input, &view, stage);
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
        // a feature on, but the Portrait section's eye off
        let mut eye_off = plain.clone();
        controls::set(&mut eye_off, "retouch.skin.amount", 60.0);
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
        let view = FrameView { to_norm: p.frame.out_to_norm(p.w, p.h), to_out: p.frame.norm_to_out(p.w, p.h) };
        let input = info.retouch.as_deref().unwrap();
        let scene = Scene::new(input, &view, p.w, p.h);
        let b = scene.faces[0].bounds;
        for (got, want) in b.iter().zip([30.0, 15.0, 70.0, 35.0]) {
            assert!((got - want).abs() < 1e-6, "{b:?}");
        }
        assert!((scene.faces[0].size - 40.0).abs() < 1e-6);
    }
}
