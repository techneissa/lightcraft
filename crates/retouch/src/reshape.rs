//! Face reshape: slim face, jaw, chin, double chin, nose, forehead, eye size. Moves pixels: runs in the geometry slot next to Transform.
//!
//! Stub (issue #80): one Amount slider and no effect yet. The feature's own issue fills this module, its example
//! (`examples/reshape.rs`), its test file (`tests/reshape.rs`) and its section of the Portrait panel; nothing else.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

/// The Face Reshape settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reshape {
    /// 0…100; 0 is off.
    pub amount: f64,
}

impl Feature for Reshape {
    const KEY: &'static str = "reshape";
    const LABEL: &'static str = "Face Reshape";
    const STAGE: Stage = Stage::Geometry;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.reshape.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];
    const PARTS: &'static [Part] = &[];

    fn apply(&self, _img: &mut Rgb32f, _scene: &Scene<'_>, _face: Option<usize>) {}
}
