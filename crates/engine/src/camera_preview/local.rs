//! The Profile picker's local camera profiles ([`CameraColour::Local`]): the colour of a raw whose photo chose one of the
//! user's own profile files (`camera_profiles::local_profiles`). The profile's matrix and hue/saturation table are
//! the base colour, as for a camera profile in Camera Matching; only the tone and chroma curves are fitted to the
//! file's JPEG. Unlike Camera Matching there is no retry with the photo's own colour, and no partial look: that is the
//! point of a separate choice. A missing, unusable or other-camera file gives Camera Matching, with a note.
use super::colour::{PROXY, colour_choice_offered, starting_colour};
use super::look_fit::{with_dro_off_tone, without_clipped};
use super::mono::{fit_tone_only_with, monochrome};
use super::pairs::{Attempt, TONE_FITS, fit_attempt};
use super::proxy::{LensGeometry, proxies};
use super::tone::{HighlightCheck, highlights};
use super::{CameraLook, CameraTone, CameraTransform, HsvTable, Mat3, RawImage, Rgb32f, Xy};
use crate::camera_profiles::LocalUnavailable;
use lightcraft_develop::CameraColour;

/// What the local choice made of the photo, besides the look.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LocalOutcome {
    /// The profile's colour is kept, but no tone fit passed: the photo has LightCraft's default tone curve (the
    /// look's own tone is a placeholder).
    pub default_tone: bool,
    /// Why the photo renders as Camera Matching although it chose a local profile (permanent note under the picker).
    pub note: Option<String>,
}

/// [`starting_colour`] for the photo's Profile choice: a local profile ([`CameraColour::Local`] with the key of its
/// file, `camera_profiles::local_key`) where the raw is offered the choice, else exactly [`starting_colour`].
pub(crate) fn starting_colour_choice(
    raw: &mut RawImage,
    bytes: &[u8],
    lens: Option<&LensGeometry>,
    embedded: Option<&lightcraft_develop::EmbeddedLens>,
    colour: CameraColour,
    profile_key: u64,
) -> (Xy, CameraTransform, Option<CameraLook>, LocalOutcome) {
    if colour == CameraColour::Local && colour_choice_offered(raw) {
        let local = local_colour_with(
            raw,
            profile_key,
            |raw, t, colour| fit_local_look(raw, bytes, t, lens, colour),
            |raw| starting_colour(raw, bytes, lens, embedded, CameraColour::Auto),
        );
        if let Some(local) = local {
            return local;
        }
    }
    let (xy, t, look) = starting_colour(raw, bytes, lens, embedded, colour);
    (xy, t, look, LocalOutcome::default())
}

/// [`starting_colour_choice`] for a local profile, with the tone fit and Camera Matching passed in (tests pin the order
/// with them). `None`: the file has colour matrices of its own, so the choice does not apply.
pub(super) fn local_colour_with(
    raw: &mut RawImage,
    profile_key: u64,
    fit: impl FnOnce(&RawImage, &CameraTransform, &(Mat3, Option<HsvTable>)) -> Option<CameraLook>,
    camera_matching: impl FnOnce(&mut RawImage) -> (Xy, CameraTransform, Option<CameraLook>),
) -> Option<(Xy, CameraTransform, Option<CameraLook>, LocalOutcome)> {
    let xy = lightcraft_raw::color::as_shot_white_xy(raw);
    let t = lightcraft_raw::color::camera_transform(raw, xy);
    if !t.matrix_is_fallback {
        return None;
    }
    let model = raw.metadata.model.as_deref().map(str::trim).unwrap_or_default().to_owned();
    let unavailable = |raw: &mut RawImage, id: &str, why: LocalUnavailable| {
        let (xy, t, look) = camera_matching(raw);
        (xy, t, look, LocalOutcome { default_tone: false, note: Some(why.note(id)) })
    };
    let (profile_id, profile) = match crate::camera_profiles::resolve_local(profile_key, &model) {
        Ok(found) => found,
        Err((id, why)) => return Some(unavailable(raw, &id, why)),
    };
    // (the profile's matrix is "white-balanced camera RGB → Rec.2020", the look's is relative to the camera transform)
    let Some(colour) = t.matrix.inverse().map(|inverse| (profile.matrix().mul(&inverse), profile.hue_sat.clone())) else {
        return Some(unavailable(raw, &profile_id, LocalUnavailable::Invalid));
    };
    let outcome = |default_tone| LocalOutcome { default_tone, note: None };
    if let Some(look) = fit(raw, &t, &colour) {
        return Some((xy, t, Some(look), outcome(false)));
    }
    // no tone fit passed (no camera JPEG, a different frame, a scene too dull): the profile's colour with the default tone
    if lightcraft_pipeline::profiling() {
        eprintln!("[profile] {:?} local camera profile {}: no tone fit passed, default tone", raw.format, profile.model);
    }
    // (the placeholder is strictly increasing and below 1 by construction, so this always holds)
    let Some(tone) = placeholder_tone() else { return Some(unavailable(raw, &profile_id, LocalUnavailable::Invalid)) };
    let look = CameraLook { matrix: colour.0, tone, hue_sat: colour.1 };
    Some((xy, t, Some(look), outcome(true)))
}

