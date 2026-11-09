//! The Portrait tab's section of the skin tone feature (`lightcraft_retouch::skin_tone`): Even Tone, Redness and
//! Face Light, for every face or the picked one.

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    for id in ["retouch.skinTone.evenTone", "retouch.skinTone.redness", "retouch.skinTone.faceLight"] {
        cx.slider(id);
    }
}
