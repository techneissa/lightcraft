//! The Portrait panel's section of the teeth lips feature (`lightcraft_retouch::teeth_lips`). Empty until the
//! feature lands: its issue fills `body` (sliders through [`super::Ctx::slider`]), nothing else in the panel.

use crate::icons::Icon;

pub(super) const ICON: Icon = Icon::Chat;

pub(super) fn body(_cx: &mut super::Ctx<'_>) {}
