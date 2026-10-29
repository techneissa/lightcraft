//! The bottom bar's quick filters: a Stars dropdown (several ratings, the menu stays open) and flag
//! buttons that drive the same `library.filter` as the filter bar, so both show one state.

use std::time::Duration;

use egui::vec2;
use lightcraft_catalog::Flag;
use serde_json::json;

use crate::LightcraftApp;
use crate::headless::Headless;
use crate::panels::quick_filters::{Mode, mode, stars_summary};
use crate::state::ViewMode;

const T: Duration = Duration::from_secs(10);
const SETTLE: Duration = Duration::from_secs(120);

/// The demo library on a wide window, photo `i` rated `i % 6` and flagged pick / reject / none by `i / 6`.
fn rated() -> Headless {
    let services = crate::Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    app.ui.view = ViewMode::PhotoGrid;
    for (i, id) in app.session.visible_cloned().into_iter().enumerate() {
        app.session.execute("photo.rate", &json!({"rating": i % 6, "ids": [id.0]})).unwrap();
        let flag = ["pick", "reject", "none"][(i / 6) % 3];
        app.session.execute("photo.flag", &json!({"flag": flag, "ids": [id.0]})).unwrap();
    }
    let mut h = Headless::new(app, [1400.0, 800.0], 1.0);
    h.settle(SETTLE);
    h
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
}

fn shown(h: &mut Headless) -> Vec<(u8, Flag)> {
    h.app.session.visible_cloned().into_iter().filter_map(|id| h.app.session.catalog.photo(id)).map(|p| (p.rating, p.flag)).collect()
}

#[test]
fn the_stars_dropdown_keeps_open_while_ratings_are_ticked() {
    let mut h = rated();
    let all = h.app.session.visible_cloned().len();
    click(&mut h, "quickfilter:stars");
    h.settle(SETTLE);
    // the menu is open, and a second click lands on a row of it
    click(&mut h, "quickfilter:star:1");
    h.settle(SETTLE);
    click(&mut h, "quickfilter:star:2");
    h.settle(SETTLE);
    assert_eq!(h.app.session.filter.ratings, vec![1, 2]);
    assert_eq!(stars_summary(&h.app.session.filter), "1★, 2★");
    let got = shown(&mut h);
    assert!(!got.is_empty() && got.iter().all(|(r, _)| *r == 1 || *r == 2));
    // unticking a row takes it out again, and the menu is still there for the last one
    click(&mut h, "quickfilter:star:1");
    click(&mut h, "quickfilter:star:2");
    h.settle(SETTLE);
    assert_eq!(h.app.session.filter, Default::default());
    assert_eq!(h.app.session.visible_cloned().len(), all);
    assert_eq!(stars_summary(&h.app.session.filter), "All");
}

#[test]
fn flags_or_within_the_group_and_stars_and_flags() {
    let mut h = rated();
    click(&mut h, "quickfilter:flag:pick");
    click(&mut h, "quickfilter:flag:none");
    h.settle(SETTLE);
    assert!(shown(&mut h).iter().all(|(_, f)| *f != Flag::Reject));
    assert!(shown(&mut h).iter().any(|(_, f)| *f == Flag::Pick) && shown(&mut h).iter().any(|(_, f)| *f == Flag::None));
    // 1★ + Picked + Unflagged: AND between the groups
    click(&mut h, "quickfilter:stars");
    h.settle(SETTLE);
    click(&mut h, "quickfilter:star:1");
    h.settle(SETTLE);
    let got = shown(&mut h);
    assert!(!got.is_empty() && got.iter().all(|(r, f)| *r == 1 && *f != Flag::Reject), "{got:?}");
    // ✕ resets both groups, and nothing is hidden any more
    click(&mut h, "quickfilter:clear");
    h.settle(SETTLE);
    assert_eq!(h.app.session.filter, Default::default());
}

