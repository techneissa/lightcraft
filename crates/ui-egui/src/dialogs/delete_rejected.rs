//! Photo ▸ Delete Rejected Photos (`photo.deleteRejected`): the rejected photos of the view on
//! screen go to Recently Deleted. Like Delete, it asks first while Settings ▸ General ▸ "Confirm
//! before moving photos to Recently Deleted" is on, and the question says how many.

use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::Dialog;
use crate::theme::Tokens;

const ID: &str = "photo.deleteRejected";

/// A menu, key or command-palette run of Delete Rejected Photos: with the confirmation on it opens
/// the dialog (nothing runs yet). `None`: run as usual (confirmation off, nothing rejected, or not
/// this command).
pub(crate) fn run_item(app: &mut LightcraftApp, id: &str) -> Option<Result<Value, String>> {
    if id != ID || !app.ui.settings.confirm_delete || app.session.source == lightcraft_engine::LibrarySource::RecentlyDeleted {
        return None;
    }
    let count = app.session.rejected_in_view().len();
    if count == 0 {
        return None;
    }
    app.ui.dialog = Some(Dialog::ConfirmDeleteRejected { count });
    Some(Ok(Value::Null))
}

/// The menu row is greyed out while the view on screen shows no rejected photo.
pub(crate) fn enabled(app: &LightcraftApp, id: &str) -> bool {
    id != ID || app.session.has_rejected_in_view()
}

pub(crate) fn body(ui: &mut egui::Ui, t: &Tokens, dlg: &Dialog) {
    let Dialog::ConfirmDeleteRejected { count } = dlg else { return };
    let what = if *count == 1 {
        crate::i18n::tr("Move the rejected photo in this view to Recently Deleted?").to_string()
    } else {
        crate::i18n::tr_format!("Move the {count} rejected photos in this view to Recently Deleted?", count = count)
    };
    ui.label(what);
    ui.label(egui::RichText::new(crate::i18n::tr("They can be restored from Recently Deleted until it is emptied.")).color(t.text_dim));
}

pub(crate) fn confirm(app: &mut LightcraftApp) -> Result<Value, String> {
    app.run(ID, json!({}))
}
