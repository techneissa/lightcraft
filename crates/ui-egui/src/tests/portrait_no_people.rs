//! The Portrait tab on a photo without people (fork, `fork/retouch-foundation.md`, issue #167): once the face search
//! found nobody, the tool strip's Portrait icon is off (in its place), shortcut T does nothing, and the open tab is
//! one line with no sections; a photo with faces has them all.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lightcraft_develop::retouch::{Face, NormRect};
use lightcraft_engine::retouch_input::FaceProvider;
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// A face while `people` is set, else none: the test flips it between two searches.
struct Switch(Arc<AtomicBool>);

impl FaceProvider for Switch {
    fn name(&self) -> &str {
        "Switch"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        Ok(if self.0.load(Ordering::SeqCst) { vec![Face::boxed(NormRect::new(0.2, 0.2, 0.35, 0.45))] } else { vec![] })
    }
}

fn rect_of(h: &mut Headless, id: &str) -> Option<[f64; 4]> {
    let r = h.request("ui.widgets", json!({"filter": id}), T);
    r["result"].as_array().into_iter().flatten().find(|w| w["id"] == id).and_then(|w| serde_json::from_value(w["rect"].clone()).ok())
}

fn settle(h: &mut Headless) {
    for _ in 0..6 {
        h.step();
    }
}

/// Two photos: the first searched with no face (a landscape), the second with one.
fn two_photos() -> (Headless, lightcraft_catalog::PhotoId, lightcraft_catalog::PhotoId) {
    let people = Arc::new(AtomicBool::new(false));
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(Switch(people.clone())));
    let ids = session.visible_cloned();
    let (landscape, portrait) = (ids[0], ids[1]);
    session.retouch_detect_now(landscape, false).unwrap();
    people.store(true, Ordering::SeqCst);
    session.retouch_detect_now(portrait, false).unwrap();
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    (h, landscape, portrait)
}

fn select(h: &mut Headless, id: lightcraft_catalog::PhotoId) {
    h.app.session.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    settle(h);
}

