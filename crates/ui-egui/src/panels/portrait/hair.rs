//! The Portrait tab's section of the Stray Hair feature (`lightcraft_retouch::hair`): Amount, for every face or the
//! picked one.

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    cx.slider("retouch.hair.amount");
}
