//! The Keyboard Shortcuts window's size: it opens at about 1000 × 640, can be dragged larger or
//! smaller, and opens at the size it was left at (saved in the settings, `shortcutsWindowSize`).
//! The list takes the height the window gives it.

use egui::{Vec2, vec2};

use crate::LightcraftApp;

/// The size it opens at the first time.
pub const DEFAULT: Vec2 = vec2(1000.0, 640.0);
/// The smallest it is dragged to: the columns still fit.
pub const MIN: Vec2 = vec2(640.0, 420.0);
/// What a very small screen still gets: the title, the search, a few rows and the buttons.
const SMALLEST: Vec2 = vec2(400.0, 280.0);
/// Room left around the window on the screen.
const MARGIN: f32 = 12.0;
/// Room below the list before it has been measured (the footer, the status line and the buttons).
/// The least height the list is given, so a short window still shows a few rows.
const MIN_LIST: f32 = 48.0;
const BELOW_GUESS: f32 = 112.0;
const CONTENT_BOTTOM: &str = "shortcuts-content-bottom";
const LIST_BOTTOM: &str = "shortcuts-list-bottom";

/// The largest the window is dragged to on a screen of `screen` points.
pub fn max_size(screen: Vec2) -> Vec2 {
    vec2((screen.x - MARGIN).max(SMALLEST.x), (screen.y - MARGIN).max(SMALLEST.y))
}

/// The smallest it is dragged to on a screen that holds [`MIN`]: never beyond what `screen` holds.
pub fn min_size(screen: Vec2) -> Vec2 {
    MIN.min(max_size(screen))
}

/// The size to open at: the remembered one, else [`DEFAULT`], never beyond what the screen holds.
pub fn opening_size(app: &LightcraftApp, screen: Vec2) -> Vec2 {
    let saved = app.ui.settings.shortcuts_window_size.map(|[w, h]| vec2(w, h)).filter(|s| s.x.is_finite() && s.y.is_finite());
    let want = saved.unwrap_or(DEFAULT);
    let max = max_size(screen);
    let min = min_size(screen);
    vec2(want.x.clamp(min.x, max.x), want.y.clamp(min.y, max.y))
}

/// Remember the window's `size` once the user is done dragging. A size at the screen's limit is
/// the screen's, not the user's choice, so a large size squeezed onto a small screen is kept for a
/// larger one. Returns whether it was saved.
pub fn remember(app: &mut LightcraftApp, size: Vec2, screen: Vec2) -> bool {
    let max = max_size(screen);
    if !(size.x.is_finite() && size.y.is_finite()) || size.x >= max.x - 0.5 || size.y >= max.y - 0.5 {
        return false;
    }
    let new = [size.x.round(), size.y.round()];
    if app.ui.settings.shortcuts_window_size == Some(new) || (app.ui.settings.shortcuts_window_size.is_none() && (size - DEFAULT).length() < 1.0) {
        return false;
    }
    app.ui.settings.shortcuts_window_size = Some(new);
    true
}

/// Where the dialog's content ends this frame (after its buttons), for [`list_height`] on the next.
pub fn note_content_bottom(ui: &egui::Ui) {
    if !ui.is_sizing_pass() {
        let y = ui.min_rect().bottom();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(CONTENT_BOTTOM), y));
    }
}

/// Where the list ended this frame.
pub(super) fn note_list_bottom(ui: &egui::Ui, y: f32) {
    if !ui.is_sizing_pass() {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(LIST_BOTTOM), y));
    }
}

/// The height of the list: what the window offers less what sits below it, in whole points so the
/// window never grows by a fraction each frame.
pub(super) fn list_height(ui: &egui::Ui) -> f32 {
    let (content, list) = ui.ctx().data(|d| (d.get_temp::<f32>(egui::Id::unique(CONTENT_BOTTOM)), d.get_temp::<f32>(egui::Id::unique(LIST_BOTTOM))));
    let below = match (content, list) {
        (Some(c), Some(l)) if c >= l && c - l < 400.0 => c - l,
        _ => BELOW_GUESS,
    };
    let h = (ui.available_height() - below).floor();
    if h.is_finite() { h.max(MIN_LIST) } else { 300.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> LightcraftApp {
        LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default())
    }

    #[test]
    fn it_opens_at_1000_by_640_until_a_size_is_remembered() {
        let mut a = app();
        let screen = vec2(1800.0, 1100.0);
        assert_eq!(opening_size(&a, screen), vec2(1000.0, 640.0));
        assert!(remember(&mut a, vec2(1200.0, 800.0), screen));
        assert_eq!(opening_size(&a, screen), vec2(1200.0, 800.0));
        assert!(!remember(&mut a, vec2(1200.0, 800.0), screen), "an unchanged size is not saved again");
    }

    #[test]
    fn a_small_screen_squeezes_it_without_losing_the_choice() {
        let mut a = app();
        let big = vec2(1800.0, 1100.0);
        assert!(remember(&mut a, vec2(1400.0, 900.0), big));
        let small = vec2(1100.0, 700.0);
        assert_eq!(opening_size(&a, small), max_size(small));
        assert!(!remember(&mut a, max_size(small), small), "the screen's limit is not the user's choice");
        assert_eq!(a.ui.settings.shortcuts_window_size, Some([1400.0, 900.0]));
    }

    #[test]
    fn a_damaged_saved_size_falls_back() {
        let mut a = app();
        a.ui.settings.shortcuts_window_size = Some([f32::NAN, 10.0]);
        assert_eq!(opening_size(&a, vec2(1800.0, 1100.0)), DEFAULT);
        a.ui.settings.shortcuts_window_size = Some([10.0, 10.0]);
        assert_eq!(opening_size(&a, vec2(1800.0, 1100.0)), MIN);
    }
}
