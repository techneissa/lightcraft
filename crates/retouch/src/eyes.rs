//! Eyes: Whites, Iris, Dark Circles and Eye Bags (fork feature, `fork/retouch-eyes.md`).
//!
//! Four sliders, 0…100, each off at 0, per face:
//! - **Whites** brightens the eye whites, more when they sit in shadow, keeping their shading (the corners and the
//!   shadow under the upper lid stay darker), and takes most of their colour out (a yellow or red cast, veins), so
//!   veins stay as faint grey lines rather than vanishing.
//! - **Iris** brings out the iris's pattern (its fibres' contrast, a little clarity), brightens it between the pupil
//!   and the limbal ring, more where light enters below, and adds a little of its own colour; the pupil, the ring and
//!   the catchlight stay.
//! - **Dark Circles** lifts the shadow under each eye and turns its colour (blue, purple, brown) toward the cheek's
//!   below it, keeping the skin's texture.
//! - **Eye Bags** flattens the shading of the bag under each eye (the bulge and the crease below it), keeping the
//!   skin's fine texture.
//!
//! Where: Whites and Iris on their part masks (a face without them, closed eyes, is left alone), cut to the eye's
//! opening between the lids and the iris to the mesh's iris circle when the face has the 478 mesh landmarks; Dark Circles and Eye Bags in crescents under the
//! lower lids, placed from the landmarks ([`geom`]) and limited to the Facial Skin mask, never on the eye, lashes or
//! glasses (Others). A face without Facial Skin or without both eye landmarks gets no under-eye change. Every size is
//! a fraction of the eye's width, which comes from the landmarks and is kept within a range of the face's size, so
//! the preview, the loupe and the export match.
//!
//! Each effect is a per-pixel gain times one weight per area ([`Area`], made in [`weights`] only), so the Refine
//! brush (#99) can scale or extend an area's weight there without touching the effects.

mod geom;
mod grid;
mod tone;
mod under;

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

pub use geom::Eye;

/// The Eyes settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Eyes {
    /// 0…100; 0 is off.
    pub whites: f64,
    /// 0…100; 0 is off.
    pub iris: f64,
    /// 0…100; 0 is off.
    pub dark_circles: f64,
    /// 0…100; 0 is off.
    pub eye_bags: f64,
}

/// The areas the effects work in, one weight each (0…1 per pixel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Whites,
    Iris,
    DarkCircles,
    EyeBags,
}

/// A slider's value as 0…1 (0 for a value that is not a number).
fn unit(v: f64) -> f32 {
    if v.is_finite() { (v.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 }
}

impl Feature for Eyes {
    const KEY: &'static str = "eyes";
    const LABEL: &'static str = "Eyes";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const PARTS: &'static [Part] = &[Part::FacialSkin, Part::Others, Part::EyeWhites, Part::Iris];
    /// The under-eye grid reaches about 4.7 half eye widths from an eye's centre (at most 0.12 of the face's size
    /// each, [`geom::HALF_RANGE`]), and an eye's centre is within [`geom::CENTRE_SLACK`] of the face's box: 0.56 +
    /// 0.1, rounded up. Whites and Iris read inside their masks' areas (the iris's blur a few pixels past).
    const REACH: f64 = 0.7;
    /// Refine Add paints more of the eye whites.
    const ADD_PARTS: &'static [Part] = &[Part::EyeWhites];
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::amount("retouch.eyes.whites", "Whites"), get: |s| s.whites, set: |s, v| s.whites = v },
        Control { spec: Spec::amount("retouch.eyes.iris", "Iris"), get: |s| s.iris, set: |s, v| s.iris = v },
        Control { spec: Spec::amount("retouch.eyes.darkCircles", "Dark Circles"), get: |s| s.dark_circles, set: |s, v| s.dark_circles = v },
        Control { spec: Spec::amount("retouch.eyes.eyeBags", "Eye Bags"), get: |s| s.eye_bags, set: |s, v| s.eye_bags = v },
    ];

    /// The eye whites and the irises, and the crescents under the lower lids where Dark Circles and Eye Bags work
    /// (on the Facial Skin mask).
    fn area(scene: &Scene<'_>, faces: &[usize]) -> Option<lightcraft_raster::Plane> {
        crate::feature::union_area(scene, faces, |f| {
            let ctx = scene.faces.get(f)?;
            let mut out = crate::feature::parts_area(scene, &[f], &[Part::EyeWhites, Part::Iris])
                .unwrap_or_else(|| lightcraft_raster::Plane::filled(scene.width, scene.height, 0.0));
            if ctx.face.mask(Part::FacialSkin).is_some() {
                for eye in geom::eyes(ctx) {
                    let Some([x0, y0, x1, y1]) = eye.pixel_box(weights::UNDER_BOX, 2.0, scene.width, scene.height) else { continue };
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let (a, b) = eye.local(x as f64 + 0.5, y as f64 + 0.5);
                            let t = eye.below_lid(a, b);
                            let w =
                                weights::of(Area::DarkCircles, a, t).max(weights::of(Area::EyeBags, a, t)) * scene.mask_at(f, Part::FacialSkin, x, y);
                            if let Some(o) = out.data.get_mut(y * scene.width + x) {
                                *o = o.max(w);
                            }
                        }
                    }
                }
            }
            Some(out)
        })
    }

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        if img.width == 0 || img.height == 0 || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(ctx) = scene.faces.get(face) else { return };
        let eyes = geom::eyes(ctx);
        // the under-eye effects read the skin around the eyes, so they go first, on the untouched eyes
        let (dark, bags) = (unit(self.dark_circles), unit(self.eye_bags));
        if dark > 0.0 || bags > 0.0 {
            for eye in &eyes {
                under::apply(img, scene, face, eye, dark, bags);
            }
        }
        let whites = unit(self.whites);
        if whites > 0.0 {
            tone::whites(img, scene, face, &eyes, whites);
        }
        let iris = unit(self.iris);
        if iris > 0.0 {
            tone::iris(img, scene, face, &eyes, iris);
        }
    }
}

