//! The finished full-size picture in the loupe (#178): once it is there, zoom and pan render nothing; an edit never
//! shows the old one; going back to a photo reads it from disk.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::render::Slot;
use crate::tests_render_fixtures::RenderFixture;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(180);

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
}

fn view(h: &mut Headless, zoom: serde_json::Value, pan: [f32; 2]) {
    exec(h, "view.navigate", json!({"zoom": zoom, "pan": pan}));
    assert!(h.settle(SETTLE), "the view settles");
}

/// The loupe at Fit on the fixture's first photo, with full-size pictures on (and, with `disk`, a disk cache there).
fn at_fit(fixture: &RenderFixture, disk: Option<&std::path::Path>) -> Headless {
    let mut session = fixture.session();
    if let Some(dir) = disk {
        session.media.attach_disk_cache(dir, 4 << 30);
    }
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.app.renderer.full.enabled = true;
    let r = h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    // (the loupe's first frames: the photo is open at Fit before anything zooms)
    for _ in 0..3 {
        h.step();
    }
    h
}

/// [`at_fit`], zoomed into at 100 %: a photo the finished picture is made for.
fn loupe(fixture: &RenderFixture, disk: Option<&std::path::Path>) -> Headless {
    let mut h = at_fit(fixture, disk);
    look_closer(&mut h, [0.5, 0.5]);
    h
}

/// Zoom the photo open to 100 % with the centre at `pan` (without waiting for it to render).
fn look_closer(h: &mut Headless, pan: [f32; 2]) {
    exec(h, "view.navigate", json!({"zoom": {"percent": 100}, "pan": pan}));
    h.step();
}

/// Whether the activity stack shows the build's row.
fn row(h: &Headless) -> bool {
    h.app.session.activity.list().iter().any(|t| t.label == super::ROW)
}

/// Builds that ever ran at once in this app.
fn most_at_once(h: &Headless) -> usize {
    h.app.renderer.full.at_once.most.load(std::sync::atomic::Ordering::SeqCst)
}

/// Run frames until the photo's finished picture is up, with nothing being made for it.
fn until_ready(h: &mut Headless) {
    let ready = h.step_until(SETTLE, |h| h.app.renderer.full.ready() && !h.app.renderer.is_pending(Slot::Full));
    assert!(ready, "the finished picture comes: {}", h.app.renderer.full.to_json());
}

// Given a photo with a mask and spots whose finished picture is ready, when I zoom Fit → 50 % → 100 % → 200 %, pan
// anywhere and go back to Fit, then no render starts, and every zoomed view is drawn from the picture
#[test]
fn zoom_and_pan_draw_the_finished_picture_and_start_no_render() {
    let fixture = RenderFixture::new("full-picture-zoom", 1);
    let mut h = loupe(&fixture, None);
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.5, 0.1], "end": [0.5, 0.6]}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": -0.5}}));
    for (i, mode) in ["heal", "clone", "fill"].iter().enumerate() {
        let x = 0.42 + 0.04 * i as f64;
        exec(&mut h, "spot.add", json!({"mode": mode, "points": [[x, 0.5]], "size": 0.006, "source": [0.0, 0.03]}));
    }
    // while it renders, the activity stack has its row (looked for from the first frame: a settle could run it whole)
    assert!(h.step_until(SETTLE, row), "the build's activity row");
    until_ready(&mut h);
    assert!(!row(&h), "no row once it is made");
    assert_eq!(h.app.renderer.full.stats.rendered, 1);
    let started = h.app.renderer.started;
    let steps: [(&str, serde_json::Value, [f32; 2]); 8] = [
        ("50 %", json!({"percent": 50}), [0.5, 0.5]),
        ("100 %", json!({"percent": 100}), [0.5, 0.5]),
        ("200 %", json!({"percent": 200}), [0.5, 0.5]),
        ("a pan to a corner", json!({"percent": 200}), [0.05, 0.05]),
        ("a pan to the other", json!({"percent": 200}), [0.95, 0.95]),
        ("100 % there", json!({"percent": 100}), [0.95, 0.95]),
        ("Fit", json!("fit"), [0.5, 0.5]),
        ("100 % again", json!({"percent": 100}), [0.3, 0.7]),
    ];
    for (name, zoom, pan) in steps {
        view(&mut h, zoom, pan);
        let fit = name == "Fit";
        assert_eq!(h.app.renderer.full.on_screen, !fit, "{name}: drawn from the finished picture");
        assert!(h.app.region_view.is_none(), "{name}: no window render");
        assert_eq!(h.app.renderer.started, started, "{name}: no render started");
    }
}

