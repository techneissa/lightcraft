//! The colour source of a raw in the Profile picker: "Camera Matching" (the default: LightCraft's colours for the raw
//! are matched to the camera's own JPEG), "No Camera Matching" (the JPEG is never read), one row per local camera
//! profile made for the photo's camera (the user's own files in the camera-profiles folder, named from the file), and
//! macOS Colour where this Mac's raw decoder reads the camera ([`crate::macos_colour::picker`]).
//! Stored as `profile.camera_colour` (`auto` | `colorimetric` | `local` | `macos`, with `profile.camera_profile` naming the file)
//! and offered only for a raw whose starting colour is fitted to its JPEG (`SourceInfo::colorimetric`).

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{CameraColour, DevelopSettings};
use serde_json::json;

use crate::LightcraftApp;

/// The two built-in choices, in picker order: the value, its label and a plain-words tooltip.
pub(crate) const CHOICES: [(CameraColour, &str, &str); 2] = [
    (CameraColour::Auto, "Camera Matching", "LightCraft matches this raw's colours to the camera's own picture. This is the default."),
    (
        CameraColour::Colorimetric,
        "No Camera Matching",
        "Skips the match and uses LightCraft's generic colours for this camera. The picture can look darker and flatter.",
    ),
];

/// One row of the picker: a built-in choice or a local profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub colour: CameraColour,
    /// The local profile's id (its file name without `.json`); `None` for the built-in choices.
    pub profile: Option<String>,
    pub label: String,
    pub tip: String,
    /// A profile the photo stores but this computer cannot use for it: shown, greyed, and not clickable.
    pub enabled: bool,
}

impl Row {
    /// Identifies the row in widget ids: `auto`, `colorimetric` or `local:<id>`.
    pub(crate) fn key(&self) -> String {
        match (&self.colour, &self.profile) {
            (CameraColour::Local, Some(id)) => format!("local:{id}"),
            (colour, _) => value(*colour).to_string(),
        }
    }

    /// Whether the photo's settings are on this row.
    pub(crate) fn applied(&self, d: &DevelopSettings) -> bool {
        d.profile.camera_colour == self.colour && (self.colour != CameraColour::Local || d.profile.camera_profile == self.profile)
    }

    /// `d` with this row chosen.
    pub(crate) fn with(&self, d: &DevelopSettings) -> DevelopSettings {
        let mut s = with(d, self.colour);
        s.profile.camera_profile.clone_from(&self.profile);
        s
    }

    /// The `develop.profile` parameters that choose this row.
    pub(crate) fn params(&self) -> serde_json::Value {
        match &self.profile {
            Some(id) if self.colour == CameraColour::Local => json!({"camera_colour": "local", "camera_profile": id}),
            _ => json!({"camera_colour": value(self.colour)}),
        }
    }
}

/// The rows of photo `id`'s picker: the built-in choices, the local profiles made for its camera, and, when the photo
/// stores a profile that is not among them (a missing, unusable or other-camera file), that one greyed.
pub(crate) fn rows(app: &LightcraftApp, id: PhotoId, d: &DevelopSettings) -> Vec<Row> {
    let mut rows: Vec<Row> = CHOICES
        .iter()
        .map(|(colour, label, tip)| Row { colour: *colour, profile: None, label: (*label).into(), tip: (*tip).into(), enabled: true })
        .collect();
    rows.extend(crate::macos_colour::picker::row(app, id, d));
    let camera = app.session.catalog.photo(id).map(|p| p.meta.camera.clone()).unwrap_or_default();
    for local in lightcraft_engine::camera_profiles::local_profiles_for_camera(&camera) {
        rows.push(Row {
            colour: CameraColour::Local,
            profile: Some(local.id.clone()),
            label: local.name.clone(),
            tip: crate::i18n::tr_format!(
                "Your own profile, from {file}.json. Its colours are the base; the tone follows the camera's picture.",
                file = local.id
            ),
            enabled: true,
        });
    }
    if d.profile.camera_colour == CameraColour::Local
        && let Some(stored) = &d.profile.camera_profile
        && !rows.iter().any(|r| r.profile.as_ref() == Some(stored))
    {
        rows.push(Row {
            colour: CameraColour::Local,
            profile: Some(stored.clone()),
            label: stored.clone(),
            tip: crate::i18n::tr("This profile isn't available for this camera on this computer, so the picture uses Camera Matching.").to_string(),
            enabled: false,
        });
    }
    rows
}

/// Whether the choice is offered for photo `id` (a raw without colour matrices of its own).
pub(crate) fn offered(app: &LightcraftApp, id: PhotoId) -> bool {
    app.session.source_info(id).colorimetric
}

/// The row the photo's settings are on, for the Edit panel's one-line summary.
pub(crate) fn current(app: &LightcraftApp, id: PhotoId, d: &DevelopSettings) -> Row {
    let all = rows(app, id, d);
    all.iter().find(|r| r.applied(d)).or_else(|| all.first()).cloned().unwrap_or(Row {
        colour: CameraColour::Auto,
        profile: None,
        label: CHOICES[0].1.into(),
        tip: CHOICES[0].2.into(),
        enabled: true,
    })
}

/// `d` with `colour` as its colour source (no local profile).
pub(crate) fn with(d: &DevelopSettings, colour: CameraColour) -> DevelopSettings {
    let mut s = d.clone();
    s.profile.camera_colour = colour;
    s.profile.camera_profile = None;
    s
}

/// The command value of `colour`.
pub(crate) fn value(colour: CameraColour) -> &'static str {
    match colour {
        CameraColour::Colorimetric => "colorimetric",
        CameraColour::Local => "local",
        CameraColour::Macos => "macos",
        CameraColour::Auto => "auto",
    }
}
