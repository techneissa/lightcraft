//! The Camera Matching choice of a raw in the Profile picker: "Camera Matching" (the default: LightCraft's colours
//! for the raw are matched to the camera's own JPEG) or "No Camera Matching" (the JPEG is never read). Stored as
//! `profile.camera_colour` (`auto` | `colorimetric`) and offered only for a raw whose starting colour is fitted to its
//! JPEG (`SourceInfo::colorimetric`).

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{CameraColour, DevelopSettings};

use crate::LightcraftApp;

/// The two choices, in picker order: the value, its label and a plain-words tooltip.
pub(crate) const CHOICES: [(CameraColour, &str, &str); 2] = [
    (CameraColour::Auto, "Camera Matching", "LightCraft matches this raw's colours to the camera's own picture. This is the default."),
    (
        CameraColour::Colorimetric,
        "No Camera Matching",
        "Skips the match and uses LightCraft's generic colours for this camera. The picture can look darker and flatter.",
    ),
];

/// Whether the choice is offered for photo `id` (a raw without colour matrices of its own).
pub(crate) fn offered(app: &LightcraftApp, id: PhotoId) -> bool {
    app.session.source_info(id).colorimetric
}

pub(crate) fn label(colour: CameraColour) -> &'static str {
    CHOICES.iter().find(|c| c.0 == colour).map_or("Camera Matching", |c| c.1)
}

/// `d` with `colour` as its Camera Matching choice.
pub(crate) fn with(d: &DevelopSettings, colour: CameraColour) -> DevelopSettings {
    let mut s = d.clone();
    s.profile.camera_colour = colour;
    s
}

/// The command value of `colour`.
pub(crate) fn value(colour: CameraColour) -> &'static str {
    if colour == CameraColour::Colorimetric { "colorimetric" } else { "auto" }
}
