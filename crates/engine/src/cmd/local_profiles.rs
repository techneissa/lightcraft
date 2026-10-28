//! Photos that a local camera profile used to change inside Camera Matching, and the one-time offer to keep their look.
//!
//! Before local profiles were choices of their own, a file `<model>.json` in the camera-profiles folder silently replaced
//! the built-in profile of that camera inside Camera Matching, so every photo of the camera on Camera Matching was
//! rendered with it. Camera Matching no longer reads the folder, so those photos change. For each such file, once per
//! library, the app asks whether to move the photos to the file's own Profile choice ([`offer`]); never automatically.

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::CameraColour;
use serde_json::json;

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::Session;

/// A profile file whose camera's photos looked different with it, and those photos.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    /// The file name without `.json`.
    pub id: String,
    /// What the Profile picker calls it.
    pub name: String,
    pub photos: Vec<PhotoId>,
}

/// The file stem builds before local profiles read for camera `model` (`<model>.json`, `camera_profiles::file_name`).
fn legacy_stem(model: &str) -> String {
    crate::camera_profiles::file_name(model).trim_end_matches(".json").to_owned()
}

/// The photos of camera `profile` is made for that are on Camera Matching.
fn on_camera_matching(s: &Session, profile: &crate::camera_profiles::LocalProfile) -> Vec<PhotoId> {
    s.catalog
        .photos()
        .filter(|p| !p.deleted && p.fitted_look() && p.develop.profile.camera_colour == CameraColour::Auto && profile.is_for_camera(&p.meta.camera))
        .map(|p| p.id)
        .collect()
}

/// The first profile file that changed the look of this library's photos and has not been asked about yet. Only a file
/// named after its camera's model (`ILCE-7M5.json`) was ever in effect inside Camera Matching.
pub fn offer(s: &Session) -> Option<Offer> {
    crate::camera_profiles::local_profiles().into_iter().find_map(|l| {
        if !l.valid() || s.local_profile_asked.contains(&l.id) || l.id != legacy_stem(&l.model) {
            return None;
        }
        let photos = on_camera_matching(s, &l);
        (!photos.is_empty()).then(|| Offer { id: l.id.clone(), name: l.name.clone(), photos })
    })
}

fn ask_done(s: &mut Session, id: &str) {
    if !s.local_profile_asked.iter().any(|x| x == id) {
        s.local_profile_asked.push(id.to_owned());
        if let Err(e) = s.save_prefs() {
            log::warn!("local camera profiles: {e}");
        }
    }
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "profiles.localOffer", "Local Camera Profile Offer", [], None, "{} — {offer: null | {id, name, photos: count}}: a profile file that changed the look of this library's photos inside Camera Matching (a file named after its camera, such as ILCE-7M5.json) and has not been asked about; the app asks once to move them to it (`profiles.localSwitch`)", always, |s, _| {
            Ok(json!({"offer": offer(s).map(|o| json!({"id": o.id, "name": o.name, "photos": o.photos.len()}))}))
        }),
        cmd!(
            "profiles.localAsked",
            "Local Camera Profile Asked",
            [],
            None,
            "{id} — remember that this library has been asked about profile file `id` (it is not offered again)",
            always,
            |s, p| {
                let id = str_param(p, "id").ok_or_else(|| bad("profiles.localAsked", "missing id"))?.to_string();
                ask_done(s, &id);
                Ok(json!({}))
            }
        ),
        cmd!(
            "profiles.localSwitch",
            "Switch Photos to Local Profile",
            [],
            None,
            "{id} — every photo of the profile's camera that is on Camera Matching goes to the local profile `id`, in one undo step → {changed}",
            always,
            |s, p| {
                let c = "profiles.localSwitch";
                let id = str_param(p, "id").ok_or_else(|| bad(c, "missing id"))?.to_string();
                let profile = crate::camera_profiles::local_profile(&id)
                    .filter(|l| l.valid())
                    .ok_or_else(|| bad(c, format!("no usable local profile `{id}`")))?;
                let label = format!("Profile: {}", profile.name);
                let ops: Vec<Op> = on_camera_matching(s, &profile)
                    .into_iter()
                    .filter_map(|photo| s.develop_of(photo).map(|d| (photo, d)))
                    .filter_map(|(photo, d)| {
                        let mut d = (*d).clone();
                        d.profile.camera_colour = CameraColour::Local;
                        d.profile.camera_profile = Some(id.clone());
                        s.develop_op(photo, d, &label)
                    })
                    .collect();
                let changed = ops.len();
                if changed > 0 {
                    s.commit(&label, Op::Batch { ops })?;
                }
                ask_done(s, &id);
                Ok(json!({"changed": changed}))
            }
        ),
    ]
}
