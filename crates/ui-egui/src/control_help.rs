//! What a slider does, in one short sentence: the text its hover says after its name (`Exposure: brightens or
//! darkens the whole photo.`). One line per control id, so a panel never words its own. A control without a line
//! shows only its name.
//!
//! The lines use plain words, no dashes. Ids are the develop control ids (`light.exposure`), the template ids of the
//! indexed controls (`pointColor.hueShift`, `redEye.darken`), the Portrait ids (`retouch.skin.smoothness`) and the
//! ids the panels give their own sliders (a mask's local adjustments are bare: `exposure`, `amount`).

use lightcraft_develop::ControlSpec;

/// The line saying what `spec`'s control does; `None` when it has none.
pub fn control_help(spec: &ControlSpec) -> Option<&'static str> {
    // Two mask sliders share the id `refine`: Refine Edges (the mask's) and Refine (a colour range's).
    if spec.id == "refine" {
        return Some(if spec.label == "Refine" {
            "widens or narrows the range of colors picked."
        } else {
            "snaps the mask edges to the edges in the photo."
        });
    }
    develop(spec.id).or_else(|| mask(spec.id)).or_else(|| portrait(spec.id)).or_else(|| tools(spec.id))
}

/// The hover text of a slider: its name, then what it does.
pub fn hover_text(spec: &ControlSpec, label: &str) -> String {
    match control_help(spec) {
        Some(help) => crate::i18n::tr_format!("{label}: {help}", label = label, help = crate::i18n::tr(help)),
        None => label.to_string(),
    }
}

/// The hover text of a curve channel button.
pub fn curve_channel_help(channel: &str) -> &'static str {
    match channel {
        "parametric" => "Parametric curve: shapes the tones with the Highlights, Lights, Darks and Shadows sliders.",
        "master" => "Point curve: shapes the tones of all colors by dragging points on the curve.",
        "red" => "Red channel: shapes only the red of the photo with its own curve.",
        "green" => "Green channel: shapes only the green of the photo with its own curve.",
        _ => "Blue channel: shapes only the blue of the photo with its own curve.",
    }
}