// Given the finished picture of a photo, when I change Exposure and zoom in at once, then the old picture is never
// drawn: the zoomed view renders its window as before, and the new picture follows
#[test]
fn an_edit_then_a_zoom_never_draws_the_old_picture() {
    let fixture = RenderFixture::new("full-picture-edit", 1);
    let mut h = loupe(&fixture, None);
    assert!(h.settle(SETTLE), "Fit settles");
    until_ready(&mut h);
    let (_, old) = h.app.renderer.full.held().expect("a picture");
    let started = h.app.renderer.started;
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.7}}));
    exec(&mut h, "view.navigate", json!({"zoom": {"percent": 100}, "pan": [0.5, 0.5]}));
    // every frame on the way to the new picture: the old one is never what the loupe drew
    let drew_old = std::cell::Cell::new(0);
    let ready = h.step_until(SETTLE, |h| {
        let full = &h.app.renderer.full;
        if full.on_screen && full.held().is_some_and(|(_, k)| k == old) {
            drew_old.set(drew_old.get() + 1);
        }
        full.ready() && full.held().is_some_and(|(_, k)| k != old)
    });
    assert!(ready, "the new picture comes: {}", h.app.renderer.full.to_json());
    assert_eq!(drew_old.get(), 0, "frames drawn from the old picture");
    assert!(h.app.renderer.started > started + 1, "the edit and the zoomed view rendered meanwhile");
    view(&mut h, json!({"percent": 200}), [0.5, 0.5]);
    assert!(h.app.renderer.full.on_screen, "the new picture is drawn");
}

// Given a photo whose finished picture was made and kept on disk, when I go to another photo and back, then it is
// read from disk and nothing renders it again; and once its settings change, the file of the old look is not used
#[test]
fn going_back_to_a_photo_reads_its_picture_from_disk_and_a_new_look_does_not() {
    let fixture = RenderFixture::new("full-picture-disk", 2);
    let dir = std::env::temp_dir().join(format!("lc-full-picture-{}-{}", std::process::id(), lightcraft_preview::next_tick()));
    let mut h = loupe(&fixture, Some(&dir));
    let first = h.app.session.active().expect("a photo");
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.4}}));
    assert!(h.settle(SETTLE), "Fit settles");
    until_ready(&mut h);
    let written = |h: &Headless| h.app.session.media.rendered.disk().map_or(0, |d| d.full_writes.load(std::sync::atomic::Ordering::Relaxed));
    assert!(h.step_until(SETTLE, |h| written(h) >= 1), "the picture is written to disk");
    exec(&mut h, "library.next", json!({}));
    assert_ne!(h.app.session.active(), Some(first));
    look_closer(&mut h, [0.4, 0.4]);
    assert!(h.settle(SETTLE), "the other photo settles");
    until_ready(&mut h);
    let stats = h.app.renderer.full.stats;
    assert_eq!((stats.rendered, stats.from_disk), (2, 0), "{stats:?}");
    exec(&mut h, "library.previous", json!({}));
    assert_eq!(h.app.session.active(), Some(first));
    assert!(h.settle(SETTLE), "back on the first photo");
    until_ready(&mut h);
    let stats = h.app.renderer.full.stats;
    assert_eq!((stats.rendered, stats.from_disk), (2, 1), "read back, not rendered: {stats:?}");
    look_closer(&mut h, [0.6, 0.6]);
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": -0.4}}));
    assert!(h.settle(SETTLE), "the edit settles");
    until_ready(&mut h);
    let stats = h.app.renderer.full.stats;
    assert_eq!((stats.rendered, stats.from_disk), (3, 1), "the new look is rendered, not read: {stats:?}");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

// Given a build of the finished picture under way, when I change Exposure, then it stops: nothing of it is kept, no
// second build runs beside it, and the picture that follows is the new look's
#[test]
fn an_edit_stops_the_build_under_way_and_no_two_run_at_once() {
    let fixture = RenderFixture::new("full-picture-stop", 1);
    let mut h = loupe(&fixture, None);
    assert!(h.step_until(SETTLE, row), "a build starts");
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.6}}));
    until_ready(&mut h);
    let stats = h.app.renderer.full.stats;
    assert_eq!((stats.dropped, stats.rendered), (1, 1), "the first build was dropped, the second kept: {stats:?}");
    assert_eq!(most_at_once(&h), 1, "never two builds at once");
}

