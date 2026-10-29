//! Delete Permanently and Empty Recently Deleted: what happens to the photos' files.
//!
//! The setting (Settings ▸ General ▸ Culling ▸ Delete Permanently, [`DeleteFiles`]) picks it once;
//! on Ask Each Time this dialog asks: Move to Trash, Keep Files, or Cancel. Local photos (browsed
//! from a folder, never in the library) have nothing to be removed from: their files always go to
//! the Trash, so a set of only Local photos gets a plain Cancel / Move to Trash question. The engine
//! commands take `files: "keep" | "trash"` (`crates/engine/src/file_trash.rs`). [`run_item`] is
//! where menus, context menus and keys arrive; a command run by id (the control channel, MCP)
//! never asks and keeps the files unless it says `trash`.

use serde_json::{Value, json};

use lightcraft_engine::catalog::PhotoId;

use crate::LightcraftApp;
use crate::state::Dialog;
use crate::theme::Tokens;

const DELETE: &str = "photo.deletePermanently";
const EMPTY: &str = "library.emptyRecentlyDeleted";

/// A command that `run_item` sends through the setting.
pub(crate) fn handles(id: &str) -> bool {
    id == DELETE || id == EMPTY
}

/// The photos the command would remove.
fn targets(app: &LightcraftApp, id: &str, params: &Value) -> Vec<PhotoId> {
    if id == EMPTY {
        return app.session.catalog.photos().filter(|p| p.deleted).map(|p| p.id).collect();
    }
    match params.get("ids").and_then(Value::as_array) {
        Some(ids) => ids.iter().filter_map(Value::as_u64).map(PhotoId).collect(),
        None => app.session.selected(),
    }
}

/// A menu, context-menu or key run of Delete Permanently / Empty Recently Deleted: with Ask Each
/// Time it opens the dialog (nothing runs yet), else it runs with the setting's choice and says
/// what happened. `None`: not one of these commands, or `files` was given: run as usual.
pub(crate) fn run_item(app: &mut LightcraftApp, id: &str, params: &mut Value) -> Option<Result<Value, String>> {
    if !handles(id) || params.get("files").is_some() {
        return None;
    }
    let ids = targets(app, id, params);
    let n = ids.len();
    if n == 0 {
        return None;
    }
    let local = ids.iter().filter(|id| app.session.catalog.photo(**id).is_some_and(|p| p.local)).count();
    let Some(files) = app.ui.settings.delete_files.param() else {
        app.ui.dialog = Some(Dialog::DeletePermanently { count: n, local, empty: id == EMPTY, files: "trash".into() });
        return Some(Ok(Value::Null));
    };
    Some(run(app, id, params.clone(), files))
}

/// Run the command with `files`, then say how it went.
fn run(app: &mut LightcraftApp, id: &str, mut params: Value, files: &str) -> Result<Value, String> {
    params["files"] = json!(files);
    let r = app.run(id, params);
    match &r {
        Ok(r) => say(app, &outcome(r)),
        Err(e) => say(app, e),
    }
    r
}

fn say(app: &mut LightcraftApp, text: &str) {
    if text.is_empty() {
        return;
    }
    let until = app.tick_now() + 6.0;
    app.ui.toast = Some((text.to_string(), until, None));
}

