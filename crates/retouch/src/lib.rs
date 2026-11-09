//! Portrait retouching for LightCraft (fork feature, `fork/retouch-foundation.md`, issues #79 and #80).
//!
//! - The settings: [`Retouch`] (the `fork_retouch` block of the develop settings): [`Features`] for every face, and
//!   per-face overrides keyed by person or by place ([`FaceKey`], matched by [`overrides::resolve`]).
//! - The input contract: [`input`] (faces, landmarks, persons and part masks, from the engine's providers).
//! - The step: [`run::apply`], one [`Stage`] at a time, every feature in the fixed order of [`FEATURES`].
//! - One module per feature, each with its own settings struct, sliders ([`Control`]) and `apply`.
//! - The Refine brush ([`refine`]): strokes per feature that reduce, restore or add its effect, applied around
//!   every feature's run, so no feature writes code for it.
//!
//! The crate depends on no model, engine, develop, pipeline or UI crate. `crates/retouch/README.md` has the
//! contract and the rules every feature follows.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod control;
pub mod feature;
pub mod input;
pub mod overrides;
mod reduced;
pub mod refine;
pub mod run;
pub mod settings;

pub mod background;
pub mod blemish;
pub mod eyes;
pub mod glare;
pub mod hair;
pub mod reshape;
pub mod skin;
pub mod skin_tone;
pub mod teeth_lips;
pub mod wrinkles;

pub use control::{Control, Spec};
pub use feature::{DEFAULT_REACH, FACE_PARTS, FaceCtx, Feature, MAX_FACES, Scene, Scope, Stage, View, WholePhoto, first_part};
pub use input::{Face, Input, KeyFrame, KeyMap, NormRect, Part, PartMask};
pub use refine::{Refine, RefineLayer, RefineMode, RefineStroke};
pub use settings::{FaceKey, FaceOverride, FeatureInfo, MAX_OVERRIDES, Reads, Retouch};

// The render order: tone and colour retouching first (spots before smoothing, skin before colour, the face before
// what is around it), then the geometry slot, then the background. Each stage runs its features in this order.
settings::features! {
    blemish: blemish::Blemish,
    skin: skin::Skin,
    wrinkles: wrinkles::Wrinkles,
    skin_tone: skin_tone::SkinTone,
    eyes: eyes::Eyes,
    teeth_lips: teeth_lips::TeethLips,
    glare: glare::Glare,
    hair: hair::Hair,
    reshape: reshape::Reshape,
    background: background::Background,
}

/// Every slider of every feature, in panel order.
pub fn controls() -> impl Iterator<Item = &'static Spec> {
    FEATURES.iter().flat_map(|f| (f.specs)())
}

/// The parts some feature reads ([`Feature::PARTS`]), in [`Part::ALL`] order: the only masks the face search keeps.
pub fn parts_read() -> Vec<Part> {
    Part::ALL.into_iter().filter(|p| FEATURES.iter().any(|f| f.parts.contains(p))).collect()
}

/// The spec of control `id` (`retouch.<feature>.<field>`).
pub fn find(id: &str) -> Option<&'static Spec> {
    if !id.starts_with("retouch.") {
        return None;
    }
    controls().find(|s| s.id == id)
}

/// The feature a control belongs to.
pub fn feature_of(id: &str) -> Option<&'static FeatureInfo> {
    let key = id.strip_prefix("retouch.")?.split('.').next()?;
    FEATURES.iter().find(|f| f.key == key)
}