// Given the finished picture, or a build under way, when the loupe is closed (the grid), then the picture goes once the
// loupe has been closed a while, and a build that comes back to a closed loupe is not kept
#[test]
fn a_closed_loupe_lets_go_of_the_picture_and_of_a_build_that_comes_back() {
    let fixture = RenderFixture::new("full-picture-closed", 1);
    let mut h = loupe(&fixture, None);
    assert!(h.settle(SETTLE), "Fit settles");
    until_ready(&mut h);
    let r = h.request("ui.set", json!({"view": "photoGrid"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(SETTLE, |h| !h.app.renderer.full.holds_anything()), "the picture goes with the loupe closed");
    // a build under way when the loupe closes: what comes back is not kept
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.3}}));
    assert!(h.step_until(SETTLE, row), "a build starts");
    let r = h.request("ui.set", json!({"view": "photoGrid"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(SETTLE, |h| !h.app.renderer.full.holds_anything()), "the build ends");
    assert_eq!(h.app.renderer.full.held(), None, "nothing held with the loupe closed");
    assert_eq!(h.app.renderer.full.bytes(), 0);
}

// Given the finished picture, when memory is short (the stage caches are trimmed for memory), then it goes first, and
// it is not made again for the same look
#[test]
fn short_memory_lets_go_of_the_picture() {
    let fixture = RenderFixture::new("full-picture-memory", 1);
    let mut h = loupe(&fixture, None);
    assert!(h.settle(SETTLE), "Fit settles");
    until_ready(&mut h);
    h.app.renderer.stage_budget_override = Some(1 << 20);
    for _ in 0..30 {
        h.step();
    }
    let full = &h.app.renderer.full;
    assert_eq!((full.held(), full.stats.freed_for_memory, full.holds_anything()), (None, 1, false), "{}", full.to_json());
}

// Given a photo open in the Edit view whose original is being read ahead (#182), when its finished picture is due,
// then the build waits for the read-ahead and renders from the original it decoded: the file is decoded once
#[test]
fn the_build_waits_for_the_read_ahead_and_decodes_nothing_again() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let fixture = RenderFixture::new("full-picture-read-ahead", 1);
    let mut session = fixture.session();
    let plain = session.media.file_loader.clone().expect("the fixture session reads files");
    let full = Arc::new(AtomicUsize::new(0));
    let held = Arc::new(AtomicBool::new(true));
    let (f, hold) = (full.clone(), held.clone());
    session.media.file_loader = Some(Arc::new(move |path: &str, max_edge, options, with| {
        if max_edge == usize::MAX {
            f.fetch_add(1, Ordering::SeqCst);
            // (a guard against a hung test, not a timing)
            let until = std::time::Instant::now() + SETTLE;
            while hold.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        plain(path, max_edge, options, with)
    }));
    session.media.set_budget(2 << 30);
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.app.renderer.full.enabled = true;
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    let id = h.app.session.active().expect("a photo");
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.running_for(id)), "the original is read ahead");
    // (zoomed into while it is read: the zoom's window waits for it, and the photo is one the picture is made for)
    look_closer(&mut h, [0.5, 0.5]);
    // well past the build's settle time: it waits while the read-ahead runs
    for _ in 0..240 {
        h.step();
        assert!(!h.app.renderer.full.renders(id), "no build while the original is read ahead");
    }
    held.store(false, Ordering::SeqCst);
    until_ready(&mut h);
    assert_eq!(full.load(Ordering::SeqCst), 1, "the original is decoded once");
    assert_eq!(h.app.renderer.full.stats.rendered, 1);
}

// Given a build waiting at the memory gate (AI Denoise holds it for its whole run), when the look changes, then the
// build stops there at once instead of waiting for the gate, and the new look's picture is built once the gate frees
//
// (The gate is this app's own, not the process's: every test of the crate shares one process, and holding its gate for
// as long as a failing test waits held up the loads of every other test running then. The build is waited for at the
// gate, not only for its row: a build still queued when the look changes is dropped from the queue, which ends
// nothing at the gate and is not counted.)
#[test]
fn a_build_waiting_at_the_memory_gate_stops_when_the_look_changes() {
    let fixture = RenderFixture::new("full-picture-gate", 1);
    let mut h = loupe(&fixture, None);
    let gate: &'static lightcraft_engine::memory::WorkGate = Box::leak(Box::new(lightcraft_engine::memory::WorkGate::new(1 << 30)));
    h.app.renderer.full.gate = Some(gate);
    let held = gate.acquire(gate.usage().1);
    assert!(h.step_until(SETTLE, row), "a build is asked for");
    let at_once = h.app.renderer.full.at_once.clone();
    assert!(h.step_until(SETTLE, |_| at_once.now.load(std::sync::atomic::Ordering::SeqCst) == 1), "the build runs, and waits at the gate");
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.5}}));
    let stopped = h.step_until(SETTLE, |h| h.app.renderer.full.stats.dropped >= 1);
    drop(held);
    assert!(stopped, "the stopped build ended with the gate still held: {}", h.app.renderer.full.to_json());
    until_ready(&mut h);
    assert_eq!(h.app.renderer.full.stats.rendered, 1, "the new look's picture");
}

