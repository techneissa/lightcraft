//! The retouching step: every enabled feature of a stage, in the fixed order, on one image.

use lightcraft_raster::Rgb32f;

use crate::feature::{Feature, Scene, Scope, Stage, View};
use crate::input::Input;
use crate::refine::Refine;
use crate::{Features, Retouch};

/// Retouch `img` for `stage`: each feature that is on runs once per face with that face's settings (or once for
/// the picture). `view` maps `img`'s pixels to the photo. Does nothing, and touches no pixel, when the block is off
/// ([`Retouch::is_off`]); a feature that is off for a face never runs for it.
pub fn apply(img: &mut Rgb32f, r: &Retouch, input: &Input, view: &dyn View, stage: Stage) {
    if r.is_off() || img.width == 0 || img.height == 0 {
        return;
    }
    let scene = Scene::new(input, view, img.width, img.height);
    let n = input.faces.len().min(crate::feature::MAX_FACES);
    let own = crate::overrides::resolve_in(r, input.faces.get(..n).unwrap_or(&[]), view.key_map());
    let per_face: Vec<(&Features, &Refine)> = scene
        .faces
        .iter()
        .map(|f| own.get(f.index).copied().flatten().and_then(|i| r.faces.get(i)).map_or((&r.global, &r.refine), |o| (&o.settings, &o.refine)))
        .collect();
    crate::run_stage(img, &scene, (&r.global, &r.refine), &per_face, stage);
}

/// Run feature `T` for `stage`: once for the picture, or once per face that has it on, each with its Refine strokes
/// for `T` ([`crate::refine`]; a run without strokes is the feature's own `apply`, nothing more).
pub(crate) fn run_one<'a, T: Feature>(
    img: &mut Rgb32f,
    scene: &Scene<'_>,
    global: (&T, &Refine),
    per_face: impl Iterator<Item = (&'a T, &'a Refine)>,
    stage: Stage,
) {
    if T::STAGE != stage {
        return;
    }
    let run = |img: &mut Rgb32f, s: &T, refine: &Refine, face: Option<usize>| match refine.strokes(T::KEY) {
        [] => s.apply(img, scene, face),
        strokes => crate::refine::run_refined(img, scene, s, face, strokes),
    };
    match T::SCOPE {
        Scope::Picture => {
            if !global.0.is_off() {
                run(img, global.0, global.1, None);
            }
        }
        Scope::Face => {
            for (i, (s, refine)) in per_face.enumerate() {
                if !s.is_off() {
                    run(img, s, refine, Some(i));
                }
            }
        }
    }
}
