//! The one-time question about a local camera profile file (`profiles.localOffer`): a file named after its camera
//! (`ILCE-7M5.json`) used to change that camera's photos inside Camera Matching, which no longer reads the folder.
//! Asked when the library opens, until it is answered: Switch (`profiles.localSwitch`) or Leave on Camera Matching
//! (`profiles.localAsked`) remember the file; closing the window another way (Esc, a click outside) leaves the
//! question for the next launch. Never answered by itself.

use serde_json::json;

use crate::LightcraftApp;
use crate::state::Dialog;

/// Ask about the first profile file that changed this library's photos and has not been asked about. Nothing is asked
/// while another dialog is open (the next launch asks); putting the question remembers nothing.
pub(crate) fn check(app: &mut LightcraftApp) {
    if app.ui.dialog.is_some() {
        return;
    }
    let Ok(answer) = app.run("profiles.localOffer", json!({})) else { return };
    let Some(offer) = answer.get("offer").filter(|o| o.is_object()) else { return };
    let (Some(id), Some(name)) = (offer["id"].as_str(), offer["name"].as_str()) else { return };
    let count = offer["photos"].as_u64().unwrap_or(0) as usize;
    app.ui.dialog = Some(Dialog::LocalProfileSwitch { id: id.to_string(), name: name.to_string(), count });
}
