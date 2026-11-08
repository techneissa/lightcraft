//! Background: blur, swap and Clean Backdrop. Runs after every retouch, once for the whole picture.
//!
//! Stub (issue #80): one Amount slider and no effect yet. The feature's own issue fills this module, its example
//! (`examples/background.rs`), its test file (`tests/background.rs`) and its section of the Portrait panel; nothing else.

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Scene, Scope, Spec, Stage};

/// The Background settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    /// 0…100; 0 is off.
    pub amount: f64,
}

impl Feature for Background {
    const KEY: &'static str = "background";
    const LABEL: &'static str = "Background";
    const STAGE: Stage = Stage::Background;
    const SCOPE: Scope = Scope::Picture;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.background.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];

    fn apply(&self, _img: &mut Rgb32f, _scene: &Scene<'_>, _face: Option<usize>) {}
}
