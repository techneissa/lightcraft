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

#[cfg(test)]
mod tests {
    use super::RelightLook;
    use crate::state::UiState;

    /// A `ui.json` with a look this build doesn't know (a hand edit, a newer build) loads with the look Crosses and every
    /// other setting kept, instead of the whole file being set aside.
    #[test]
    fn an_unknown_look_costs_nothing_else() {
        for bad in [serde_json::json!("bogus"), serde_json::json!(7), serde_json::Value::Null] {
            let ui: UiState = serde_json::from_value(serde_json::json!({"relightLook": bad, "hdrVisualize": true})).unwrap();
            assert_eq!(ui.relight_look, RelightLook::Crosses);
            assert!(ui.hdr_visualize, "the rest is kept");
        }
        let ui: UiState = serde_json::from_value(serde_json::json!({"relightLook": "contours"})).unwrap();
        assert_eq!(ui.relight_look, RelightLook::Contours);
    }
}