/// The Edit panel's develop controls.
fn develop(id: &str) -> Option<&'static str> {
    Some(match id {
        "profile.amount" => "sets how strongly the profile's colors and tones are applied.",
        "look.amount" => "sets how strongly the look is applied.",
        "wb.temp" | "wb.tempRel" => "shifts the color balance between cool blue and warm yellow.",
        "wb.tint" | "wb.tintRel" => "shifts the color balance between green and magenta.",
        "light.exposure" => "brightens or darkens the whole photo.",
        "light.contrast" => "widens or narrows the gap between light and dark tones.",
        "light.highlights" => "darkens or brightens the bright areas.",
        "light.shadows" => "lightens or deepens the dark areas.",
        "light.whites" => "moves the white point, changing the very brightest tones.",
        "light.blacks" => "moves the black point, changing the very darkest tones.",
        "hdr.maxEv" => "sets how many stops above standard white the HDR version may reach.",
        "hdr.sdrBrightness" => "brightens or darkens the standard (SDR) version of an HDR photo.",
        "hdr.sdrContrast" => "changes the contrast of the SDR version of an HDR photo.",
        "hdr.sdrHighlights" => "darkens or brightens the bright areas of the SDR version.",
        "hdr.sdrShadows" => "lightens or deepens the dark areas of the SDR version.",
        "hdr.sdrWhites" => "moves the white point of the SDR version.",
        "hdr.sdrClarity" => "adds or softens contrast in midtone detail of the SDR version.",
        "curve.highlights" => "raises or lowers the curve in its brightest region.",
        "curve.lights" => "raises or lowers the curve in the upper midtones.",
        "curve.darks" => "raises or lowers the curve in the lower midtones.",
        "curve.shadows" => "raises or lowers the curve in its darkest region.",
        "curve.splitShadows" => "sets where the Shadows and Darks regions of the curve meet.",
        "curve.splitMid" => "sets where the Darks and Lights regions of the curve meet.",
        "curve.splitHighlights" => "sets where the Lights and Highlights regions of the curve meet.",
        "curve.refineSaturation" => "pulls back the extra color strength a strong curve adds.",
        "color.vibrance" => "boosts muted colors more than colors that are already strong.",
        "color.saturation" => "makes all colors more or less intense.",
        "grading.shadows.hue" => "sets the color of the tint added to the shadows.",
        "grading.shadows.sat" => "sets how strong the shadow tint is.",
        "grading.shadows.lum" => "lightens or darkens the shadows.",
        "grading.midtones.hue" => "sets the color of the tint added to the midtones.",
        "grading.midtones.sat" => "sets how strong the midtone tint is.",
        "grading.midtones.lum" => "lightens or darkens the midtones.",
        "grading.highlights.hue" => "sets the color of the tint added to the highlights.",
        "grading.highlights.sat" => "sets how strong the highlight tint is.",
        "grading.highlights.lum" => "lightens or darkens the highlights.",
        "grading.global.hue" => "sets the color of a tint added to the whole photo.",
        "grading.global.sat" => "sets how strong the overall tint is.",
        "grading.global.lum" => "lightens or darkens the whole photo.",
        "grading.blending" => "sets how much the shadow, midtone and highlight tints overlap.",
        "grading.balance" => "shifts the weight between the shadow tint and the highlight tint.",
        "effects.texture" => "enhances or smooths fine surface detail.",
        "effects.clarity" => "adds or softens contrast in midtone detail.",
        "effects.dehaze" => "removes or adds haze.",
        "vignette.amount" => "darkens or lightens the corners of the photo.",
        "vignette.midpoint" => "sets how far the vignette reaches toward the center.",
        "vignette.roundness" => "makes the vignette more square or more round.",
        "vignette.feather" => "sets how soft the vignette edge is.",
        "vignette.highlights" => "keeps bright areas from darkening as much under the vignette.",
        "grain.amount" => "adds film grain.",
        "grain.size" => "sets how big the grain specks are.",
        "grain.roughness" => "sets how uneven the grain looks.",
        "reflection.detailProtection" => "protects fine detail from the reflection removal.",
        "reflection.left" => "sets the left edge of the area reflection removal works on.",
        "reflection.top" => "sets the top edge of the area reflection removal works on.",
        "reflection.right" => "sets the right edge of the area reflection removal works on.",
        "reflection.bottom" => "sets the bottom edge of the area reflection removal works on.",
        "reflection.amount" => "sets how much of the reflection removal shows.",
        "reflection.threshold" => "sets how bright a spot must be to count as glare.",
        "reflection.expand" => "grows the glare that was found before it is filled.",
        "enhance.denoise" => "sets how strongly AI noise removal is applied.",
        "detail.sharpenAmount" => "sharpens edges and fine detail.",
        "detail.sharpenRadius" => "sets how far sharpening reaches around edges.",
        "detail.sharpenDetail" => "sets how much fine detail sharpening brings out.",
        "detail.sharpenMasking" => "limits sharpening to edges and leaves flat areas smooth.",
        "detail.nrLuminance" => "reduces grainy noise in brightness.",
        "detail.nrDetail" => "sets how much fine detail noise reduction keeps.",
        "detail.nrContrast" => "sets how much local contrast noise reduction keeps.",
        "detail.nrColor" => "reduces blotches of color noise.",
        "detail.nrColorDetail" => "sets how much fine color detail color noise reduction keeps.",
        "detail.nrColorSmoothness" => "evens out larger color blotches left after color noise reduction.",
        "optics.distortion" => "corrects barrel or pincushion bending of straight lines.",
        "optics.vignetting" => "brightens or darkens the corners to fix lens falloff.",
        "optics.vignettingMidpoint" => "sets how far from the corners the falloff fix reaches.",
        "optics.profileDistortion" => "sets how much of the lens profile's distortion fix is applied.",
        "optics.profileVignetting" => "sets how much of the lens profile's falloff fix is applied.",
        "optics.defringePurple" => "removes purple fringes along high contrast edges.",
        "optics.defringePurpleHueLo" => "sets the lowest purple hue treated as a fringe.",
        "optics.defringePurpleHueHi" => "sets the highest purple hue treated as a fringe.",
        "optics.defringeGreen" => "removes green fringes along high contrast edges.",
        "optics.defringeGreenHueLo" => "sets the lowest green hue treated as a fringe.",
        "optics.defringeGreenHueHi" => "sets the highest green hue treated as a fringe.",
        "optics.caRed" => "lines up red and cyan edge fringes caused by the lens.",
        "optics.caBlue" => "lines up blue and yellow edge fringes caused by the lens.",
        "geometry.vertical" => "corrects vertical perspective, as if tilting the camera up or down.",
        "geometry.horizontal" => "corrects horizontal perspective, as if turning the camera sideways.",
        "geometry.rotate" => "rotates the photo by a few degrees.",
        "geometry.aspect" => "stretches the photo wider or taller.",
        "geometry.scale" => "zooms the photo in or out.",
        "geometry.offsetX" => "moves the photo left or right in the frame.",
        "geometry.offsetY" => "moves the photo up or down in the frame.",
        "crop.angle" => "rotates the photo to level the horizon.",
        "lensBlur.amount" | "lens_blur" => "sets how strongly out of focus areas are blurred.",
        "lensBlur.boost" | "lens_blur_boost" => "turns bright out of focus highlights into brighter bokeh discs.",
        "lensBlur.transition" | "lens_blur_transition" => "sets how gradually the blur grows away from the area in focus.",
        "calibration.shadowsTint" => "shifts the dark areas between green and magenta.",
        "calibration.redHue" => "shifts the hue of the red primary.",
        "calibration.redSat" => "changes the saturation of the red primary.",
        "calibration.greenHue" => "shifts the hue of the green primary.",
        "calibration.greenSat" => "changes the saturation of the green primary.",
        "calibration.blueHue" => "shifts the hue of the blue primary.",
        "calibration.blueSat" => "changes the saturation of the blue primary.",
        "pointColor.hueShift" => "shifts the hue of the picked color.",
        "pointColor.satShift" => "makes the picked color more or less intense.",
        "pointColor.lumShift" => "makes the picked color lighter or darker.",
        "pointColor.variance" => "narrows or widens the spread of colors inside the range.",
        "pointColor.range" => "widens or narrows the whole range of colors picked.",
        "pointColor.hueRange" => "widens or narrows the range of hues picked.",
        "pointColor.satRange" => "widens or narrows the range of saturations picked.",
        "pointColor.lumRange" => "widens or narrows the range of brightness picked.",
        "redEye.pupilSize" => "sets how large the corrected pupil is.",
        "redEye.darken" => "sets how dark the corrected pupil is.",
        _ => return mixer(id),
    })
}