// Given a build whose cut panics, when it fails, then its row ends and nothing is held, and the next look builds
#[test]
fn a_build_that_panics_ends_its_row_and_the_next_look_builds() {
    let fixture = RenderFixture::new("full-picture-panic", 1);
    let mut h = loupe(&fixture, None);
    h.app.renderer.full.fail_next = true;
    assert!(h.step_until(SETTLE, row), "a build starts");
    assert!(h.step_until(SETTLE, |h| !h.app.renderer.full.holds_anything()), "the failed build ends");
    assert!(!row(&h), "its row ends");
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.4}}));
    until_ready(&mut h);
    assert_eq!(h.app.renderer.full.stats.rendered, 1);
}

/// A small photo full of fine detail (leaves of noise, thin lines, hard edges), as a PNG in `dir`: `w × h` px.
fn detailed_photo(dir: &std::path::Path, w: u32, h: u32) -> String {
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for y in 0..h {
        for x in 0..w {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let noise = (seed & 0x3f) as f32 - 32.0;
            let lines = if (x / 3 + y / 5) % 7 == 0 { 70.0 } else { 0.0 };
            let block = if (x / 96 + y / 64) % 2 == 0 { 40.0 } else { -20.0 };
            let base = 90.0 + 60.0 * (x as f32 / w as f32) + block + lines;
            let c = |v: f32| v.clamp(0.0, 255.0) as u8;
            bytes.extend_from_slice(&[c(base + noise), c(base * 0.8 + noise * 0.5 + 30.0 * (y as f32 / h as f32)), c(base * 0.6 - noise), 255]);
        }
    }
    let image = lightcraft_codecs::EncodeImage::new(w, h, 4, lightcraft_codecs::Samples::U8(&bytes));
    let png = lightcraft_codecs::encode_png(&image, &lightcraft_codecs::EncodeMeta::default()).expect("encode the PNG");
    let path = dir.join("detailed.png");
    std::fs::write(&path, png).expect("write the PNG");
    path.to_string_lossy().into_owned()
}

// Given a photo with strong Clarity, Dehaze and Texture, a feathered mask and a spot, when its finished picture is
// built, then its full size is the whole frame's render pixel for pixel, across every texture edge: what zoom and pan
// show is what the photo exports as (bands of window renders, before #199, were up to 21 codes off it)
#[test]
fn the_picture_is_the_whole_frame_render_pixel_for_pixel() {
    let dir = std::env::temp_dir().join(format!("lc-full-whole-{}-{}", std::process::id(), lightcraft_preview::next_tick()));
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    let size = [1536usize, 1024];
    let path = detailed_photo(&dir, size[0] as u32, size[1] as u32);
    let mut session = lightcraft_engine::Session::new().with_fs();
    let id = session.catalog.alloc_photo_id();
    let photo =
        lightcraft_catalog::Photo::new(id, lightcraft_catalog::Source::File { path }, "DETAILED.PNG", "PNG", 1536, 1024, "2026-01-01T10:00:00");
    session.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(photo) }).expect("add the photo");
    session.keep_active_in_view();
    session.selection = lightcraft_engine::Selection::single(id);
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "develop.set", json!({"values": {"effects.clarity": 100, "effects.dehaze": 80, "effects.texture": 100}}));
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.333], "feather": 80}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": 1.0, "clarity": 60}}));
    exec(&mut h, "spot.add", json!({"mode": "heal", "points": [[0.4, 0.666]], "size": 0.03, "source": [0.0, 0.08]}));
    assert!(h.settle(SETTLE), "the edits settle");
    let whole = super::picture_job(&mut h.app, id, size, true).expect("the picture's job");
    let reference = whole.clone().run().rendered.expect("the whole frame").image;
    // (textures of 500 px: the full size is cut into 4 × 3 of them, each with a pixel of its neighbours)
    let plan = super::Plan {
        disk: None,
        size,
        tile: 500,
        min_edge: size[0],
        render: true,
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        at_once: Arc::default(),
        writing: Arc::default(),
        fail_cut: false,
        gate: None,
    };
    let built = super::work(whole, plan).1.expect("the picture");
    assert_eq!(built.levels.first(), Some(&size));
    let full: Vec<_> = built.tiles.iter().filter(|t| t.level == 0).collect();
    assert_eq!(full.len(), 12, "textures of the full size");
    let mut seen = vec![0u8; size[0] * size[1]];
    let mut differ = 0usize;
    for t in full {
        let [x0, y0, tw, th] = t.rect;
        for y in 0..th {
            for x in 0..tw {
                let got = t.image.pixels[(y + t.border[1]) * t.image.size[0] + x + t.border[0]];
                let want = reference.data[(y0 + y) * size[0] + x0 + x];
                differ += usize::from(got.to_array() != want);
                seen[(y0 + y) * size[0] + x0 + x] += 1;
            }
        }
    }
    assert_eq!(differ, 0, "pixels that differ from the whole frame's render");
    assert!(seen.iter().all(|&n| n == 1), "every pixel in exactly one texture");
    let _ = std::fs::remove_dir_all(&dir);
}