/// A valid curve for a look that keeps LightCraft's default tone: never rendered (`SourceInfo::camera_tone` is `None`).
fn placeholder_tone() -> Option<CameraTone> {
    let knots = std::array::from_fn(|i| {
        let x = 0.004 * 1.3f32.powi(i as i32);
        [x, x / (1.0 + x)]
    });
    CameraTone::new(knots)
}

/// The look for the local profile's `colour`: the camera JPEG's tone and chroma curves fitted around it. Sony's Dynamic
/// Range Optimizer is lowered out of the tone as in Camera Matching.
fn fit_local_look(
    raw: &RawImage,
    bytes: &[u8],
    transform: &CameraTransform,
    lens: Option<&LensGeometry>,
    colour: &(Mat3, Option<HsvTable>),
) -> Option<CameraLook> {
    let (sensor, reference, clipped) = proxies(raw, bytes, transform, PROXY, 0.0, lens)?;
    let dro = lightcraft_raw::embedded_preview_dynamic_range_optimized(bytes) == Some(true);
    let look = fit_local_look_with(&sensor, &reference, &clipped, colour, dro)?;
    Some(if dro { with_dro_off_tone(look) } else { look })
}

/// The profile-colour attempts of Camera Matching's search and nothing else: per tone fit, all pixels then the ones
/// away from edges, then the same without the clipped pixels; a black-and-white JPEG gives its tone alone. The colour
/// in the look is always `colour`.
pub(super) fn fit_local_look_with(
    sensor: &Rgb32f,
    reference: &Rgb32f,
    clipped: &[bool],
    colour: &(Mat3, Option<HsvTable>),
    dro: bool,
) -> Option<CameraLook> {
    let given = Some(colour.clone());
    let unclipped = without_clipped(sensor, clipped);
    let bright = HighlightCheck { pairs: highlights(sensor, reference), dro, grey: false };
    let search = |on: &Rgb32f| {
        TONE_FITS.iter().find_map(|&tone| {
            [false, true].into_iter().find_map(|away_from_edges| {
                let attempt = Attempt { tone, profile: true, away_from_edges, dark: false };
                fit_attempt(on, reference, &given, attempt, &bright).map(|look| (look, attempt))
            })
        })
    };
    if let Some((look, attempt)) = search(sensor) {
        // refitted without the clipped pixels on the attempt that passed; the fit on all pixels when that misses the gates
        return Some(unclipped.as_ref().and_then(|u| fit_attempt(u, reference, &given, attempt, &bright)).unwrap_or(look));
    }
    if let Some((look, _)) = unclipped.as_ref().and_then(search) {
        return Some(look);
    }
    // a black-and-white JPEG has no colour to match: the profile's colour and the JPEG's tone curve
    if monochrome(reference) {
        let grey = HighlightCheck { pairs: highlights(sensor, reference), dro, grey: true };
        return fit_tone_only_with(unclipped.as_ref().unwrap_or(sensor), reference, given, &grey);
    }
    None
}
