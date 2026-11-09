//! What a retouching feature is ([`Feature`]) and what it gets to work with ([`Scene`]).

use std::fmt::Debug;

use lightcraft_raster::{Plane, Rgb32f};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::control::Control;
use crate::input::{Face, Input, Part};

/// Where in the render a feature runs. The step runs the stages in this order; inside a stage, features run in
/// the order of [`crate::FEATURES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Tone and colour retouching on the scene-linear image, after white balance and before local masks, detail
    /// and effects.
    Retouch,
    /// Moving pixels (face reshape): a geometry step next to Transform. Reserved: the pipeline does not run it
    /// yet; the reshape feature wires it in.
    Geometry,
    /// The background (blur, swap, clean backdrop), after every retouch.
    Background,
}

/// What one run of a feature covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Once per face, with that face's settings (its own when it has them, else the ones for every face).
    Face,
    /// Once for the whole picture, with the settings for every face (per-face settings are not used).
    Picture,
}

/// A retouching feature: its settings struct, its sliders and what it does to the picture.
///
/// `is_off` must be exact: an off feature is never run, and a render with every feature off is byte for byte the
/// render without retouching.
pub trait Feature: Clone + Default + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The key in the settings and in control ids (`retouch.<KEY>.<field>`).
    const KEY: &'static str;
    /// The section title in the Portrait panel (Title Case).
    const LABEL: &'static str;
    const STAGE: Stage;
    const SCOPE: Scope;
    /// Its sliders, in panel order.
    const CONTROLS: &'static [Control<Self>];

    /// Whether these settings leave the picture alone. By default: every slider at its default.
    fn is_off(&self) -> bool {
        Self::CONTROLS.iter().all(|c| c.read(self) == c.spec.default)
    }

    /// Retouch `img` in place. `face` is the face this run is for ([`Scope::Face`]), `None` for a
    /// [`Scope::Picture`] run. Never called while [`Feature::is_off`].
    ///
    /// Sizes are fractions of the face's size ([`FaceCtx::px`]), never pixel counts, so the preview, the loupe and
    /// the export match. Pixels outside the feature's mask must not change, and a face without the masks the
    /// feature needs is left alone.
    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>);
}

/// How the pixels of the image being retouched map to normalised photo coordinates ([`crate::input`]).
pub trait View: Sync {
    /// Pixel position (`x`, `y`; pixel centres at +0.5) → normalised photo coordinates.
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64);
    /// Normalised photo coordinates → pixel position; `None` for a point that has no place in the image.
    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)>;
}

/// The view of an image that is the whole photo (the examples and tests): normalised × size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WholePhoto {
    pub width: usize,
    pub height: usize,
}

impl View for WholePhoto {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        (x / self.width.max(1) as f64, y / self.height.max(1) as f64)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        let p = (nx * self.width as f64, ny * self.height as f64);
        (p.0.is_finite() && p.1.is_finite()).then_some(p)
    }
}

/// One face, mapped into the pixels of the image being retouched.
#[derive(Clone, Debug)]
pub struct FaceCtx<'a> {
    pub face: &'a Face,
    /// Its place in [`Input::faces`].
    pub index: usize,
    /// The face's box in pixels (the bounding box of its mapped corners): x0, y0, x1, y1.
    pub bounds: [f64; 4],
    /// The landmarks in pixels, in [`crate::input::LANDMARKS`] order (`None` where one can't be mapped).
    pub landmarks: Vec<Option<(f64, f64)>>,
    /// The face's size in pixels: the longer side of its box. Every size setting is a fraction of it.
    pub size: f64,
}

impl FaceCtx<'_> {
    /// `fraction` of the face's size, in pixels (0 for a value that is not a number).
    pub fn px(&self, fraction: f64) -> f64 {
        let v = fraction * self.size;
        if v.is_finite() { v.max(0.0) } else { 0.0 }
    }
}

/// Everything a feature run sees besides the image: the faces, mapped into the image's pixels, and their masks.
pub struct Scene<'a> {
    pub faces: Vec<FaceCtx<'a>>,
    pub width: usize,
    pub height: usize,
    view: &'a dyn View,
}

/// Most faces the step retouches (the largest, as the detector ranks them, come first).
pub const MAX_FACES: usize = 100;

