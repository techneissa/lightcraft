//! Use Smart Previews for viewing and editing (Settings ▸ Performance, `library.smartPreviewsForEditing`): the setting is
//! saved with the user's other settings (`ui.json`) and handed to the engine, which follows it from the first frame; a
//! command run by an agent changes it the other way. Switching it on asks once whether to build the library's missing
//! smart previews.

use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::Dialog;
use crate::theme::Tokens;

const ID: &str = "library.smartPreviewsForEditing";

/// Settings and engine agree on the setting: whichever changed since last time wins.
pub(crate) fn sync(app: &mut LightcraftApp) {
    let (saved, engine) = (app.ui.settings.smart_previews_for_editing, app.session.media.smart_editing());
    let before = app.smart_editing_applied;
    if before != Some(saved) {
        app.smart_editing_applied = Some(saved);
        let _ = app.session.execute(ID, &json!({"enabled": saved}));
        // switched on in Settings (not loaded at launch): ask once about the photos that have none
        if saved && before == Some(false) {
            offer(app);
        }
    } else if engine != saved {
        // an agent ran the command: the saved setting follows
        app.ui.settings.smart_previews_for_editing = engine;
        app.smart_editing_applied = Some(engine);
    }
}

/// Ask whether to build the smart previews the library lacks, the first time the setting is switched on and only if some
/// are missing. The question takes the Settings window's place (as a licence does).
fn offer(app: &mut LightcraftApp) {
    if app.ui.settings.smart_editing_offered {
        return;
    }
    let Ok(info) = app.run(ID, json!({})) else { return };
    let (photos, built) = (info["photos"].as_u64().unwrap_or(0), info["smartPreviews"].as_u64().unwrap_or(0));
    if !info["available"].as_bool().unwrap_or(false) || photos <= built {
        return;
    }
    app.ui.settings.smart_editing_offered = true;
    app.ui.dialog = Some(Dialog::SmartPreviewsForEditing { missing: (photos - built) as usize });
}

/// Build what is missing (a row in the activity stack).
pub(crate) fn build(app: &mut LightcraftApp) -> Result<Value, String> {
    app.run(ID, json!({"build": true}))
}

pub(crate) fn body(ui: &mut egui::Ui, t: &Tokens, missing: usize) {
    let what = if missing == 1 {
        crate::i18n::tr("1 photo has no smart preview yet.").to_string()
    } else {
        crate::i18n::tr_format!("{missing} photos have no smart preview yet.", missing = missing)
    };
    ui.label(what);
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "Build them now, in the background? A photo without one opens from its original until it has one. The Previews menu has Build Smart Previews for later.",
        ))
        .color(t.text_dim),
    );
}
