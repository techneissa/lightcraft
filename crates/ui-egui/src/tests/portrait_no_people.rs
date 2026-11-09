//! The Portrait tab on a photo without people (fork, `fork/retouch-foundation.md`, issue #167): once nobody is seen in
//! the photo (the face search found none, or the early check did: a hint), the tool strip's Portrait icon is dimmed
//! in its place but still opens the tab, which is one line, "No People in This Photo", and "Search Again" (the real
//! face search); a photo with faces has all its sections.

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
fn the_portrait_icon_stays_in_its_place_and_opens_the_tab_without_people() {
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
    let r = h.request("ui.clickWidget", json!({"id": "icon:portrait"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_eq!(h.app.ui.right, RightPanel::Portrait, "the dimmed icon still opens the tab");
    assert!(rect_of(&mut h, "portrait:noPeople").is_some(), "which says so");
    assert!(rect_of(&mut h, "button:portraitSearchAgain").is_some(), "and can search again");
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

/// The real key: T opens the Portrait tab also on a photo without people, where it says so.
#[test]
fn shortcut_t_opens_the_tab_on_a_photo_without_people_and_says_so() {
    let (mut h, landscape, _) = two_photos();
    h.app.ui.right = RightPanel::Edit;
    select(&mut h, landscape);
    let r = h.request("ui.key", json!({"key": "t"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_eq!(h.app.ui.right, RightPanel::Portrait, "T opens the tab");
    assert!(rect_of(&mut h, "portrait:noPeople").is_some(), "with its one line");
    assert!(rect_of(&mut h, "section:portrait.skin").is_none());
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

/// A detector that finds a face while `people` is set (nobody else), and counts its runs.
struct Counting {
    runs: Arc<std::sync::atomic::AtomicUsize>,
    people: Arc<AtomicBool>,
}

impl FaceProvider for Counting {
    fn name(&self) -> &str {
        "Counting"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(if self.people.load(Ordering::SeqCst) { vec![Face::boxed(NormRect::new(0.2, 0.2, 0.35, 0.45))] } else { vec![] })
    }
}

fn counting_app(parse_ahead: bool) -> (Headless, Arc<std::sync::atomic::AtomicUsize>, Arc<AtomicBool>) {
    let (runs, people) = (Arc::new(std::sync::atomic::AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(Counting { runs: runs.clone(), people: people.clone() }));
    // (mains power: the test does not depend on the Mac it runs on)
    session.parse_ahead_fix_power(Some(lightcraft_engine::parse_ahead::power::Power::default()));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    h.app.ui.settings.parse_ahead = parse_ahead;
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    (h, runs, people)
}

/// A photo without faces dims the Portrait icon as it opens, without the tab ever being opened (a hint: nothing was
/// searched); the tab then says so without searching, and "Search Again" runs the real search, which fills the tab
/// when it finds a face.
#[test]
fn the_early_check_dims_the_icon_and_search_again_runs_the_real_search() {
    let (mut h, runs, people) = counting_app(true);
    let photo = h.app.session.active().unwrap();
    assert_ne!(h.app.ui.right, RightPanel::Portrait);
    let found = h.step_until(Duration::from_secs(60), |h| h.app.session.retouch_no_people(photo));
    assert!(found, "the photo was never found to have no face");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    let st = h.app.session.retouch_faces_of(photo);
    assert!(st.found.is_none() && st.hint, "a hint, not a search result");
    // the icon opens the tab, which does not search by itself
    let r = h.request("ui.clickWidget", json!({"id": "icon:portrait"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_eq!(h.app.ui.right, RightPanel::Portrait);
    assert!(rect_of(&mut h, "portrait:noPeople").is_some());
    assert!(rect_of(&mut h, "section:portrait.skin").is_none());
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the tab searched by itself");
    // Search Again: the real search, now finding a face (the early check had missed it)
    people.store(true, Ordering::SeqCst);
    let r = h.request("ui.clickWidget", json!({"id": "button:portraitSearchAgain"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let found = h.step_until(Duration::from_secs(60), |h| h.app.session.retouch_faces_of(photo).found.is_some());
    assert!(found, "the search never finished");
    settle(&mut h);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert!(!h.app.session.retouch_no_people(photo), "the icon is normal again");
    assert!(rect_of(&mut h, "portrait:noPeople").is_none());
    assert!(rect_of(&mut h, "section:portrait.skin").is_some(), "the tab fills");
}

/// Search Again that finds nobody again leaves the tab as it was, with its button back.
#[test]
fn search_again_that_finds_nobody_leaves_the_one_line() {
    let (mut h, runs, _) = counting_app(true);
    let photo = h.app.session.active().unwrap();
    assert!(h.step_until(Duration::from_secs(60), |h| h.app.session.retouch_no_people(photo)));
    h.app.ui.right = RightPanel::Portrait;
    settle(&mut h);
    let r = h.request("ui.clickWidget", json!({"id": "button:portraitSearchAgain"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(Duration::from_secs(60), |h| h.app.session.retouch_faces_of(photo).found.is_some()));
    settle(&mut h);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    let st = h.app.session.retouch_faces_of(photo);
    assert!(st.no_people() && !st.hint, "now a real answer");
    assert!(rect_of(&mut h, "portrait:noPeople").is_some() && rect_of(&mut h, "button:portraitSearchAgain").is_some());
}

/// With the setting off in Settings ▸ Performance no check runs: the icon stays on until the tab is opened.
#[test]
fn no_check_runs_while_parsing_ahead_is_switched_off() {
    let (mut h, runs, _) = counting_app(false);
    let photo = h.app.session.active().unwrap();
    for _ in 0..90 {
        h.step();
    }
    assert_eq!((runs.load(Ordering::SeqCst), h.app.session.parse_ahead_peeks()), (0, 0));
    assert!(!h.app.session.retouch_no_people(photo));
}
