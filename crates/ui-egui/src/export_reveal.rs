//! Export ▸ "Show in Finder": after a successful export the desktop app shows the first file
//! written in the system file manager ([`ExportOptions::show_in_finder`]). The opener lives in
//! [`crate::Services::reveal`], which only the desktop host sets, so headless runs, the CLI and MCP
//! never spawn anything; a failed opener is a toast, never a panic.

use lightcraft_engine::export::ExportOptions;
use serde_json::Value;

use crate::LightcraftApp;

/// The checkbox's label: the file manager's name on this platform.
pub fn label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else if cfg!(target_os = "windows") {
        "Show in File Explorer"
    } else {
        "Show in File Manager"
    }
}

/// Export dialog, after export: the checkbox.
pub fn row(ui: &mut egui::Ui, opts: &mut ExportOptions) {
    let r = ui.checkbox(&mut opts.show_in_finder, crate::i18n::tr(label()));
    crate::widgets::register(ui, "exportShowInFinder", r.rect);
}

/// The first file an export wrote (`path` of a per-photo result); skipped and failed photos have none.
fn first_written(files: &[Value]) -> Option<&str> {
    files.iter().find_map(|f| f.get("path").and_then(Value::as_str))
}

/// Reveal the export's first file when `show` is on and something was written. `Err` says why
/// the file manager did not open; `Ok(false)` means nothing to do (option off, no file, no opener).
pub fn reveal_exported(app: &mut LightcraftApp, show: bool, files: &[Value]) -> Result<bool, String> {
    if !show {
        return Ok(false);
    }
    let (Some(path), Some(reveal)) = (first_written(files), app.services.reveal.as_mut()) else { return Ok(false) };
    reveal(path).map(|()| true)
}

/// [`reveal_exported`] for a finished export: the note to add to its summary when the file
/// manager did not open.
pub fn reveal_note(app: &mut LightcraftApp, show: bool, files: &[Value]) -> Option<String> {
    let e = reveal_exported(app, show, files).err()?;
    Some(crate::i18n::tr_format!(" · couldn't show the file: {e}", e = e))
}
