//! Settings sync (`fork/settings-sync.md`): move all of LightCraft's settings between Macs in one step.
//! Export Settings… and Import Settings… use one `.lcsettings` file; Save to Folder and Load from
//! Folder keep it in a folder the user chose (one in iCloud Drive, say), and Keep in Sync saves
//! there on quit and offers a newer file from another Mac at start. Settings ▸ Sync has the buttons.
//!
//! The file is JSON: `kind` `lightcraft-settings`, `version`, `app`, and two parts, each optional on
//! import: `ui` holds what `ui.json` holds minus the keys of one Mac ([`rules`]: window, side bar
//! widths, docking, zoom and pan, folders…, built from a deny list so a new setting syncs without a
//! change here), and `presets` holds the user presets as a `.lcpreset` file. A folder save adds a
//! top-level `savedBy` (`lightcraft_engine::settings_folder`).
//!
//! An import reads the whole file first ([`plan`]) and changes nothing until [`apply`]: a bad value for
//! a setting this build knows is an error that names it and changes nothing; a setting, section or
//! part this build doesn't have (from a newer LightCraft) is skipped and named; a part the file lacks
//! leaves that setting as it is.
//!
//! The same contract as PhotoCraft's Settings sync, so both apps share one folder: the folder is
//! also kept in `~/Library/Application Support/ArtCraft/sync-folder.json`.
//!
//! - [`rules`]: which `ui.json` keys are synced and which stay on one Mac, and the file's text.
//! - [`plan`]: reading a file against the settings now, and applying it.
//! - [`run`]: the commands, the prompt, and the folder, Keep in Sync at start and on quit.

use serde::{Deserialize, Serialize};

pub mod plan;
mod prompt;
pub mod rules;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod run;

#[cfg(test)]
mod page_tests;
#[cfg(test)]
mod tests;

pub use plan::{Plan, apply, plan};
pub use prompt::{confirm, prompt_body};
#[cfg(not(target_arch = "wasm32"))]
pub use run::{command, offer_at_start, save_on_quit};

/// What a settings file says it is (`kind`).
pub const KIND: &str = "lightcraft-settings";
/// The version this build writes and the newest it knows.
pub const VERSION: u64 = 1;
/// The biggest settings file read (user presets can make one a few megabytes).
pub const MAX_BYTES: usize = 32_000_000;

/// The commands of Settings sync.
pub const COMMANDS: [&str; 5] = ["settings.export", "settings.import", "settings.syncSave", "settings.syncLoad", "settings.syncChoose"];

/// Settings sync is for the desktop app: the web has no folders to sync to.
#[cfg(target_arch = "wasm32")]
pub fn command(_app: &mut crate::LightcraftApp, _id: &str, _p: &serde_json::Value) -> Result<serde_json::Value, String> {
    Err("Settings sync isn't available on the web".into())
}

/// Nothing to offer on the web.
#[cfg(target_arch = "wasm32")]
pub fn offer_at_start(_app: &mut crate::LightcraftApp) {}

/// Nothing to save on the web.
#[cfg(target_arch = "wasm32")]
pub fn save_on_quit(_app: &mut crate::LightcraftApp) {}

/// The load prompt: what a file would change, shown before anything does.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    /// What was read.
    pub plan: Plan,
    /// The file's name.
    pub name: String,
    /// "Saved on <Mac>, <when>", when the file says who saved it.
    pub saved_line: Option<String>,
    /// The `savedBy.at` of a file from the sync folder: OK notes that this Mac has it.
    pub saved_at: Option<String>,
    /// The file came from the sync folder.
    pub from_folder: bool,
}

/// What Settings sync keeps between frames (a field of [`crate::LightcraftApp`]).
#[derive(Default)]
pub struct Runtime {
    /// Where this Mac's name and the shared folder file come from; tests set them so nothing real is touched.
    #[cfg(not(target_arch = "wasm32"))]
    pub env: lightcraft_engine::settings_folder::SyncEnv,
    /// The prompt the open load dialog shows.
    pub prompt: Option<Prompt>,
    /// Keep in Sync looked at the folder (once, at start).
    pub started: bool,
    /// A newer file from another Mac, to offer when no other dialog is open.
    pub offer: Option<Prompt>,
}

/// `sync` of the UI state: this Mac's sync choices. Never part of the settings file (each Mac has
/// its own), so an agent can't point the commands at another folder either (`ui.set` refuses it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncPrefs {
    /// The chosen sync folder; empty when none is chosen.
    pub folder: String,
    /// Keep in Sync: save to the folder on quit, and offer a newer file from another Mac at start.
    pub keep: bool,
    /// The `savedBy.at` of the file this Mac saved to the folder or loaded from it last.
    pub last: String,
    /// User presets go into the file (and Save to Folder), on by default.
    pub presets: bool,
}

impl Default for SyncPrefs {
    fn default() -> Self {
        Self { folder: String::new(), keep: false, last: String::new(), presets: true }
    }
}
