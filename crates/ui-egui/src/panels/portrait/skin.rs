//! The Portrait tab's section of the skin feature (`lightcraft_retouch::skin`): Smoothness, Texture and Shine, for
//! every face or the picked one.

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    for id in ["retouch.skin.smoothness", "retouch.skin.texture", "retouch.skin.shine"] {
        cx.slider(id);
    }
}