impl<'a> Scene<'a> {
    /// The faces of `input` mapped through `view` into a `width × height` image: the first [`MAX_FACES`], less those
    /// whose box is not a box or maps nowhere. `face` arguments of [`Feature::apply`] index this list.
    pub fn new(input: &'a Input, view: &'a dyn View, width: usize, height: usize) -> Scene<'a> {
        let faces = input.faces.iter().take(MAX_FACES).enumerate().filter_map(|(i, f)| map_face(f, i, view)).collect();
        Scene { faces, width, height, view }
    }

    pub fn view(&self) -> &dyn View {
        self.view
    }

    /// The mask of `part` of face `face` at pixel (`x`, `y`) of the image; 0 without that mask.
    pub fn mask_at(&self, face: usize, part: Part, x: usize, y: usize) -> f32 {
        let Some(m) = self.faces.get(face).and_then(|f| f.face.mask(part)) else { return 0.0 };
        let (nx, ny) = self.view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
        m.sample(nx, ny)
    }

    /// The pixels of the image the mask of `part` of face `face` can touch: `[x0, y0, x1, y1)`, clamped to the image;
    /// `None` without that mask or when it lies outside the image. Every pixel outside it has the mask at 0, so a
    /// feature reads and writes only these. (The area's edges are followed at 16 points each, so a lens or
    /// transform warp that bends them stays inside, with a pixel to spare.)
    pub fn mask_box(&self, face: usize, part: Part) -> Option<[usize; 4]> {
        const STEPS: usize = 16;
        let a = self.faces.get(face)?.face.mask(part)?.area;
        if !a.is_valid() || self.width == 0 || self.height == 0 {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for i in 0..=STEPS {
            let t = i as f64 / STEPS as f64;
            let (u, v) = (a.x0 + t * a.width(), a.y0 + t * a.height());
            for (nx, ny) in [(u, a.y0), (u, a.y1), (a.x0, v), (a.x1, v)] {
                if let Some((px, py)) = self.view.to_px(nx, ny).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                    (x0, y0, x1, y1) = (x0.min(px), y0.min(py), x1.max(px), y1.max(py));
                }
            }
        }
        if x0 > x1 || y0 > y1 {
            return None;
        }
        let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
        let b = [
            clamp(x0.floor() - 1.0, self.width),
            clamp(y0.floor() - 1.0, self.height),
            clamp(x1.ceil() + 1.0, self.width),
            clamp(y1.ceil() + 1.0, self.height),
        ];
        (b[0] < b[2] && b[1] < b[3]).then_some(b)
    }

    /// The mask of `part` of face `face`, resampled to the image's pixels; `None` without that mask.
    pub fn mask_plane(&self, face: usize, part: Part) -> Option<Plane> {
        let m = self.faces.get(face)?.face.mask(part)?;
        let view = self.view;
        Some(Plane::from_fn(self.width, self.height, |x, y| {
            let (nx, ny) = view.to_norm(x as f64 + 0.5, y as f64 + 0.5);
            m.sample(nx, ny)
        }))
    }
}

fn map_face<'a>(face: &'a Face, index: usize, view: &dyn View) -> Option<FaceCtx<'a>> {
    let b = face.bounds;
    if !b.is_valid() {
        return None;
    }
    let corners: Vec<(f64, f64)> = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)].iter().filter_map(|(x, y)| view.to_px(*x, *y)).collect();
    let [c0, c1, c2, c3] = corners.as_slice() else { return None };
    let fold = |f: fn(f64, f64) -> f64, pick: fn(&(f64, f64)) -> f64| [c1, c2, c3].iter().fold(pick(c0), |a, c| f(a, pick(c)));
    let bounds = [fold(f64::min, |c| c.0), fold(f64::min, |c| c.1), fold(f64::max, |c| c.0), fold(f64::max, |c| c.1)];
    let side = |a: &(f64, f64), b: &(f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let size = side(c0, c1).max(side(c0, c3));
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let landmarks = face.landmarks.iter().map(|(x, y)| view.to_px(*x, *y).filter(|(px, py)| px.is_finite() && py.is_finite())).collect();
    Some(FaceCtx { face, index, bounds, landmarks, size })
}