/// The weights of the areas at one pixel, from what [`geom`] and the masks give. The one place an area's extent is
/// decided: the Refine brush (#99) multiplies or adds its strokes here.
pub mod weights {
    use super::Area;
    use super::geom::{smoothstep, window};

    /// How far below the lower lid (in half eye widths) the under-eye areas start, so lashes stay out.
    pub const LID_GAP: f64 = 0.06;

    /// The automatic weight of `area` at eye-local point (`a`, `t`): `a` across the eye in half widths (−1 outer
    /// corner, +1 inner corner), `t` below the lower lid in half widths. Whites and Iris have no geometric extent
    /// (their masks are it): 1.
    pub fn of(area: Area, a: f64, t: f64) -> f32 {
        let w = match area {
            Area::Whites | Area::Iris => 1.0,
            Area::DarkCircles => {
                // a crescent from just below the lashes, deepest under the middle, shallow at the sides, at full
                // weight past where a circle ends: the skin's own level decides what is lifted (skin as light as
                // the skin outside the circle is not), so the circle's edge, however sharp, is lifted whole
                // (beside the corners it starts lower: the lid's dark rim and the corner's shadow reach below them)
                let deep = dark_deep(a);
                let corner = 0.3 * smoothstep(0.8, 1.2, a.abs());
                window(a, -1.3, -0.65, 0.95, 1.4)
                    * smoothstep(LID_GAP + corner, LID_GAP + 0.18 + corner, t)
                    * (1.0 - smoothstep(deep, deep + 0.15, t))
            }
            Area::EyeBags => {
                let deep = 1.45 * (1.0 - 0.3 * a * a).max(0.25);
                window(a, -1.35, -0.75, 1.0, 1.45) * smoothstep(LID_GAP, LID_GAP + 0.2, t) * (1.0 - smoothstep(0.55 * deep, deep, t))
            }
        };
        w as f32
    }

    /// How far below the lid (in half widths) Dark Circles' crescent reaches at `a`: deepest under the middle.
    fn dark_deep(a: f64) -> f64 {
        (1.0 - 0.3 * a * a).max(0.25)
    }

    /// The skin just outside Dark Circles' crescent, below it and beside it, whose lightness the circle is lifted to:
    /// a band from the crescent's lower edge down 0.55 half widths, and the skin beside the eye's corners.
    pub fn ring(a: f64, t: f64) -> f32 {
        let deep = dark_deep(a);
        let below = window(a, -1.3, -0.65, 0.95, 1.4) * window(t, deep, deep + 0.15, deep + 0.4, deep + 0.55);
        let beside = window(a, -1.8, -1.65, -1.45, -1.3).max(window(a, 1.4, 1.5, 1.7, 1.85)) * window(t, 0.15, 0.25, 0.7, 0.9);
        below.max(beside) as f32
    }

    /// The local box (in `a`, `t`) every under-eye weight and the ring lie in: `[a0, a1, t0, t1]`.
    pub const UNDER_BOX: [f64; 4] = [-1.85, 1.9, -0.1, 1.55];
    /// The part of it the effects reach (Dark Circles and Eye Bags are 0 outside it).
    pub const EFFECT_BOX: [f64; 4] = [-1.4, 1.5, -0.1, 1.5];
}
