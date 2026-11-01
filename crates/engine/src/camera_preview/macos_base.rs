//! The tone of a macOS Colour photo (`files::macos`): the Mac's decoder gives an upright linear Rec.2020 picture with the
//! camera's colours already in it, so only the tone and chroma curves are fitted to the camera JPEG, with the identity
//! colour model, as for a DNG's own colours (`fit_dng_tone`).
use super::colour::PROXY;
use super::look_fit::{FIT_PREVIEW_MAX_PIXELS, with_dro_off_tone};
use super::pairs::{Attempt, TONE_FITS, fit_attempt};
use super::proxy::reference_at;
use super::tone::{HighlightCheck, highlights};
use super::{CameraLook, Filter, Mat3, Rgb32f, fit};

/// The camera JPEG of a raw, small (the fit works on a [`PROXY`]-pixel picture), in the raw's sensor orientation, and
/// whether the camera rendered it with DRO. Decoded before the Mac's decoder runs, so the file can be let go of.
pub(crate) struct BaseReference {
    image: Rgb32f,
    dro: bool,
}

/// The [`BaseReference`] of the raw in `bytes`; `None` without a decodable camera JPEG.
pub(crate) fn base_reference(bytes: &[u8]) -> Option<BaseReference> {
    let edge = (2 * PROXY).max(384) as u32;
    let options = lightcraft_codecs::DecodeOptions { max_size: Some((edge, edge)), max_pixels: FIT_PREVIEW_MAX_PIXELS };
    let decoded = crate::files::decode_raw_preview(bytes, options, None)?;
    let dro = lightcraft_raw::embedded_preview_dynamic_range_optimized(bytes) == Some(true);
    Some(BaseReference { image: decoded.to_working(), dro })
}

/// The look (identity matrix, fitted tone and chroma curves) of `base`, an upright linear Rec.2020 picture of a raw
/// whose EXIF orientation is `orientation`, fitted to its camera JPEG, with the same gates as every other fit, lowered
/// to Sony's tone without DRO when the camera used it. `None` (the fit is refused) when the JPEG does not show the same
/// frame or no attempt passes the gates.
pub(crate) fn fit_base_tone(reference: &BaseReference, orientation: lightcraft_geom::Orientation, base: &Rgb32f) -> Option<CameraLook> {
    let upright = reference.image.clone().into_oriented(orientation);
    let (bw, bh) = (base.width as f64, base.height as f64);
    if bw < 16.0 || bh < 16.0 || upright.width == 0 || upright.height == 0 {
        return None;
    }
    if (upright.width as f64 / upright.height as f64 / (bw / bh) - 1.0).abs() > 0.02 {
        return None;
    }
    let sensor = fit(base, PROXY, PROXY, Filter::Box);
    let upright = reference_at(&upright, &sensor);
    let bright = HighlightCheck { pairs: highlights(&sensor, &upright), dro: reference.dro, grey: false };
    let colour = Some((Mat3::IDENTITY, None));
    let look = TONE_FITS.iter().find_map(|&tone| {
        [false, true].into_iter().find_map(|away_from_edges| {
            fit_attempt(&sensor, &upright, &colour, Attempt { tone, profile: true, away_from_edges, dark: true }, &bright)
        })
    })?;
    Some(if reference.dro { with_dro_off_tone(look) } else { look })
}

#[cfg(test)]
mod tests;
