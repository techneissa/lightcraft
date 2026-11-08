//! The Portrait panel's section of the skin tone feature (`lightcraft_retouch::skin_tone`). Empty until the
//! feature lands: its issue fills `body` (sliders through [`super::Ctx::slider`]), nothing else in the panel.

use crate::icons::Icon;

pub(super) const ICON: Icon = Icon::Picker;

pub(super) fn body(_cx: &mut super::Ctx<'_>) {}