/// The Color Mixer and B&W Mixer bands (`mixer.<color>.<hue|sat|lum>`, `bw.<color>`).
fn mixer(id: &str) -> Option<&'static str> {
    if id.starts_with("mixer.") {
        return match id.rsplit('.').next()? {
            "hue" => Some("shifts this color toward its neighboring hues."),
            "sat" => Some("makes this color more or less intense."),
            "lum" => Some("makes this color lighter or darker."),
            _ => None,
        };
    }
    id.starts_with("bw.").then_some("makes this color lighter or darker in the black and white photo.")
}

/// A mask's local adjustments and its other sliders (ids as the Masking panel names them).
fn mask(id: &str) -> Option<&'static str> {
    Some(match id {
        "temp" => "shifts the masked area between cool blue and warm yellow.",
        "tint" => "shifts the masked area between green and magenta.",
        "exposure" => "brightens or darkens the masked area.",
        "contrast" => "widens or narrows the gap between light and dark in the masked area.",
        "highlights" => "darkens or brightens the bright parts of the masked area.",
        "shadows" => "lightens or deepens the dark parts of the masked area.",
        "whites" => "moves the white point of the masked area.",
        "blacks" => "moves the black point of the masked area.",
        "texture" => "enhances or smooths fine surface detail in the masked area.",
        "clarity" => "adds or softens contrast in midtone detail in the masked area.",
        "dehaze" => "removes or adds haze in the masked area.",
        "hue" => "shifts the colors of the masked area around the color wheel.",
        "saturation" => "makes the colors of the masked area more or less intense.",
        "sharpness" => "sharpens or softens detail in the masked area.",
        "noise" => "changes the noise in the masked area.",
        "moire" => "reduces moiré patterns in the masked area.",
        "defringe" => "removes color fringes along edges in the masked area.",
        "amount" => "scales the strength of every adjustment on this mask.",
        "decontaminate" => "stops the color behind the mask from bleeding into its edges.",
        "recolor" => "sets how strongly the masked area takes the chosen color.",
        "recolorRange" => "widens or narrows which colors count as the area's main color.",
        "smoothness" => "sets how softly the selected range fades out at its edges.",
        "edge" => "makes the selection border crisper or softer.",
        "ui.maskOverlayOpacity" => "sets how strongly the mask overlay shows over the photo.",
        _ => return None,
    })
}