/// The toast for a result: nothing when no file was asked for.
fn outcome(r: &Value) -> String {
    let trashed = r["trashed"].as_array().map_or(0, Vec::len);
    let failed = r["failed"].as_array().map_or(0, Vec::len);
    let kept = if failed == 0 {
        String::new()
    } else {
        let why = r["failed"][0][1].as_str().unwrap_or_default();
        crate::i18n::tr_format!(
            "Could not move {failed} file{} to the Trash, so the photos stayed in the library: {why}",
            if failed == 1 { "" } else { "s" },
            failed = failed,
            why = why
        )
    };
    let moved = match trashed {
        0 => String::new(),
        1 => crate::i18n::tr("Moved 1 file to the Trash. Undo brings back the photo, not the file: get the file back from the Trash.").to_string(),
        n => crate::i18n::tr_format!(
            "Moved {n} files to the Trash. Undo brings back the photos, not the files: get the files back from the Trash.",
            n = n
        ),
    };
    [moved, kept].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The dialog's title: "Delete 3 Photos Permanently?", or "Move 3 Photos to the Trash?" for Local photos.
pub(crate) fn title(dlg: &Dialog) -> String {
    let Dialog::DeletePermanently { count, local, .. } = dlg else { return String::new() };
    match (*local == *count, *count) {
        (true, 1) => crate::i18n::tr("Move 1 Photo to the Trash?").to_string(),
        (true, n) => crate::i18n::tr_format!("Move {n} Photos to the Trash?", n = n),
        (false, 1) => crate::i18n::tr("Delete 1 Photo Permanently?").to_string(),
        (false, n) => crate::i18n::tr_format!("Delete {n} Photos Permanently?", n = n),
    }
}

/// The text of the dialog: what each choice does.
pub(crate) fn body(ui: &mut egui::Ui, t: &Tokens, dlg: &Dialog) {
    let Dialog::DeletePermanently { count, local, empty, .. } = dlg else { return };
    if local == count {
        let what = match (*empty, *count) {
            (true, 1) => crate::i18n::tr("The photo in Recently Deleted goes away, and its file goes to the Trash.").to_string(),
            (true, n) => crate::i18n::tr_format!("All {n} photos in Recently Deleted go away, and their files go to the Trash.", n = n),
            (false, 1) => crate::i18n::tr("This photo goes away, and its file goes to the Trash.").to_string(),
            (false, n) => crate::i18n::tr_format!("These {n} photos go away, and their files go to the Trash.", n = n),
        };
        ui.label(what);
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "The files and their XMP sidecars go to the Trash, and you can get them back from there. Undo brings back the photos, not the files.",
            ))
            .color(t.text_dim),
        );
        return;
    }
    let what = match (*empty, *count) {
        (true, 1) => crate::i18n::tr("The photo in Recently Deleted will be removed from your library.").to_string(),
        (true, n) => crate::i18n::tr_format!("All {n} photos in Recently Deleted will be removed from your library.", n = n),
        (false, 1) => crate::i18n::tr("This photo will be removed from your library.").to_string(),
        (false, n) => crate::i18n::tr_format!("These {n} photos will be removed from your library.", n = n),
    };
    ui.label(what);
    ui.add_space(4.0);
    ui.label(egui::RichText::new(crate::i18n::tr("Move to Trash")).strong());
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "Their files and XMP sidecars go to the Trash, and you can get them back from there. Undo brings back the photos, not the files.",
        ))
        .color(t.text_dim),
    );
    ui.add_space(4.0);
    ui.label(egui::RichText::new(crate::i18n::tr("Keep Files")).strong());
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "LightCraft forgets the photos. Their files stay in their folders, and a folder you open again can show them as new photos.",
        ))
        .color(t.text_dim),
    );
    if *local > 0 {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(crate::i18n::tr_format!(
                "{n} of them are Local photos (browsed, not in your library): their files go to the Trash either way.",
                n = local
            ))
            .color(t.text_dim),
        );
    }
}

/// The second button: Keep Files, left of the confirm. It confirms with the files kept (Local
/// photos' files still go to the Trash: the engine does that).
fn extra(kind: std::mem::Discriminant<Dialog>) -> crate::button::Button<'static> {
    crate::button::Button::new(crate::i18n::tr("Keep Files")).widget("button:dialogKeepFiles").id_salt(kind).min_width(72.0)
}

/// The room the second button takes in the button row (with the gap before it).
pub(crate) fn extra_width(ui: &egui::Ui, dlg: &Dialog) -> f32 {
    if has_extra(dlg) { extra(std::mem::discriminant(dlg)).desired_size(ui).x + ui.spacing().item_spacing.x } else { 0.0 }
}

/// Only a set with library photos in it has files to keep.
fn has_extra(dlg: &Dialog) -> bool {
    matches!(dlg, Dialog::DeletePermanently { count, local, .. } if local < count)
}

pub(crate) fn extra_button(ui: &mut egui::Ui, dlg: &mut Dialog, confirm: &mut bool) {
    if !has_extra(dlg) {
        return;
    }
    let b = extra(std::mem::discriminant(&*dlg));
    let Dialog::DeletePermanently { files, .. } = dlg else { return };
    if b.show(ui).clicked() {
        *files = "keep".into();
        *confirm = true;
    }
}

/// The dialog's confirm (Move to Trash, or Keep Files with `files` set to keep).
pub(crate) fn confirm(app: &mut LightcraftApp, dlg: &Dialog) -> Result<Value, String> {
    let Dialog::DeletePermanently { empty, files, .. } = dlg else { return Ok(Value::Null) };
    let id = if *empty { EMPTY } else { DELETE };
    // (the error is told in the toast: the dialog closes either way)
    let _ = run(app, id, json!({}), files);
    Ok(Value::Null)
}
