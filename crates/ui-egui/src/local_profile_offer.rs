//! The one-time question about a local camera profile file (`profiles.localOffer`): a file named after its camera
//! (`ILCE-7M5.json`) used to change that camera's photos inside Camera Matching, which no longer reads the folder.
//! Asked once per file and library, when the library opens; never answered by itself.

use serde_json::json;

use crate::LightcraftApp;
use crate::state::Dialog;

/// Ask about the first profile file that changed this library's photos and has not been asked about. Nothing is asked
/// while another dialog is open (the next launch asks); asking is remembered as the question is put.
pub(crate) fn check(app: &mut LightcraftApp) {
    if app.ui.dialog.is_some() {
        return;
    }
    let Ok(answer) = app.run("profiles.localOffer", json!({})) else { return };
    let Some(offer) = answer.get("offer").filter(|o| o.is_object()) else { return };
    let (Some(id), Some(name)) = (offer["id"].as_str(), offer["name"].as_str()) else { return };
    let count = offer["photos"].as_u64().unwrap_or(0) as usize;
    if app.run("profiles.localAsked", json!({"id": id})).is_err() {
        return;
    }
    app.ui.dialog = Some(Dialog::LocalProfileSwitch { id: id.to_string(), name: name.to_string(), count });
}