/// The Portrait panel's sliders (`retouch.<feature>.<field>`).
fn portrait(id: &str) -> Option<&'static str> {
    Some(match id {
        "retouch.skin.smoothness" => "evens out bumps and blotches in the skin and keeps its pores.",
        "retouch.skin.texture" => "strengthens or softens the fine texture of the skin.",
        "retouch.skin.shine" => "tones down oily highlights on the skin.",
        "retouch.skinTone.evenTone" => "evens out blotchy skin tone.",
        "retouch.skinTone.redness" => "calms red skin.",
        "retouch.skinTone.faceLight" => "lightens or darkens the face against the rest of the photo.",
        "retouch.blemish.amount" => "removes spots, starting with the most visible ones.",
        "retouch.wrinkles.forehead" => "softens lines on the forehead.",
        "retouch.wrinkles.crowsFeet" => "softens lines at the outer corners of the eyes.",
        "retouch.wrinkles.smileLines" => "softens lines on the cheeks and around the mouth.",
        "retouch.wrinkles.neck" => "softens lines on the neck.",
        "retouch.eyes.whites" => "brightens the whites of the eyes.",
        "retouch.eyes.iris" => "brings out the pattern and color of the iris.",
        "retouch.eyes.darkCircles" => "lightens the shadows under the eyes.",
        "retouch.eyes.eyeBags" => "softens the bulge under the eyes.",
        "retouch.teethLips.teethWhitening" => "whitens the teeth.",
        "retouch.teethLips.lipColorAmount" => "sets how strongly the chosen lip color shows.",
        "retouch.teethLips.lipHue" => "shifts the hue of the lips.",
        "retouch.teethLips.lipSaturation" => "makes the lip color more or less intense.",
        "retouch.hair.amount" => "removes stray hairs at the edge of the hair and across the face.",
        "retouch.glare.amount" => "sets how much of the glasses glare repair shows.",
        "retouch.reshape.slimFace" => "slims or widens the face.",
        "retouch.reshape.jaw" => "narrows or widens the jaw.",
        "retouch.reshape.chin" => "lengthens or shortens the chin.",
        "retouch.reshape.doubleChin" => "lifts or lowers the soft area under the jaw.",
        "retouch.reshape.nose" => "narrows or widens the lower nose.",
        "retouch.reshape.forehead" => "raises or lowers the top of the forehead.",
        "retouch.reshape.eyeSize" => "enlarges or shrinks the eyes.",
        "retouch.background.blur" => "blurs what is behind the person.",
        "retouch.background.swap" => "sets how much the chosen backdrop replaces the background.",
        "retouch.background.clean" => "smooths the backdrop behind the person.",
        _ => return None,
    })
}

/// The tools' sliders: brushes, spots, presets.
fn tools(id: &str) -> Option<&'static str> {
    Some(match id {
        "ui.brushSize" => "sets how large the brush is.",
        "ui.brushFeather" => "sets how soft the brush edge is.",
        "ui.brushFlow" => "sets how much each pass of the brush adds.",
        "ui.lensBlurBrushAmount" => "sets how strongly the brush blurs.",
        "portrait.refineStrength" => "sets how strongly the Refine brush changes the effect.",
        "ui.removeSize" => "sets how big the spot is.",
        "ui.removeFeather" => "sets how soft the spot's edge is.",
        "ui.removeOpacity" => "sets how strongly the spot covers what is under it.",
        "ui.removeExpand" => "sets how far a spot reaches past the object it removes.",
        "ui.spotsThreshold" => "sets how sensitive the Visualize Spots view is.",
        "presetAmount" => "sets how strongly the applied preset is blended in.",
        _ => return None,
    })
}
