//! The Portrait tab's section of the wrinkles feature (`lightcraft_retouch::wrinkles`): Forehead, Crow's Feet, Smile
//! Lines and Neck, for every face or the picked one.

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    for id in ["retouch.wrinkles.forehead", "retouch.wrinkles.crowsFeet", "retouch.wrinkles.smileLines", "retouch.wrinkles.neck"] {
        cx.slider(id);
    }
}