#[test]
fn the_filter_bar_and_the_bottom_bar_show_one_state() {
    let mut h = rated();
    h.request("engine.execute", json!({"command": "view.filterBar"}), T);
    h.settle(SETTLE);
    // the filter bar's ≥ 3★ reads as 3★ 4★ 5★ in the Stars dropdown; Picked from the filter bar lights the bottom bar's button
    click(&mut h, "filter:star3");
    click(&mut h, "filter:pick");
    h.settle(SETTLE);
    assert_eq!(stars_summary(&h.app.session.filter), "3★, 4★, 5★");
    // a quick-filter tick replaces the range with a set that the filter bar names beside its stars
    click(&mut h, "quickfilter:flag:reject");
    h.settle(SETTLE);
    assert_eq!(h.app.session.filter.flags_shown(), Some(vec![Flag::Pick, Flag::Reject]));
    click(&mut h, "quickfilter:stars");
    h.settle(SETTLE);
    click(&mut h, "quickfilter:star:5");
    h.settle(SETTLE);
    assert_eq!((h.app.session.filter.rating, h.app.session.filter.ratings.clone()), (0, vec![3, 4]));
    // the filter bar's flag button follows: Picked is lit there too, and clicking it takes Picked off the same set
    click(&mut h, "filter:pick");
    h.settle(SETTLE);
    assert_eq!(h.app.session.filter.flags_shown(), Some(vec![Flag::Reject]));
    // and a star pick in the filter bar replaces the set
    click(&mut h, "filter:star2");
    h.settle(SETTLE);
    assert_eq!((h.app.session.filter.rating, h.app.session.filter.ratings.clone()), (2, vec![]));
    click(&mut h, "button:filterClear");
    h.settle(SETTLE);
    assert_eq!(h.app.session.filter, Default::default());
}

#[test]
fn the_bar_has_room_in_steps() {
    assert_eq!(mode(1000.0, false), Mode::Full);
    assert_eq!(mode(800.0, false), Mode::Compact);
    assert_eq!(mode(600.0, false), Mode::Hidden);
    // Detail and Compare fill the right side already: one button, and only in a wider bar
    assert_eq!(mode(1000.0, true), Mode::Compact);
    assert_eq!(mode(800.0, true), Mode::Hidden);
}

/// Nothing overlaps the slider, the view buttons or the pill at any width, in any view, with a filter on or off.
#[test]
fn the_bottom_bar_does_not_overlap_with_the_quick_filters() {
    for width in [280.0, 400.0, 600.0, 720.0, 800.0, 900.0, 960.0, 1000.0, 1400.0] {
        for view in [ViewMode::PhotoGrid, ViewMode::SquareGrid, ViewMode::Detail, ViewMode::Compare, ViewMode::Survey, ViewMode::People] {
            for filtering in [false, true] {
                for scale in [1.0, 2.0] {
                    let ctx = egui::Context::default();
                    crate::theme::install_fonts(&ctx);
                    crate::theme::apply(&ctx);
                    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Default::default());
                    app.ui.view = view;
                    if filtering {
                        app.session.execute("library.filter", &json!({"ratings": [1, 2, 3], "flags": ["pick", "none"]})).unwrap();
                    }
                    let mut input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 100.0))), ..Default::default() };
                    input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
                    let mut output = ctx.run_ui(input, |ui| crate::panels::bottombar::show(&mut app, ui));
                    output.textures_delta.clear();
                    let controls = crate::widgets::take_registry(&ctx);
                    let has = |id: &str| controls.iter().any(|(c, _)| c == id);
                    // the right group is the thumbnail slider in the grid views; the quick filters sit left of it
                    if matches!(view, ViewMode::PhotoGrid | ViewMode::SquareGrid | ViewMode::People) {
                        match mode(width - 24.0, false) {
                            Mode::Full => assert!(has("quickfilter:stars") && has("quickfilter:flag:pick") && has("quickfilter:flag:none"), "{view:?} at {width}: {controls:?}"),
                            Mode::Compact => assert!(has("icon:quickFilters") && !has("quickfilter:stars"), "{view:?} at {width}"),
                            Mode::Hidden => assert!(!has("icon:quickFilters") && !has("quickfilter:stars"), "{view:?} at {width}"),
                        }
                    }
                    for (index, (id, rect)) in controls.iter().enumerate() {
                        assert!(rect.left() >= 0.0 && rect.right() <= width, "{view:?} at {width}: {id} outside the bar: {rect:?}");
                        for (other_id, other) in controls.iter().skip(index + 1) {
                            assert!(!rect.intersect(*other).is_positive(), "{view:?} at {width} filtering {filtering}: {id} overlaps {other_id}: {rect:?} / {other:?}");
                        }
                    }
                }
            }
        }
    }
}