// Given photos culled with the arrow keys, when they are only stepped past (at Fit, or at a zoom kept from the photo
// before), then no full-size picture is rendered for them, however long each stays on screen; once a photo is zoomed
// into or panned, its picture is built
#[test]
fn photos_only_stepped_past_render_no_picture() {
    let fixture = RenderFixture::new("full-picture-culling", 3);
    let mut h = at_fit(&fixture, None);
    // well past the settle time on each, the loupe sharp and at rest
    let linger = |h: &mut Headless, what: &str| {
        assert!(h.settle(SETTLE), "{what}: the photo settles");
        let id = h.app.session.active().expect("a photo");
        for _ in 0..180 {
            h.step();
            assert!(!h.app.renderer.full.renders(id) && !row(h), "{what}: no build");
        }
    };
    linger(&mut h, "the first photo at Fit");
    assert_eq!(h.app.renderer.full.stats.rendered, 0, "the first photo at Fit");
    exec(&mut h, "library.next", json!({}));
    linger(&mut h, "the second photo at Fit");
    assert_eq!(h.app.renderer.full.stats.rendered, 0);
    look_closer(&mut h, [0.5, 0.5]);
    until_ready(&mut h);
    assert_eq!(h.app.renderer.full.stats.rendered, 1, "the photo zoomed into");
    exec(&mut h, "library.next", json!({}));
    linger(&mut h, "the third photo, at the zoom kept from the second");
    assert_eq!(h.app.renderer.full.stats.rendered, 1);
    look_closer(&mut h, [0.3, 0.6]);
    until_ready(&mut h);
    assert_eq!(h.app.renderer.full.stats.rendered, 2, "the photo panned");
}

// Given a photo opened at the zoom kept from the photo before, when I press Z to Fit and Z again, back to that same
// zoom and pan, then it counts as zoomed into: its picture is built
#[test]
fn z_to_fit_and_back_on_a_photo_opened_zoomed_builds_its_picture() {
    let fixture = RenderFixture::new("full-picture-z-toggle", 2);
    let mut h = at_fit(&fixture, None);
    let z = |h: &mut Headless| {
        let r = h.request("ui.key", json!({"key": "z"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
    };
    z(&mut h);
    let kept = (h.app.ui.zoom, h.app.ui.pan);
    assert_ne!(kept.0, crate::state::Zoom::Fit, "Z zooms in");
    exec(&mut h, "library.next", json!({}));
    assert!(h.settle(SETTLE), "the second photo settles, at the zoom kept");
    let id = h.app.session.active().expect("a photo");
    for _ in 0..120 {
        h.step();
        assert!(!h.app.renderer.full.renders(id), "stepped to at a kept zoom: no build");
    }
    let rendered = h.app.renderer.full.stats.rendered;
    z(&mut h);
    assert_eq!(h.app.ui.zoom, crate::state::Zoom::Fit);
    assert!(h.settle(SETTLE), "Fit settles");
    z(&mut h);
    assert_eq!((h.app.ui.zoom, h.app.ui.pan), kept, "back to the same zoom and pan");
    until_ready(&mut h);
    assert_eq!(h.app.renderer.full.stats.rendered, rendered + 1, "the photo zoomed into with Z");
}