#[test]
fn the_portrait_icon_is_off_without_people_and_stays_in_its_place() {
    let (mut h, landscape, portrait) = two_photos();
    h.app.ui.right = RightPanel::Edit;
    select(&mut h, portrait);
    let on = (rect_of(&mut h, "icon:portrait").unwrap(), rect_of(&mut h, "icon:masking").unwrap(), rect_of(&mut h, "icon:redeye").unwrap());
    let r = h.request("ui.clickWidget", json!({"id": "icon:portrait"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_eq!(h.app.ui.right, RightPanel::Portrait, "with faces the icon opens the tab");
    h.app.ui.right = RightPanel::Edit;
    select(&mut h, landscape);
    let off = (rect_of(&mut h, "icon:portrait").unwrap(), rect_of(&mut h, "icon:masking").unwrap(), rect_of(&mut h, "icon:redeye").unwrap());
    assert_eq!(on, off, "no icon moves");
    let _ = h.request("ui.clickWidget", json!({"id": "icon:portrait"}), T);
    settle(&mut h);
    assert_eq!(h.app.ui.right, RightPanel::Edit, "without people the icon does nothing");
    // another icon still works
    let r = h.request("ui.clickWidget", json!({"id": "icon:masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_eq!(h.app.ui.right, RightPanel::Masking);
}

#[test]
fn the_icon_stays_on_while_the_faces_are_not_known() {
    let people = Arc::new(AtomicBool::new(false));
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(Switch(people)));
    let id = session.active().unwrap();
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    assert!(!h.app.session.retouch_no_people(id), "nothing searched: unknown");
    let r = h.request("ui.clickWidget", json!({"id": "icon:portrait"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.right, RightPanel::Portrait, "the tab opens and starts the search");
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while !h.app.session.retouch_no_people(id) {
        assert!(std::time::Instant::now() < until, "the search never finished");
        h.step();
    }
    settle(&mut h);
    assert!(rect_of(&mut h, "portrait:noPeople").is_some(), "the open tab collapses once the search found nobody");
    assert!(rect_of(&mut h, "section:portrait.skin").is_none());
}

#[test]
fn the_open_tab_is_one_line_without_people_and_follows_the_photo() {
    let (mut h, landscape, portrait) = two_photos();
    h.app.ui.right = RightPanel::Portrait;
    select(&mut h, landscape);
    assert!(rect_of(&mut h, "portrait:noPeople").is_some(), "the line");
    for id in ["section:portrait.skin", "section:portrait.presets", "portrait:editing", "portrait:card", "portrait:switch"] {
        let got = rect_of(&mut h, id);
        // (the title row with its eye is the tab's header and stays)
        assert_eq!(got.is_some(), id == "portrait:switch", "{id}: {got:?}");
    }
    select(&mut h, portrait);
    assert!(rect_of(&mut h, "portrait:noPeople").is_none());
    assert!(rect_of(&mut h, "section:portrait.skin").is_some(), "a photo with faces has its sections");
    assert!(rect_of(&mut h, "portrait:card").is_some());
    select(&mut h, landscape);
    assert!(rect_of(&mut h, "portrait:noPeople").is_some(), "and back");
    assert!(rect_of(&mut h, "section:portrait.skin").is_none());
}

#[test]
fn shortcut_t_does_nothing_on_a_photo_without_people() {
    let (mut h, landscape, portrait) = two_photos();
    h.app.ui.right = RightPanel::Edit;
    select(&mut h, landscape);
    let r = h.request("engine.execute", json!({"command": "panel.portrait", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_eq!(h.app.ui.right, RightPanel::Edit, "no tab for a landscape");
    assert!(h.app.ui.toast.as_ref().is_some_and(|t| t.0 == "No People in This Photo"), "it says why");
    select(&mut h, portrait);
    let r = h.request("engine.execute", json!({"command": "panel.portrait", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.right, RightPanel::Portrait, "with faces it opens");
}

/// A photo that keeps Portrait settings but shows no face (its faces were deleted) keeps them.
#[test]
fn stored_portrait_settings_stay_when_the_photo_shows_no_face() {
    let (mut h, landscape, _) = two_photos();
    h.app.session.execute("retouch.set", &json!({"control": "retouch.skin.smoothness", "value": 40, "id": landscape.0})).unwrap();
    let before = h.app.session.develop_of(landscape).unwrap().retouch.clone();
    h.app.ui.right = RightPanel::Portrait;
    select(&mut h, landscape);
    assert!(rect_of(&mut h, "portrait:noPeople").is_some());
    let after = h.app.session.develop_of(landscape).unwrap().retouch.clone();
    assert_eq!(before.global.get("retouch.skin.smoothness"), Some(40.0));
    assert_eq!(after.global.get("retouch.skin.smoothness"), Some(40.0), "looking at the collapsed tab changes nothing");
}

/// A detector that finds nobody and counts its runs.
struct Counting(Arc<std::sync::atomic::AtomicUsize>);

impl FaceProvider for Counting {
    fn name(&self) -> &str {
        "Counting"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
}

fn counting_app(parse_ahead: bool) -> (Headless, Arc<std::sync::atomic::AtomicUsize>) {
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(Counting(runs.clone())));
    // (mains power: the test does not depend on the Mac it runs on)
    session.parse_ahead_fix_power(Some(lightcraft_engine::parse_ahead::power::Power::default()));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    h.app.ui.settings.parse_ahead = parse_ahead;
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    (h, runs)
}

/// A photo without faces greys the Portrait icon as it opens, without the tab ever being opened.
#[test]
fn the_icon_is_off_for_a_photo_without_faces_before_the_tab_is_opened() {
    let (mut h, runs) = counting_app(true);
    let photo = h.app.session.active().unwrap();
    assert_ne!(h.app.ui.right, RightPanel::Portrait);
    let found = h.step_until(Duration::from_secs(60), |h| h.app.session.retouch_no_people(photo));
    assert!(found, "the photo was never found to have no face");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    settle(&mut h);
    let r = h.request("ui.clickWidget", json!({"id": "icon:portrait"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_ne!(h.app.ui.right, RightPanel::Portrait, "the greyed icon opened the tab");
    // the tab later (by another way in): one line, and no search of its own
    h.app.ui.right = RightPanel::Portrait;
    settle(&mut h);
    assert!(rect_of(&mut h, "portrait:noPeople").is_some());
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the tab searched again");
}

/// With the setting off in Settings ▸ Performance no check runs: the icon stays on until the tab is opened.
#[test]
fn no_check_runs_while_parsing_ahead_is_switched_off() {
    let (mut h, runs) = counting_app(false);
    let photo = h.app.session.active().unwrap();
    for _ in 0..90 {
        h.step();
    }
    assert_eq!((runs.load(Ordering::SeqCst), h.app.session.parse_ahead_peeks()), (0, 0));
    assert!(!h.app.session.retouch_no_people(photo));
}
