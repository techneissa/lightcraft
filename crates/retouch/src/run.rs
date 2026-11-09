//! The retouching step: every enabled feature of a stage, in the fixed order, on one image.

use lightcraft_raster::Rgb32f;

use crate::feature::{Feature, Scene, Scope, Stage, View};
use crate::input::Input;
use crate::refine::{Layer, Refine};
use crate::{Features, Retouch};

/// Retouch `img` for `stage`: each feature that is on runs once per face with that face's settings (or once for
/// the picture). `view` maps `img`'s pixels to the photo. Does nothing, and touches no pixel, when the block is off
/// ([`Retouch::is_off`]); a feature that is off for a face never runs for it.
pub fn apply(img: &mut Rgb32f, r: &Retouch, input: &Input, view: &dyn View, stage: Stage) {
    if r.is_off() || img.width == 0 || img.height == 0 {
        return;
    }
    let scene = Scene::new(input, view, img.width, img.height);
    // (strokes beyond what a render rasterises are left out: the photo renders as if without them)
    let strokes = crate::refine::check_budget(r).is_ok();
    let per_face = layers(r, input, &scene, strokes);
    let global = Layer { strokes: if strokes { &r.refine } else { &NO_STROKES }, picked: false };
    crate::run_stage(img, &scene, (&r.global, global), &per_face, stage);
}

static NO_STROKES: Refine = Refine(Vec::new());

/// What each face of `scene` renders with: its own settings and strokes when it has them (strokes picked for it
/// alone), else those for every face. Without `strokes` (a photo over its work cap) no Refine strokes at all.
pub(crate) fn layers<'a>(r: &'a Retouch, input: &Input, scene: &Scene<'_>, strokes: bool) -> Vec<(&'a Features, Layer<'a>)> {
    let n = input.faces.len().min(crate::feature::MAX_FACES);
    let own = crate::overrides::resolve_in(r, input.faces.get(..n).unwrap_or(&[]), scene.view().key_map());
    let layer = |refine: &'a Refine, picked| Layer { strokes: if strokes { refine } else { &NO_STROKES }, picked };
    scene
        .faces
        .iter()
        .map(|f| match own.get(f.index).copied().flatten().and_then(|i| r.faces.get(i)) {
            Some(o) => (&o.settings, layer(&o.refine, true)),
            None => (&r.global, layer(&r.refine, false)),
        })
        .collect()
}

/// Run feature `T` for `stage`: once for the picture, or once per face that has it on, each with its Refine strokes
/// for `T` ([`crate::refine`]; a run without strokes is the feature's own `apply`, nothing more).
pub(crate) fn run_one<'a, T: Feature>(
    img: &mut Rgb32f,
    scene: &Scene<'_>,
    global: (&T, Layer<'_>),
    per_face: impl Iterator<Item = (&'a T, Layer<'a>)>,
    stage: Stage,
) {
    if T::STAGE != stage {
        return;
    }
    let run = |img: &mut Rgb32f, s: &T, layer: Layer<'_>, face: Option<usize>| match layer.strokes.strokes(T::KEY) {
        [] => s.apply(img, scene, face),
        strokes => crate::refine::run_refined(img, scene, s, face, strokes, layer.picked),
    };
    match T::SCOPE {
        Scope::Picture => {
            if !global.0.is_off() {
                run(img, global.0, global.1, None);
            }
        }
        Scope::Face => {
            for (i, (s, layer)) in per_face.enumerate() {
                if !s.is_off() {
                    run(img, s, layer, Some(i));
                }
            }
        }
    }
}
