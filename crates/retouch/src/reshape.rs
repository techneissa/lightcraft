//! Face reshape: Slim Face, Jaw, Chin, Double Chin, Nose, Forehead and Eye Size. Moves pixels, so it is not a
//! retouch of the image: it is a geometry step next to Transform ([`Stage::Geometry`]).
//!
//! The sliders become one smooth warp per face ([`Field`], built by [`Field::build`]) that the pipeline puts into its
//! lens and transform warp, on the photo before lens corrections (where the people parse's faces are). Masks drawn on
//! the photo, the face's part masks and every retouch then follow the reshaped face, and the preview, the loupe and
//! the export agree, as they do under Upright.
//!
//! How a slider moves the face ([`kernels`]): it moves a few of the face mesh's points (the face's contour, the jaw,
//! the chin, the nose's wings, the top of the forehead) by a fraction of the face's width or height, or scales the
//! eyes about their centres. Each move spreads to the pixels around it through a smooth bump that falls to nothing at
//! a radius that is a fraction of the face (a Wendland function, C², zero past its radius), so the warp is local:
//! pixels far from the face never move, and a straight line there stays straight. All moves are added, sampled on a
//! grid of cells a fortieth of the face, and read back bilinearly; the grid is checked cell by cell so the warp never
//! folds (each cell stays a convex, same-way-round quadrilateral, which makes the bilinear warp one to one), and the
//! whole face's moves are scaled down until it holds.
//!
//! A face without the face mesh (478 points after YuNet's five) is left alone: the box and five points alone don't
//! say where the jaw is.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

mod field;
mod kernels;

pub use field::{Field, sketch_mesh};
pub use kernels::{Kernel, Kind, MESH, MESH_POINTS, kernels};

/// The Face Reshape settings, each −100…+100 and 0 for none. A field added here is read as its default from
/// settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Reshape {
    /// + draws the cheeks and the jaw's sides in; − widens them.
    pub slim_face: f64,
    /// + widens the jaw at its corners; − narrows it.
    pub jaw: f64,
    /// + lengthens the chin; − shortens it.
    pub chin: f64,
    /// + lifts the soft area under the chin toward the jaw line; − lets it down.
    pub double_chin: f64,
    /// + widens the nose's wings; − narrows them.
    pub nose: f64,
    /// + raises the top of the forehead; − lowers it.
    pub forehead: f64,
    /// + enlarges the eyes; − makes them smaller.
    pub eye_size: f64,
}

impl Feature for Reshape {
    const KEY: &'static str = "reshape";
    const LABEL: &'static str = "Face Reshape";
    const STAGE: Stage = Stage::Geometry;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::signed("retouch.reshape.slimFace", "Slim Face"), get: |s| s.slim_face, set: |s, v| s.slim_face = v },
        Control { spec: Spec::signed("retouch.reshape.jaw", "Jaw"), get: |s| s.jaw, set: |s, v| s.jaw = v },
        Control { spec: Spec::signed("retouch.reshape.chin", "Chin"), get: |s| s.chin, set: |s, v| s.chin = v },
        Control { spec: Spec::signed("retouch.reshape.doubleChin", "Double Chin"), get: |s| s.double_chin, set: |s, v| s.double_chin = v },
        Control { spec: Spec::signed("retouch.reshape.nose", "Nose"), get: |s| s.nose, set: |s, v| s.nose = v },
        Control { spec: Spec::signed("retouch.reshape.forehead", "Forehead"), get: |s| s.forehead, set: |s, v| s.forehead = v },
        Control { spec: Spec::signed("retouch.reshape.eyeSize", "Eye Size"), get: |s| s.eye_size, set: |s, v| s.eye_size = v },
    ];
    /// No masks: the warp reads the face mesh only.
    const PARTS: &'static [Part] = &[];

    /// The face inside its outline (the mesh's face oval, else the ellipse of the face's box): where the warp works.
    fn area(scene: &Scene<'_>, faces: &[usize]) -> Option<lightcraft_raster::Plane> {
        crate::area::outline(scene, faces)
    }

    /// The warp is not a change of the image's pixels: the pipeline builds it from [`Field::build`] and samples the
    /// photo through it. Nothing to do here.
    fn apply(&self, _img: &mut Rgb32f, _scene: &Scene<'_>, _face: Option<usize>) {}
}
