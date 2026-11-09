//! Skin: Smoothness, Texture and Shine. Evens the skin's surface while keeping its pores.
//!
//! Stub (issue #80): one Amount slider and no effect yet. The feature's own issue fills this module, its example
//! (`examples/skin.rs`), its test file (`tests/skin.rs`) and its section of the Portrait panel; nothing else.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Scene, Scope, Spec, Stage};

/// The Skin settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skin {
    /// 0…100; 0 is off.
    pub amount: f64,
}

impl Feature for Skin {
    const KEY: &'static str = "skin";
    const LABEL: &'static str = "Skin";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.skin.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];

    fn apply(&self, _img: &mut Rgb32f, _scene: &Scene<'_>, _face: Option<usize>) {}
}
