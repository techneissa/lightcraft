//! Quick delete: Delete (⌫) in Recently Deleted deletes for good, and Settings ▸ General ▸
//! "Only for picked or starred photos" lets Delete and Photo ▸ Delete Rejected Photos skip their
//! confirmation when no photo in the set is picked or has stars (Undo brings them back).
//! A UI-side decision where the confirm opens; the engine commands are unchanged.

use serde_json::{Value, json};

use lightcraft_catalog::{Flag, PhotoId};

use crate::LightcraftApp;

/// Delete in Recently Deleted: the photos are already deleted, so the key means Delete Permanently
/// (through the Delete Permanently setting and dialog). `None`: not this case.
pub(crate) fn run_item(app: &mut LightcraftApp, id: &str) -> Option<Result<Value, String>> {
    if id != "photo.delete" || app.session.source != lightcraft_engine::LibrarySource::RecentlyDeleted {
        return None;
    }
    Some(crate::menubar::run_item(app, "photo.deletePermanently", json!({})))
}

/// The Delete key in Recently Deleted; true when it was used.
pub(crate) fn delete_key(app: &mut LightcraftApp) -> bool {
    run_item(app, "photo.delete").is_some()
}

/// With the setting on, whether none of `ids` is picked or has stars (so no question is needed).
pub(crate) fn skips_confirm(app: &LightcraftApp, ids: &[PhotoId]) -> bool {
    app.ui.settings.confirm_delete_picked_only
        && !ids.is_empty()
        && ids.iter().all(|id| app.session.catalog.photo(*id).is_some_and(|p| p.flag != Flag::Pick && p.rating == 0))
}

/// Tell the user what happened, in the toast the app uses for undoable moves.
pub(crate) fn say_moved(app: &mut LightcraftApp, n: usize) {
    let text = crate::i18n::tr_format!("Moved {n} photo{} to Recently Deleted. Edit ▸ Undo brings them back.", if n == 1 { "" } else { "s" }, n = n);
    let until = app.tick_now() + 6.0;
    app.ui.toast = Some((text, until, None));
}

/// Delete without asking when the setting allows it for the selection; true when it ran.
pub(crate) fn delete_selection_now(app: &mut LightcraftApp) -> bool {
    let ids = app.session.selected();
    if !skips_confirm(app, &ids) {
        return false;
    }
    match app.run("photo.delete", json!({})) {
        Ok(_) => say_moved(app, ids.len()),
        Err(e) => {
            let until = app.tick_now() + 6.0;
            app.ui.toast = Some((e, until, None));
        }
    }
    true
}
