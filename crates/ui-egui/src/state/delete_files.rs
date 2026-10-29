use super::{Deserialize, Serialize};

/// What Delete Permanently and Empty Recently Deleted do with the photos' files (Settings ▸
/// General ▸ Culling). Saved as `"ask"` / `"trash"` / `"keep"`; anything else in a hand-edited
/// `ui.json` reads as [`Self::Ask`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeleteFiles {
    /// A window asks each time.
    #[default]
    Ask,
    /// Move the files and their XMP sidecars to the system Trash.
    Trash,
    /// Remove the photos from the library only; the files stay where they are.
    Keep,
}

impl DeleteFiles {
    /// The engine's `files` parameter for a fixed choice.
    pub fn param(self) -> Option<&'static str> {
        match self {
            Self::Ask => None,
            Self::Trash => Some("trash"),
            Self::Keep => Some("keep"),
        }
    }
}
