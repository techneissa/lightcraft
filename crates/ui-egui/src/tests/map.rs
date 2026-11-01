//! Headless tests of the Map view: switching to it, fitting the photos with GPS, clusters and
//! selection, moving the view by commands and gestures, tile textures, the attribution, offline
//! mode and hostile positions. Tiles come from an in-process fake server (never the network).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use lightcraft_map::{HttpResponse, TileService, TileSource, Transport};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

pub(super) const T: Duration = Duration::from_secs(20);
pub(super) const SETTLE: Duration = Duration::from_secs(120);

/// A tile server in the test: every tile is the same 256 px PNG (or garbage for `bad` columns).
pub(super) struct FakeTiles {
    png: Vec<u8>,
    requests: AtomicUsize,
    /// Tiles whose x is odd come back as bytes that aren't an image.
    garbage_odd: bool,
}

impl Transport for FakeTiles {
    fn get(&self, url: &str, _: &[(&'static str, String)], _: usize, _: &AtomicBool) -> Result<HttpResponse, String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let x: u32 = url.rsplit('/').nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        let body = if self.garbage_odd && x % 2 == 1 { b"<html>not a tile</html>".to_vec() } else { self.png.clone() };
        Ok(HttpResponse { status: 200, headers: vec![("cache-control".into(), "max-age=604800".into())], body })
    }
}

pub(super) fn tile_png() -> Vec<u8> {
    let mut px = Vec::with_capacity(256 * 256 * 4);
    for y in 0..256u32 {
        for x in 0..256u32 {
            let line = x % 64 == 0 || y % 64 == 0;
            let v = if line { 150 } else { 205 };
            px.extend_from_slice(&[v, v + 10, v - 20, 255]);
        }
    }
    let img = lightcraft_raster::Rgba8::from_bytes(256, 256, &px).unwrap();
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

pub(super) fn fake(garbage_odd: bool) -> Arc<FakeTiles> {
    Arc::new(FakeTiles { png: tile_png(), requests: AtomicUsize::new(0), garbage_odd })
}

/// Paris ×3, London ×2, New York ×1.
pub(super) const PLACES: [(f64, f64); 6] =
    [(48.8584, 2.2945), (48.8606, 2.3376), (48.8530, 2.3499), (51.5007, -0.1246), (51.5081, -0.0759), (40.7484, -73.9857)];

/// The demo library at 1400 × 900 with the fake tile server; the first six photos in view get
/// [`PLACES`] (returned in that order).
pub(super) fn demo(tiles: Option<Arc<FakeTiles>>) -> (Headless, Vec<u64>) {
    demo_sized(tiles, [1400.0, 900.0])
}

pub(super) fn demo_sized(tiles: Option<Arc<FakeTiles>>, size: [f32; 2]) -> (Headless, Vec<u64>) {
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    if let Some(f) = tiles {
        app.session.map.set_service(TileService::with_transport(TileSource::openstreetmap(), None, Some(f as Arc<dyn Transport>)));
    } else {
        app.session.map.set_source(TileSource::offline());
    }
    let mut h = Headless::new(app, size, 1.0);
    h.settle(SETTLE);
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(PLACES.len()).map(|p| p.0).collect();
    assert_eq!(ids.len(), PLACES.len(), "the demo library has enough photos");
    for (id, (lat, lon)) in ids.iter().zip(PLACES) {
        h.app.run("photo.setMeta", json!({"ids": [id], "gps": [lat, lon]})).unwrap();
    }
    (h, ids)
}

pub(super) fn run(h: &mut Headless, id: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": id, "params": params}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    r["result"].clone()
}

pub(super) fn state(h: &mut Headless) -> Value {
    run(h, "map.state", json!({}))
}

pub(super) fn frames(h: &mut Headless, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

pub(super) fn clusters(s: &Value) -> Vec<Value> {
    s["clusters"].as_array().cloned().unwrap_or_default()
}

pub(super) fn ids_of(c: &Value) -> Vec<u64> {
    let mut v: Vec<u64> = c["ids"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    v.sort();
    v
}

#[test]
fn the_map_view_fits_the_photos_with_gps_and_credits_the_tiles() {
    let (mut h, ids) = demo(Some(fake(false)));
    let total = h.app.session.visible_cloned().len();
    let r = run(&mut h, "view.map", json!({}));
    assert_eq!((r["withGps"].as_u64(), r["withoutGps"].as_u64()), (Some(6), Some(total as u64 - 6)));
    frames(&mut h, 2);
    assert_eq!(h.app.ui.view, ViewMode::Map);
    let s = state(&mut h);
    // the first open fits every photo with GPS: all of them on the canvas
    let canvas = h.app.map.rect.unwrap();
    let cs = clusters(&s);
    let mut all: Vec<u64> = cs.iter().flat_map(ids_of).collect();
    all.sort();
    all.dedup();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(all, want, "{s}");
    for c in &cs {
        let (x, y) = (c["x"].as_f64().unwrap() as f32, c["y"].as_f64().unwrap() as f32);
        if ids_of(c).contains(&ids[5]) || ids_of(c).contains(&ids[0]) {
            assert!(canvas.contains(egui::pos2(x, y)), "{c} outside {canvas:?}");
        }
    }
    assert!(s["zoom"].as_f64().unwrap() >= 2.0 && s["zoom"].as_f64().unwrap() <= 5.0, "{s}");
    assert_eq!(s["zoom"].as_f64().unwrap().fract(), 0.0, "fits at a whole zoom (sharp tiles)");
    // the tile provider is credited on the map, visibly, and the photos without GPS are counted
    let text = h.painted_text().join("\n");
    assert!(text.contains("© OpenStreetMap contributors"), "{text}");
    assert!(text.contains(&format!("{} photos without GPS", total - 6)), "{text}");
    let w = h.request("ui.widgets", json!({}), T).to_string();
    for id in ["map:canvas", "map:attribution", "map:fit", "icon:mapZoomIn", "icon:mapZoomOut", "map:withoutGps"] {
        assert!(w.contains(&format!("\"{id}\"")), "{id}: {w}");
    }
    // the menu and the bottom bar know the view
    assert_eq!(crate::menubar::checked(&h.app, "view.map"), Some(true));
    assert!(w.contains("\"icon:map\""), "the bottom bar has the Map button: {w}");
    // the map draws in the docking canvas, beside (not under) the docked Tools panel
    let tools = h.app.widgets.iter().find(|(id, _)| id == "panel:right_panel").map(|(_, r)| *r).expect("Tools is docked by default");
    assert!(!canvas.intersect(tools).is_positive(), "map {canvas:?} overlaps Tools {tools:?}");
    // the view is kept for the session: leaving and coming back doesn't refit
    run(&mut h, "map.setView", json!({"lat": 10.0, "lon": 20.0, "zoom": 7}));
    run(&mut h, "view.photoGrid", json!({}));
    frames(&mut h, 2);
    run(&mut h, "view.map", json!({}));
    frames(&mut h, 2);
    let s = state(&mut h);
    assert_eq!((s["lat"].as_f64(), s["lon"].as_f64(), s["zoom"].as_f64()), (Some(10.0), Some(20.0), Some(7.0)), "{s}");
}

#[test]
fn clicking_a_cluster_selects_exactly_its_photos_and_double_click_opens_detail() {
    let (mut h, ids) = demo(None);
    run(&mut h, "view.map", json!({}));
    run(&mut h, "map.setView", json!({"lat": 50.0, "lon": 1.0, "zoom": 6}));
    frames(&mut h, 2);
    let s = state(&mut h);
    let cs = clusters(&s);
    let mut counts: Vec<u64> = cs.iter().map(|c| c["count"].as_u64().unwrap()).collect();
    counts.sort();
    assert_eq!(counts, vec![2, 3], "Paris and London apart, New York off screen: {s}");
    let paris = cs.iter().find(|c| c["count"] == 3).unwrap();
    let r = h.request("ui.click", json!({"x": paris["x"], "y": paris["y"]}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 2);
    let mut sel: Vec<u64> = h.app.session.selection.ids.iter().map(|p| p.0).collect();
    sel.sort();
    let mut want = ids[0..3].to_vec();
    want.sort();
    assert_eq!(sel, want, "exactly the cluster's photos");
    assert_eq!(h.app.ui.view, ViewMode::Map, "a click stays on the map");
    // the selected cluster is reported (and drawn) as selected
    let s = state(&mut h);
    assert!(clusters(&s).iter().any(|c| c["count"] == 3 && c["selected"] == true), "{s}");
    // clicking the map elsewhere leaves the selection alone
    h.request("ui.click", json!({"x": 300.0, "y": 600.0}), T);
    frames(&mut h, 2);
    assert_eq!(h.app.session.selection.ids.len(), 3);
    // a double-click on London selects its two photos and opens the first in Detail
    let london = clusters(&s).into_iter().find(|c| c["count"] == 2).unwrap();
    frames(&mut h, 40); // not a triple click with the clicks above
    h.request("ui.click", json!({"x": london["x"], "y": london["y"], "count": 2}), T);
    frames(&mut h, 6);
    let mut sel: Vec<u64> = h.app.session.selection.ids.iter().map(|p| p.0).collect();
    sel.sort();
    let mut want = ids[3..5].to_vec();
    want.sort();
    assert_eq!(sel, want);
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert_eq!(h.app.session.active().map(|a| a.0), Some(ids[3]));
}

#[test]
fn zooming_splits_clusters_and_commands_and_gestures_move_the_view() {
    let (mut h, ids) = demo(None);
    run(&mut h, "view.map", json!({}));
    run(&mut h, "map.setView", json!({"lat": 48.857, "lon": 2.32, "zoom": 9}));
    frames(&mut h, 2);
    let paris = |s: &Value| clusters(s).iter().filter(|c| ids_of(c).iter().any(|i| ids[0..3].contains(i))).count();
    assert_eq!(paris(&state(&mut h)), 1);
    let s = run(&mut h, "map.zoomIn", json!({}));
    assert_eq!(s["zoom"].as_f64(), Some(10.0));
    run(&mut h, "map.setView", json!({"zoom": 14}));
    frames(&mut h, 2);
    assert_eq!(paris(&state(&mut h)), 3, "zoomed in: one pin per photo");
    let s = run(&mut h, "map.zoomOut", json!({}));
    assert_eq!(s["zoom"].as_f64(), Some(13.0));
    // ⌘= / ⌘- / ⌘0 act on the map in this view
    let s = run(&mut h, "view.zoomIn", json!({}));
    assert_eq!(s["zoom"].as_f64(), Some(14.0));
    // bad parameters are refused and change nothing
    for bad in [json!({"lat": 95}), json!({"lon": -181}), json!({"zoom": 99}), json!({"zoom": -1}), json!({"lat": "north"})] {
        let r = h.request("engine.execute", json!({"command": "map.setView", "params": bad}), T);
        assert_eq!(r["ok"], false, "{bad}: {r}");
    }
    assert_eq!(state(&mut h)["zoom"].as_f64(), Some(14.0));
    // a drag pans: the content follows the pointer
    let before = state(&mut h);
    let canvas = h.app.map.rect.unwrap();
    let (cx, cy) = (canvas.center().x, canvas.center().y);
    h.request("ui.drag", json!({"x": cx, "y": cy, "toX": cx + 200.0, "toY": cy, "steps": 8}), T);
    frames(&mut h, 2);
    let after = state(&mut h);
    assert!(after["lon"].as_f64().unwrap() < before["lon"].as_f64().unwrap(), "dragging right shows what lies west: {before} → {after}");
    assert!((after["zoom"].as_f64().unwrap() - 14.0).abs() < 1e-9);
    // a pinch zooms around the pointer
    h.request("ui.move", json!({"x": cx, "y": cy}), T);
    h.step();
    h.request("ui.zoom", json!({"factor": 0.5}), T);
    frames(&mut h, 2);
    assert!((state(&mut h)["zoom"].as_f64().unwrap() - 13.0).abs() < 1e-6);
    // fit comes back to every photo
    let s = run(&mut h, "map.fit", json!({}));
    assert!(s["zoom"].as_f64().unwrap() <= 5.0, "{s}");
}

#[test]
fn tile_textures_are_uploaded_once_not_every_frame() {
    let tiles = fake(false);
    let (mut h, _) = demo(Some(tiles.clone()));
    run(&mut h, "view.map", json!({}));
    run(&mut h, "map.setView", json!({"lat": 48.857, "lon": 2.32, "zoom": 5}));
    let idle = |h: &Headless| h.app.session.map.service().is_some_and(|s| s.is_idle());
    assert!(h.step_until(T, |h| h.app.map.tiles_drawn > 0 && h.app.map.tiles_missing == 0 && idle(h)), "every tile arrives from the fake server");
    frames(&mut h, 2);
    let uploads = h.app.map.uploads;
    assert!(uploads > 0 && uploads as usize <= h.app.map.textures() + 8, "{uploads}");
    let requests = tiles.requests.load(Ordering::SeqCst);
    frames(&mut h, 30);
    assert_eq!(h.app.map.uploads, uploads, "nothing is uploaded again while the view stands still");
    assert_eq!(tiles.requests.load(Ordering::SeqCst), requests, "nor fetched again");
    // the tiles are painted: the fake tile's colour is on screen
    let img = h.snapshot(SETTLE);
    let canvas = h.app.map.rect.unwrap();
    let px = img.pixels[(canvas.top() as usize + 200) * img.size[0] + canvas.left() as usize + 200];
    assert!(px.r() > 140 && px.g() > 150, "{px:?}");
    // a headless screenshot (its own egui context) paints the window's tile textures too, and
    // uploads nothing into its context (texture ids are per context)
    let main = h.view.ctx.clone();
    let shot = h.app.headless_screenshot(&main, true).expect("a capture");
    let scale = shot.size[0] as f32 / img.size[0] as f32;
    let at = |x: f32, y: f32| shot.pixels[(y * scale) as usize * shot.size[0] + (x * scale) as usize];
    let px = at(canvas.left() + 200.0, canvas.top() + 200.0);
    // the fake tile's colour, not a missing texture's white
    let want = img.pixels[(canvas.top() as usize + 200) * img.size[0] + canvas.left() as usize + 200];
    let close = |a: u8, b: u8| a.abs_diff(b) <= 12;
    assert!(close(px.r(), want.r()) && close(px.g(), want.g()) && close(px.b(), want.b()), "the capture shows the tiles: {px:?} vs {want:?}");
    assert!(px.b() < 230, "{px:?}");
    assert_eq!(h.app.map.uploads, uploads, "the capture uploaded no tiles of its own");
    frames(&mut h, 3);
    assert_eq!(h.app.map.uploads, uploads);
    // panning a little reuses what is there and only uploads the newly shown tiles
    run(&mut h, "map.setView", json!({"lon": 4.0}));
    assert!(h.step_until(T, |h| h.app.map.tiles_missing == 0 && idle(h)));
    assert!(h.app.map.uploads - uploads <= 12, "{} new uploads for a small pan", h.app.map.uploads - uploads);
    assert!(h.app.map.texture_bytes() <= crate::panels::map::MAX_TEXTURE_BYTES);
    // another tile source: the old source's pictures go
    run(&mut h, "map.tileSource", json!({"offline": true}));
    frames(&mut h, 2);
    assert_eq!((h.app.map.textures(), h.app.map.tiles_drawn), (0, 0));
}

#[test]
fn tiles_that_fail_to_decode_leave_placeholders() {
    let (mut h, _) = demo(Some(fake(true)));
    run(&mut h, "view.map", json!({}));
    run(&mut h, "map.setView", json!({"lat": 48.857, "lon": 2.32, "zoom": 6}));
    assert!(h.step_until(T, |h| h.app.map.tiles_drawn > 0 && h.app.session.map.service().is_some_and(|s| s.is_idle())));
    frames(&mut h, 3);
    assert!(h.app.map.tiles_missing > 0, "the odd columns stay grey");
    assert!(h.painted_text().join("\n").contains("© OpenStreetMap contributors"));
}

#[test]
fn offline_shows_a_note_and_never_starts_the_tile_service() {
    let (mut h, _) = demo(None);
    run(&mut h, "view.map", json!({}));
    frames(&mut h, 3);
    let text = h.painted_text().join("\n");
    assert!(text.contains("Map tiles are off (Settings → Map)"), "{text}");
    assert!(!text.contains("OpenStreetMap contributors"));
    assert!(h.app.session.map.service().is_none(), "no tile service, so no network");
    assert_eq!(state(&mut h)["offline"], true);
}

#[test]
fn hovering_a_pin_shows_its_thumbnail() {
    let (mut h, _) = demo(None);
    run(&mut h, "view.map", json!({}));
    run(&mut h, "map.setView", json!({"lat": 40.7484, "lon": -73.9857, "zoom": 12}));
    frames(&mut h, 2);
    let s = state(&mut h);
    let ny = clusters(&s).into_iter().find(|c| c["count"] == 1).unwrap();
    h.request("ui.move", json!({"x": ny["x"], "y": ny["y"]}), T);
    let shown = h.step_until(SETTLE, |h| h.app.widgets.iter().any(|(w, _)| w == "map:hoverThumb"));
    assert!(shown, "the thumbnail appears once rendered");
}

#[test]
fn hostile_positions_and_sizes_never_crash() {
    let (mut h, ids) = demo(None);
    // poles, the antimeridian, and a photo with no position at all
    for (id, gps) in ids.iter().zip([json!([90.0, 180.0]), json!([-90.0, -180.0]), json!([0.0, 180.0]), json!([0.0, -180.0]), json!(null)]) {
        h.app.run("photo.setMeta", json!({"ids": [id], "gps": gps})).unwrap();
    }
    run(&mut h, "view.map", json!({}));
    frames(&mut h, 2);
    let s = run(&mut h, "map.fit", json!({}));
    frames(&mut h, 2);
    assert!(s["lat"].as_f64().unwrap().is_finite() && s["zoom"].as_f64().unwrap().is_finite(), "{s}");
    for z in [0.0, 0.5, 19.0] {
        run(&mut h, "map.setView", json!({"lat": 85.0, "lon": 180.0, "zoom": z}));
        frames(&mut h, 2);
        for c in clusters(&state(&mut h)) {
            assert!(c["x"].as_f64().unwrap().is_finite() && c["y"].as_f64().unwrap().is_finite(), "{c}");
        }
    }
    // a tiny window, then a usual one
    h.request("ui.resize", json!({"width": 120, "height": 90}), T);
    frames(&mut h, 3);
    h.request("ui.resize", json!({"width": 1400, "height": 900}), T);
    frames(&mut h, 2);
    // no photo with a position at all
    for id in &ids {
        h.app.run("photo.setMeta", json!({"ids": [id], "gps": null})).unwrap();
    }
    frames(&mut h, 2);
    let s = run(&mut h, "map.fit", json!({}));
    assert_eq!(s["withGps"], 0);
    frames(&mut h, 2);
    assert!(h.painted_text().join("\n").contains("No photos with a location"));
}

// ---- the Info panel's mini-map

/// The demo library in a window tall enough for the whole Info panel, Info open on `ids[0]`.
fn info(tiles: Option<Arc<FakeTiles>>) -> (Headless, Vec<u64>) {
    let (mut h, ids) = demo_sized(tiles, [1400.0, 1700.0]);
    run(&mut h, "library.select", json!({"ids": [ids[0]]}));
    h.app.ui.right = crate::state::RightPanel::Info;
    frames(&mut h, 3);
    (h, ids)
}

fn mini(h: &mut Headless) -> Value {
    state(h)["miniMap"].clone()
}

pub(super) fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn tiles_idle(h: &Headless) -> bool {
    h.app.session.map.service().is_some_and(|s| s.is_idle())
}

#[test]
fn the_info_panel_maps_the_active_photo_and_credits_the_tiles() {
    let (mut h, ids) = info(Some(fake(false)));
    assert!(
        h.step_until(T, |h| h.app.map.mini.as_ref().is_some_and(|m| m.tiles_drawn > 0 && m.tiles_missing == 0) && tiles_idle(h)),
        "the mini-map's tiles arrive from the fake server"
    );
    frames(&mut h, 2);
    let m = mini(&mut h);
    assert_eq!(m["photo"].as_u64(), Some(ids[0]), "{m}");
    assert!(
        (m["lat"].as_f64().unwrap() - PLACES[0].0).abs() < 1e-9 && (m["lon"].as_f64().unwrap() - PLACES[0].1).abs() < 1e-9,
        "centred on the photo: {m}"
    );
    assert_eq!(m["zoom"].as_f64(), Some(crate::panels::map::MINI_ZOOM));
    // the pin is in the middle of the map, which takes the panel's width
    let r = widget(&h, "info:miniMap").expect("the mini-map is registered");
    let tools = widget(&h, "panel:right_panel").expect("the Info panel is docked in Tools");
    assert!(tools.contains_rect(r), "the mini-map {r:?} draws inside the docked Tools panel {tools:?}");
    let (px, py) = (m["pin"]["x"].as_f64().unwrap() as f32, m["pin"]["y"].as_f64().unwrap() as f32);
    assert!((px - r.center().x).abs() < 1.0 && (py - r.center().y).abs() < 1.0, "{m} {r:?}");
    assert!(r.width() > 180.0 && (r.height() - crate::panels::map::MINI_HEIGHT).abs() < 0.5, "{r:?}");
    // the provider is credited on the mini-map itself
    let credit = widget(&h, "info:miniMapAttribution").expect("the attribution is registered");
    assert!(r.contains_rect(credit), "{credit:?} inside {r:?}");
    assert!(h.painted_text().join("\n").contains("© OpenStreetMap contributors"));
    assert_ne!(h.app.ui.view, ViewMode::Map, "the view stays: only the panel shows a map");
    // the tiles are painted: the fake tile's colour away from the pin
    let img = h.snapshot(SETTLE);
    let px = img.pixels[(r.top() as usize + 30) * img.size[0] + r.left() as usize + 30];
    assert!(px.r() > 140 && px.g() > 150, "{px:?}");
    // several selected: the active photo's place, nothing fetched for the others
    run(&mut h, "library.select", json!({"ids": [ids[3], ids[0], ids[5]], "active": ids[3]}));
    frames(&mut h, 3);
    let m = mini(&mut h);
    assert_eq!(m["photo"].as_u64(), Some(ids[3]), "{m}");
    assert!((m["lat"].as_f64().unwrap() - PLACES[3].0).abs() < 1e-9, "{m}");
    let wanted = h.app.session.map.service().map(|s| s.stats().wanted).unwrap_or(0);
    assert!(wanted > 0 && wanted <= 12, "only the active photo's few tiles: {wanted}");
    // a photo without GPS: no map, the location fields as before
    let others: Vec<u64> = h.app.session.visible_cloned().iter().map(|p| p.0).filter(|id| !ids.contains(id)).collect();
    run(&mut h, "library.select", json!({"ids": [others[0]]}));
    frames(&mut h, 3);
    assert!(mini(&mut h).is_null());
    assert!(widget(&h, "info:miniMap").is_none());
    assert!(widget(&h, "field:gps").is_some(), "the GPS field is still there");
}

#[test]
fn clicking_the_mini_map_opens_the_map_view_there_with_the_same_tiles() {
    let (mut h, ids) = info(Some(fake(false)));
    assert!(h.step_until(T, |h| h.app.map.mini.as_ref().is_some_and(|m| m.tiles_missing == 0) && tiles_idle(h)));
    frames(&mut h, 2);
    let before = h.app.map.texture_keys();
    let uploads = h.app.map.uploads;
    assert!(!before.is_empty() && uploads as usize == before.len(), "{uploads} uploads for {} tiles", before.len());
    let r = widget(&h, "info:miniMap").unwrap();
    let c = h.request("ui.click", json!({"x": r.left() + 20.0, "y": r.top() + 20.0}), T);
    assert_eq!(c["ok"], true, "{c}");
    frames(&mut h, 3);
    assert_eq!(h.app.ui.view, ViewMode::Map);
    let s = state(&mut h);
    assert!((s["lat"].as_f64().unwrap() - PLACES[0].0).abs() < 1e-9 && (s["lon"].as_f64().unwrap() - PLACES[0].1).abs() < 1e-9, "{s}");
    assert_eq!(s["zoom"].as_f64(), Some(crate::panels::map::MINI_ZOOM), "{s}");
    assert_eq!(h.app.session.active().map(|a| a.0), Some(ids[0]), "the selection is untouched");
    // the Map view draws the tiles the mini-map uploaded without uploading them again
    assert!(h.step_until(T, |h| h.app.map.tiles_drawn > 0 && h.app.map.tiles_missing == 0 && tiles_idle(h)));
    frames(&mut h, 2);
    let after = h.app.map.texture_keys();
    assert!(before.is_subset(&after), "the mini-map's textures are kept");
    let new = after.difference(&before).count() as u64;
    assert!(new > 0, "the bigger view shows more tiles");
    assert_eq!(h.app.map.uploads - uploads, new, "only the newly shown tiles are uploaded");
    // the mini-map is still there next to the Map view, and both keep their tiles
    assert!(!mini(&mut h).is_null());
    let uploads = h.app.map.uploads;
    run(&mut h, "view.photoGrid", json!({}));
    frames(&mut h, 3);
    run(&mut h, "view.map", json!({}));
    frames(&mut h, 3);
    assert_eq!(h.app.map.uploads, uploads, "switching views uploads nothing again");
}

#[test]
fn the_mini_map_fetches_only_while_it_is_on_screen() {
    let tiles = fake(false);
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    app.session.map.set_service(TileService::with_transport(TileSource::openstreetmap(), None, Some(tiles.clone() as Arc<dyn Transport>)));
    app.ui.right = crate::state::RightPanel::Edit;
    let mut h = Headless::new(app, [1400.0, 1700.0], 1.0);
    h.settle(SETTLE);
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(2).map(|p| p.0).collect();
    h.app.run("photo.setMeta", json!({"ids": ids, "gps": [PLACES[0].0, PLACES[0].1]})).unwrap();
    run(&mut h, "library.select", json!({"ids": [ids[0]]}));
    frames(&mut h, 3);
    // no map on screen: nothing asked for, nothing fetched
    assert_eq!(tiles.requests.load(Ordering::SeqCst), 0);
    assert_eq!(h.app.session.map.service().map(|s| s.stats().wanted), Some(0));
    h.app.ui.right = crate::state::RightPanel::Info;
    assert!(h.step_until(T, |h| h.app.map.mini.as_ref().is_some_and(|m| m.tiles_missing == 0) && tiles_idle(h)));
    let svc = |h: &Headless| h.app.session.map.service().map(|s| s.stats()).unwrap();
    assert!(svc(&h).wanted > 0);
    // the panel closed (or another tool): the tiles it wanted are no longer asked for
    h.app.ui.right = crate::state::RightPanel::Edit;
    frames(&mut h, 3);
    assert!(mini(&mut h).is_null());
    assert_eq!((svc(&h).wanted, svc(&h).queued), (0, 0));
    let requests = tiles.requests.load(Ordering::SeqCst);
    // another photo while the panel is closed: nothing fetched for it
    h.app.run("photo.setMeta", json!({"ids": [ids[1]], "gps": [PLACES[5].0, PLACES[5].1]})).unwrap();
    run(&mut h, "library.select", json!({"ids": [ids[1]]}));
    frames(&mut h, 10);
    assert_eq!(tiles.requests.load(Ordering::SeqCst), requests);
}

#[test]
fn offline_the_mini_map_is_grey_with_its_pin_and_a_note() {
    let (mut h, _) = info(None);
    let m = mini(&mut h);
    assert!(!m.is_null(), "the map keeps its place in the panel");
    assert_eq!(m["tiles"]["drawn"], 0);
    let text = h.painted_text().join("\n");
    assert!(text.contains("Map tiles are off"), "{text}");
    assert!(!text.contains("OpenStreetMap contributors"));
    assert!(widget(&h, "info:miniMapAttribution").is_none());
    assert!(h.app.session.map.service().is_none(), "offline: no tile service, so no network");
    // the click still opens the Map view there
    let r = widget(&h, "info:miniMap").unwrap();
    h.request("ui.click", json!({"x": r.center().x, "y": r.center().y}), T);
    frames(&mut h, 3);
    assert_eq!(h.app.ui.view, ViewMode::Map);
    assert!((state(&mut h)["lat"].as_f64().unwrap() - PLACES[0].0).abs() < 1e-9);
}

#[test]
fn hostile_positions_narrow_windows_and_a_changing_photo_never_crash_the_mini_map() {
    let (mut h, ids) = info(Some(fake(false)));
    for gps in [[90.0, 180.0], [-90.0, -180.0], [0.0, 180.0], [0.0, -180.0], [85.06, 0.0], [-89.9999, 179.9999]] {
        h.app.run("photo.setMeta", json!({"ids": [ids[0]], "gps": gps})).unwrap();
        frames(&mut h, 2);
        let m = mini(&mut h);
        assert!(m["lat"].as_f64().unwrap().is_finite() && m["lon"].as_f64().unwrap().is_finite(), "{gps:?}: {m}");
        let r = widget(&h, "info:miniMap").unwrap();
        assert!(r.contains(egui::pos2(m["pin"]["x"].as_f64().unwrap() as f32, m["pin"]["y"].as_f64().unwrap() as f32)), "{gps:?}: {m}");
    }
    // NaN never reaches the catalog, and an out-of-range position is refused
    assert!(h.app.run("photo.setMeta", json!({"ids": [ids[0]], "gps": [95.0, 0.0]})).is_err());
    // the photo changing while its tiles load: the map follows the active photo
    for id in [ids[1], ids[5], ids[3], ids[5]] {
        run(&mut h, "library.select", json!({"ids": [id]}));
        h.step();
    }
    frames(&mut h, 3);
    assert_eq!(mini(&mut h)["photo"].as_u64(), Some(ids[5]));
    // a tiny window, a narrow one, then a usual one
    for (w, hh) in [(120, 90), (300, 1700), (1400, 1700)] {
        h.request("ui.resize", json!({"width": w, "height": hh}), T);
        frames(&mut h, 3);
    }
    assert!(widget(&h, "info:miniMap").is_some());
}

#[test]
fn the_mini_map_pin_stays_on_a_map_of_any_width() {
    use crate::panels::map::keep_on_map;
    use egui::{Rect, pos2};
    let wide = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(200.0, 160.0));
    assert_eq!(keep_on_map(pos2(-50.0, 300.0), wide), pos2(8.0, 152.0));
    assert_eq!(keep_on_map(pos2(f32::NAN, 10.0), wide), wide.center());
    // narrower than the 8-point margins on both sides: the centre line, not a panic
    for w in [1.0, 5.0, 15.9, 16.0] {
        let narrow = Rect::from_min_size(pos2(10.0, 0.0), egui::vec2(w, 160.0));
        let p = keep_on_map(pos2(500.0, 80.0), narrow);
        assert!((narrow.left()..=narrow.right()).contains(&p.x), "{w}: {p:?}");
    }
}

// ---- cluster counts, and no tile requests once the map is off the screen

#[test]
fn every_cluster_pin_shows_its_photo_count() {
    let (mut h, _) = demo(Some(fake(false)));
    run(&mut h, "view.map", json!({}));
    frames(&mut h, 3);
    // zoomed out far enough that Paris (3) and London (2) merge
    for zoom in [4.0, 1.0] {
        run(&mut h, "map.setView", json!({"zoom": zoom}));
        frames(&mut h, 3);
        let s = state(&mut h);
        let cs = clusters(&s);
        let big: Vec<&Value> = cs.iter().filter(|c| c["count"].as_u64().unwrap_or(0) > 1).collect();
        assert!(!big.is_empty(), "zoom {zoom}: some photos are grouped: {s}");
        let painted = h.painted_text();
        for c in big {
            let n = c["count"].as_u64().unwrap().to_string();
            assert!(painted.contains(&n), "zoom {zoom}: the pin of {n} photos shows {n}; painted {painted:?}");
        }
    }
    // at the lowest zoom Paris and London are one pin of 5 (New York is far enough to stay alone)
    let s = state(&mut h);
    assert!(clusters(&s).iter().any(|c| c["count"] == 5), "{s}");
}

/// A transport that counts the requests that start, can hold them until released, and can fail them.
struct Held {
    starts: AtomicUsize,
    open: AtomicBool,
    fail: bool,
    png: Vec<u8>,
}

impl Held {
    fn new(hold: bool, fail: bool) -> Arc<Held> {
        Arc::new(Held { starts: AtomicUsize::new(0), open: AtomicBool::new(!hold), fail, png: tile_png() })
    }
    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
}

impl Transport for Held {
    fn get(&self, _: &str, _: &[(&'static str, String)], _: usize, cancel: &AtomicBool) -> Result<HttpResponse, String> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        while !self.open.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.fail {
            return Err("boom".into());
        }
        Ok(HttpResponse { status: 200, headers: vec![("cache-control".into(), "max-age=604800".into())], body: self.png.clone() })
    }
}

fn map_with(t: &Arc<Held>) -> Headless {
    let (mut h, _) = demo(None);
    h.app.session.map.set_service(TileService::with_transport(TileSource::openstreetmap(), None, Some(t.clone() as Arc<dyn Transport>)));
    run(&mut h, "view.map", json!({}));
    h
}

/// Requests held on the wire when the map closes finish, and nothing queued behind them starts.
#[test]
fn no_tile_request_starts_after_the_map_leaves_the_screen() {
    let t = Held::new(true, false);
    let mut h = map_with(&t);
    assert!(h.step_until(T, |_| t.starts() == 2), "two requests on the wire, the rest queued");
    assert!(h.app.session.map.service().is_some_and(|s| s.stats().queued > 0), "more tiles are waiting");
    run(&mut h, "view.photoGrid", json!({}));
    frames(&mut h, 4);
    let s = h.app.session.map.service().unwrap().stats();
    assert_eq!((s.wanted, s.queued), (0, 0), "the queue is cleared when no map is drawn");
    t.open.store(true, Ordering::SeqCst);
    assert!(h.step_until(T, |h| h.app.session.map.service().is_none_or(|s| s.is_idle())));
    frames(&mut h, 10);
    assert_eq!(t.starts(), 2, "only the two in flight finished; no request started after the map closed");
}

/// Failing tiles are retried with a backoff while the map is open, and not at all after it closes.
#[test]
fn failing_tiles_are_not_retried_after_the_map_leaves_the_screen() {
    let t = Held::new(false, true);
    let mut h = map_with(&t);
    assert!(h.step_until(T, |_| t.starts() >= 2));
    run(&mut h, "view.photoGrid", json!({}));
    frames(&mut h, 4);
    assert!(h.step_until(T, |h| h.app.session.map.service().is_none_or(|s| s.stats().in_flight == 0)));
    let n = t.starts();
    // longer than the first backoff step (1 s), frames running all the time
    let end = std::time::Instant::now() + Duration::from_millis(2500);
    while std::time::Instant::now() < end {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(t.starts(), n, "no retry once the map is gone");
    // back on the map, the tiles are asked for again
    run(&mut h, "view.map", json!({}));
    assert!(h.step_until(Duration::from_secs(30), |_| t.starts() > n), "asked again when the map is back");
}

// ---- frame times with thousands of pins

/// The demo library plus `n` photos spread over Europe in towns and countryside (deterministic).
fn library_with_pins(n: u32) -> Headless {
    use lightcraft_catalog::{Op, Photo, Source};
    let mut s = lightcraft_engine::Session::with_demo();
    let mut ops = Vec::new();
    let mut seed = 0x9e37_79b9_u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(seed >> 8) / f64::from(1u32 << 24)
    };
    let towns: Vec<(f64, f64)> = (0..60).map(|_| (36.0 + next() * 24.0, -10.0 + next() * 40.0)).collect();
    for i in 0..n {
        let id = s.catalog.alloc_photo_id();
        let mut p = Photo::new(id, Source::Demo { scene: 0 }, &format!("P{i:05}.JPG"), "JPEG", 600, 400, "2026-09-28T09:30:00");
        let (clat, clon) = towns[(next() * towns.len() as f64) as usize % towns.len()];
        // two thirds in towns (a few km round), the rest anywhere
        p.meta.gps = Some(if next() < 0.66 {
            (clat + (next() - 0.5) * 0.3, clon + (next() - 0.5) * 0.4)
        } else {
            (36.0 + next() * 24.0, -10.0 + next() * 40.0)
        });
        ops.push(Op::AddPhoto { photo: Box::new(p) });
    }
    s.commit("Add pins", Op::Batch { ops }).unwrap();
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(s, services);
    app.session.map.set_service(TileService::with_transport(TileSource::openstreetmap(), None, Some(fake(false) as Arc<dyn Transport>)));
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(SETTLE);
    h
}

fn timed(h: &mut Headless) -> f64 {
    let t0 = std::time::Instant::now();
    h.step();
    t0.elapsed().as_secs_f64() * 1000.0
}

fn summary(what: &str, mut ms: Vec<f64>) -> (f64, f64, f64) {
    ms.sort_by(f64::total_cmp);
    let (med, p95, max) = (ms[ms.len() / 2], ms[ms.len() * 95 / 100], ms[ms.len() - 1]);
    eprintln!("FRAMES {what}: median {med:.2} ms, p95 {p95:.2} ms, max {max:.2} ms over {} frames", ms.len());
    (med, p95, max)
}

/// Frame time of the Map view with ~5,000 geotagged photos while it rests, pans and zooms, tiles from
/// the in-process fake server (no network). Run on demand, it is a measurement:
/// `cargo test -p lightcraft-ui-egui --lib -- --ignored --nocapture map_frame_times`.
/// What a frame includes: the app's logic and the whole UI pass (layout, clustering, shapes); not
/// the GPU's drawing.
#[test]
#[ignore = "a measurement: run on demand"]
fn map_frame_times_with_five_thousand_pins() {
    let mut h = library_with_pins(5000);
    // the same photos in the grid, for comparison: what the map adds is the difference
    run(&mut h, "view.photoGrid", json!({}));
    frames(&mut h, 20);
    let grid: Vec<f64> = (0..100).map(|_| timed(&mut h)).collect();
    summary("the photo grid resting (comparison)", grid);
    run(&mut h, "view.map", json!({}));
    assert!(h.step_until(SETTLE, |h| h.app.session.map.service().is_none_or(|s| s.is_idle())));
    frames(&mut h, 5);
    let s = state(&mut h);
    assert_eq!(s["withGps"], 5000);
    let idle: Vec<f64> = (0..100).map(|_| timed(&mut h)).collect();
    eprintln!(
        "FRAMES slow resting frames: {:?}",
        idle.iter().enumerate().filter(|(_, m)| **m > 16.0).map(|(i, m)| (i, *m as u32)).collect::<Vec<_>>()
    );
    let (_, idle_p95, _) = summary("resting", idle);
    // pan along a diagonal at zoom 6
    run(&mut h, "map.setView", json!({"lat": 47.0, "lon": 8.0, "zoom": 6}));
    frames(&mut h, 5);
    let mut pan = Vec::new();
    for i in 0..240 {
        let t = f64::from(i) / 240.0;
        run(&mut h, "map.setView", json!({"lat": 47.0 + t * 6.0, "lon": 8.0 + t * 12.0}));
        let up = h.app.map.uploads;
        let ms = timed(&mut h);
        if ms > 16.0 {
            eprintln!("FRAMES slow pan frame {i}: {ms:.0} ms, {} uploads, cluster {:.2} ms", h.app.map.uploads - up, h.app.map.cluster_ms);
        }
        pan.push(ms);
    }
    let (_, pan_p95, _) = summary("panning at zoom 6", pan);
    // zoom in and out around the same place, 2 to 13
    let mut zoom = Vec::new();
    for i in 0..240 {
        let t = f64::from(i) / 120.0;
        let z = 2.0 + 11.0 * (1.0 - (t - 1.0).abs()).clamp(0.0, 1.0);
        run(&mut h, "map.setView", json!({"lat": 47.0, "lon": 8.0, "zoom": z}));
        let up = h.app.map.uploads;
        let ms = timed(&mut h);
        if ms > 16.0 {
            eprintln!("FRAMES slow zoom frame {i}: {ms:.0} ms, {} uploads, cluster {:.2} ms", h.app.map.uploads - up, h.app.map.cluster_ms);
        }
        zoom.push(ms);
    }
    let (_, zoom_p95, _) = summary("zooming 2 to 13 and back", zoom);
    let s = state(&mut h);
    eprintln!("FRAMES clusterMs at the end: {}", s["clusterMs"]);
    assert!(idle_p95 < 16.0 && pan_p95 < 16.0 && zoom_p95 < 16.0, "over the 16 ms budget");
}
