//! Fork (issue #294, `fork/relight.md`): which picture ⌥ shows while a Relight slider is dragged.

use serde::{Deserialize, Serialize};

/// The ⌥-drag preview of Relight: crosses lying on the surfaces, or contour lines of equal light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RelightLook {
    #[default]
    Crosses,
    Contours,
}
