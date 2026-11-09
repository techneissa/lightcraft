//! The Portrait tab's Face Reshape section (`lightcraft_retouch::reshape`): Slim Face, Jaw, Chin, Double Chin, Nose,
//! Forehead and Eye Size, for every face or the picked one.

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    for id in [
        "retouch.reshape.slimFace",
        "retouch.reshape.jaw",
        "retouch.reshape.chin",
        "retouch.reshape.doubleChin",
        "retouch.reshape.nose",
        "retouch.reshape.forehead",
        "retouch.reshape.eyeSize",
    ] {
        cx.slider(id);
    }
}
