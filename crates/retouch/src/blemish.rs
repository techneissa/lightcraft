//! Blemishes: Amount, Keep Moles and Freckles. Removes spots on the skin and keeps moles and freckles when asked.
//!
//! Stub (issue #80): one Amount slider and no effect yet. The feature's own issue fills this module, its example
//! (`examples/blemish.rs`), its test file (`tests/blemish.rs`) and its section of the Portrait panel; nothing else.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Scene, Scope, Spec, Stage};

/// The Blemishes settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Blemish {
    /// 0…100; 0 is off.
    pub amount: f64,
}

impl Feature for Blemish {
    const KEY: &'static str = "blemish";
    const LABEL: &'static str = "Blemishes";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.blemish.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];

    fn apply(&self, _img: &mut Rgb32f, _scene: &Scene<'_>, _face: Option<usize>) {}
}
