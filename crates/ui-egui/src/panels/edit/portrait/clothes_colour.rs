//! The Portrait panel's section of the clothes colour feature (`lightcraft_retouch::clothes_colour`). Empty until the
//! feature lands: its issue fills `body` (sliders through [`super::Ctx::slider`]), nothing else in the panel.

use crate::icons::Icon;

pub(super) const ICON: Icon = Icon::Tag;

pub(super) fn body(_cx: &mut super::Ctx<'_>) {}
