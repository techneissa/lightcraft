//! The Portrait panel's section of the background feature (`lightcraft_retouch::background`). Empty until the
//! feature lands: its issue fills `body` (sliders through [`super::Ctx::slider`]), nothing else in the panel.

use crate::icons::Icon;

pub(super) const ICON: Icon = Icon::Subject;

pub(super) fn body(_cx: &mut super::Ctx<'_>) {}
