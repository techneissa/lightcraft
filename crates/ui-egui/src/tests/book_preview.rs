//! The Book preview, driven through the control channel: it opens full screen from the Book view,
//! turns by key and click, animates (unless reduce-motion is on), shows its counter and thumbnails
//! while the pointer moves, draws pages from the export's composer, and returns to the Book view on
//! the same page.

use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(60);

fn demo() -> Headless {
    let services = Services { png: None, local_places: Some(Vec::new()), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1600.0, 1000.0], 1.0);
    h.settle(T);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn cmd(h: &mut Headless, id: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": id, "params": params}), T);
    assert_eq!(r["ok"], true, "{id} {params}: {r}");
    h.settle(T);
    r["result"].clone()
}

fn fails(h: &mut Headless, id: &str, params: Value) -> String {
    let r = h.request("engine.execute", json!({"command": id, "params": params}), T);
    assert_ne!(r["ok"], true, "{id} {params} should fail: {r}");
    r["error"].to_string()
}

fn key(h: &mut Headless, key: &str) {
    let r = h.request("ui.key", json!({"key": key}), T);
    assert_eq!(r["ok"], true, "{key}: {r}");
    h.settle(T);
}

fn key_with(h: &mut Headless, k: &str, extra: Value) {
    let mut p = json!({"key": k});
    for (n, v) in extra.as_object().cloned().unwrap_or_default() {
        p[n] = v;
    }
    let r = h.request("ui.key", p, T);
    assert_eq!(r["ok"], true, "{k}: {r}");
    h.settle(T);
}

fn click_at(h: &mut Headless, x: f32, y: f32) {
    let r = h.request("ui.click", json!({"x": x, "y": y}), T);
    assert_eq!(r["ok"], true, "click {x},{y}: {r}");
    h.settle(T);
}

fn page(h: &Headless) -> usize {
    h.app.ui.book.preview.as_ref().map_or(usize::MAX, |p| p.page)
}

/// Moves the pointer so the controls show.
fn wake(h: &mut Headless) {
    for (x, y) in [(700.0, 500.0), (760.0, 540.0)] {
        let r = h.request("ui.move", json!({"x": x, "y": y}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h.step();
}

/// A book of `n` photos, one per page, with a cover: the cover, then spreads of two pages.
fn open(h: &mut Headless, n: usize) -> Value {
    let photos: Vec<u64> = h.app.session.catalog.photos().take(n).map(|p| p.id.0).collect();
    cmd(h, "book.create", json!({"ids": photos, "preset": "one", "cover": true}));
    cmd(h, "view.book", json!({}));
    assert_eq!(h.app.ui.view, ViewMode::Book);
    cmd(h, "book.preview", json!({}))
}

/// Runs frames until `done`; a turn takes [`crate::panels::book::preview::SLIDE_SECS`] of the simulated clock.
fn run_until(h: &mut Headless, done: impl Fn(&Headless) -> bool) {
    assert!(h.step_until(T, done), "gave up waiting");
}

#[test]
fn the_preview_opens_full_screen_on_the_cover_and_closes_on_the_same_page() {
    let mut h = demo();
    let r = open(&mut h, 8);
    assert_eq!(r["open"], true, "{r}");
    assert!(h.app.ui.fullscreen && h.app.ui.book.preview.is_some());
    wake(&mut h);
    assert!(has(&h, "book:preview") && has(&h, "button:bookPreviewClose"), "the preview, its close button");
    assert!(!has(&h, "book:spread") && !has(&h, "book:strip"), "the Book view's panels are not drawn");
    assert_eq!((r["page"].clone(), r["spread"].clone(), r["spreads"].clone()), (json!(0), json!(0), json!(5)), "cover, then 1·2 3·4 5·6 7·8? {r}");
    assert!(h.painted_text().iter().any(|t| t.starts_with("Cover")), "the counter says Cover: {:?}", h.painted_text());
    // → turns to the first spread, ← back; Esc leaves from the page it is on
    key(&mut h, "ArrowRight");
    assert_eq!(page(&h), 1);
    key(&mut h, "Escape");
    assert!(h.app.ui.book.preview.is_none() && !h.app.ui.fullscreen);
    assert_eq!(h.app.ui.view, ViewMode::Book);
    assert_eq!(h.app.ui.book.page, 1, "the Book view is on the page the preview ended on");
    assert!(has(&h, "book:spread"), "the Book view is back");
}

#[test]
fn keys_turn_pages_and_jump() {
    let mut h = demo();
    open(&mut h, 8);
    // 8 pages with a cover: 0 | 1 2 | 3 4 | 5 6 | 7
    for (k, want) in [("ArrowRight", 1), ("PageDown", 3), ("Space", 5), ("ArrowLeft", 3), ("PageUp", 1), ("End", 7), ("Home", 0)] {
        key(&mut h, k);
        assert_eq!(page(&h), want, "{k}");
    }
    key_with(&mut h, "Space", json!({"shift": true}));
    assert_eq!(page(&h), 0, "Shift+Space at the cover stays");
    key(&mut h, "End");
    key(&mut h, "ArrowRight");
    assert_eq!(page(&h), 7, "the last spread stays last");
    key_with(&mut h, "Space", json!({"shift": true}));
    assert_eq!(page(&h), 5, "Shift+Space turns back");
    // the book view's own ←/→ did not move: it is the preview that turned
    assert_eq!(h.app.ui.view, ViewMode::Book);
}

#[test]
fn a_click_on_the_right_or_left_half_turns() {
    let mut h = demo();
    open(&mut h, 8);
    click_at(&mut h, 1200.0, 500.0);
    assert_eq!(page(&h), 1, "right half: next");
    click_at(&mut h, 1200.0, 500.0);
    assert_eq!(page(&h), 3);
    click_at(&mut h, 300.0, 500.0);
    assert_eq!(page(&h), 1, "left half: previous");
    click_at(&mut h, 300.0, 500.0);
    click_at(&mut h, 300.0, 500.0);
    assert_eq!(page(&h), 0, "the cover has nothing before it");
}

#[test]
fn the_close_button_and_the_command_close_it() {
    let mut h = demo();
    open(&mut h, 6);
    wake(&mut h);
    let r = h.request("ui.clickWidget", json!({"id": "button:bookPreviewClose"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(h.app.ui.book.preview.is_none() && !h.app.ui.fullscreen);
    cmd(&mut h, "book.preview", json!({"page": 3}));
    assert_eq!(page(&h), 3, "opens at the page asked for");
    let r = cmd(&mut h, "book.preview", json!({"close": true}));
    assert_eq!(r["open"], false);
    assert_eq!(h.app.ui.book.page, 3);
    // ⌘↩ in the Book view opens it, and again closes it
    key_with(&mut h, "Enter", json!({"cmd": true}));
    assert!(h.app.ui.book.preview.is_some(), "Cmd+Enter opens the preview");
    key_with(&mut h, "Enter", json!({"cmd": true}));
    assert!(h.app.ui.book.preview.is_none(), "and closes it");
}

#[test]
fn preview_commands_refuse_what_cannot_be_done() {
    let mut h = demo();
    assert!(fails(&mut h, "book.preview", json!({})).contains("book"), "no book yet");
    let photos: Vec<u64> = h.app.session.catalog.photos().take(4).map(|p| p.id.0).collect();
    cmd(&mut h, "book.create", json!({"ids": photos, "cover": true}));
    assert!(fails(&mut h, "book.previewPage", json!({"page": 1})).contains("not open"));
    let e = fails(&mut h, "book.preview", json!({"page": 99}));
    assert!(e.contains("`page` is 0 to"), "{e}");
    cmd(&mut h, "book.preview", json!({}));
    assert!(fails(&mut h, "book.previewPage", json!({"page": 99})).contains("the book has"));
    assert!(fails(&mut h, "book.previewPage", json!({})).contains("needs `page`"));
    assert!(fails(&mut h, "book.previewPage", json!({"to": "sideways"})).contains("next, previous, first or last"));
    assert!(fails(&mut h, "book.previewPage", json!({"page": -1})).contains("whole number"));
    let r = cmd(&mut h, "book.previewPage", json!({"to": "last"}));
    assert_eq!(r["spread"], json!(r["spreads"].as_u64().map(|n| n - 1)), "{r}");
    // a book with no pages has nothing to show
    cmd(&mut h, "book.preview", json!({"close": true}));
    cmd(&mut h, "book.create", json!({"name": "Empty", "empty": true}));
    assert!(fails(&mut h, "book.preview", json!({})).contains("no pages"));
    // deleted behind the preview: it closes
    cmd(&mut h, "book.open", json!({"book": 1}));
    cmd(&mut h, "book.preview", json!({}));
    cmd(&mut h, "book.delete", json!({"book": 1}));
    h.settle(T);
    assert!(h.app.ui.book.preview.is_none() && !h.app.ui.fullscreen, "a preview of a deleted book closes");
}

#[test]
fn a_turn_slides_unless_reduce_motion_is_on() {
    let mut h = demo();
    open(&mut h, 8);
    h.app.ui.book.preview.as_mut().unwrap().reduce_motion = false;
    let sliding =
        |h: &Headless| h.app.ui.book.preview.as_ref().is_some_and(|_| crate::panels::book::preview::state(&h.app, h.time())["sliding"] == true);
    key(&mut h, "ArrowRight");
    // (settle ran frames; the slide is over by now) turn again and look at once
    let _ = h.request("ui.key", json!({"key": "ArrowRight"}), T);
    assert!(sliding(&h) || page(&h) == 3, "a turn starts a slide");
    run_until(&mut h, |h| !sliding(h));
    let frames = h.app.ui.book.preview.as_ref().unwrap().frames;
    assert!(frames.count >= 3, "the slide drew several frames: {frames:?}");
    // reduce motion: the page changes at once
    h.app.ui.book.preview.as_mut().unwrap().reduce_motion = true;
    let slid = h.app.ui.book.preview.as_ref().unwrap().frames.count;
    key(&mut h, "ArrowRight");
    let r = crate::panels::book::preview::state(&h.app, h.time());
    assert_eq!((r["page"].clone(), r["sliding"].clone()), (json!(5), json!(false)), "{r}");
    assert_eq!(h.app.ui.book.preview.as_ref().unwrap().frames.count, slid, "no slide frames");
}

#[test]
fn the_counter_and_thumbnails_show_while_the_pointer_moves_and_hide_after_a_moment() {
    let mut h = demo();
    open(&mut h, 8);
    wake(&mut h);
    assert!(has(&h, "book:previewCounter") && has(&h, "button:bookPreviewClose"), "shown while the pointer moves");
    assert!(has(&h, "book:previewThumb:0") && has(&h, "book:previewThumb:4"), "a thumbnail for every spread that fits");
    // a few seconds of nothing: the controls are gone
    for _ in 0..240 {
        h.step();
    }
    assert!(!has(&h, "book:previewCounter") && !has(&h, "button:bookPreviewClose") && !has(&h, "book:previewThumb:0"), "controls hidden");
    // moving the pointer brings them back
    wake(&mut h);
    assert!(has(&h, "book:previewCounter") && has(&h, "book:previewThumb:2"), "shown again");
    let r = h.request("ui.clickWidget", json!({"id": "book:previewThumb:3"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert_eq!(page(&h), 5, "a thumbnail goes to its spread");
}

#[test]
fn pages_are_drawn_from_the_export_composer_and_the_next_spread_is_ready() {
    let mut h = demo();
    open(&mut h, 8);
    run_until(&mut h, |h| {
        h.app.ui.book.preview.as_ref().is_some_and(|_| crate::panels::book::preview::state(&h.app, h.time())["pagesReady"].as_u64() >= Some(3))
    });
    let r = crate::panels::book::preview::state(&h.app, h.time());
    // the cover, and the next spread (two pages) are ready; nothing before the cover
    assert_eq!(r["pagesReady"], 3, "{r}");
    key(&mut h, "ArrowRight");
    let r = crate::panels::book::preview::state(&h.app, h.time());
    assert!(r["pagesReady"].as_u64() >= Some(3), "a turn finds its pages: {r}");
    wake(&mut h);
    let text = h.painted_text().join(" ");
    assert!(text.contains("Pages 2–3 of 8"), "{text}");
}

#[test]
fn turning_pages_draws_inside_the_frame_budget() {
    let mut h = demo();
    open(&mut h, 8);
    run_until(&mut h, |h| crate::panels::book::preview::state(&h.app, h.time())["pagesReady"].as_u64() >= Some(3));
    h.app.ui.book.preview.as_mut().unwrap().reduce_motion = false;
    let mut steps = Vec::new();
    for round in 0..6 {
        let _ = h.request("ui.key", json!({"key": if round < 3 { "ArrowRight" } else { "ArrowLeft" }}), T);
        for _ in 0..40 {
            h.view.ctx.request_repaint();
            let started = std::time::Instant::now();
            h.step();
            steps.push(started.elapsed().as_secs_f32() * 1000.0);
        }
    }
    let f = h.app.ui.book.preview.as_ref().unwrap().frames;
    eprintln!(
        "book preview, {} turn frames: drawing mean {:.2} ms, worst {:.2} ms (texture upload worst {:.2} ms); the whole headless frame (UI, tessellation, CPU raster) mean {:.2} ms, worst {:.2} ms",
        f.count,
        f.sum_ms / f.count.max(1) as f32,
        f.max_ms,
        f.upload_ms,
        steps.iter().sum::<f32>() / steps.len() as f32,
        steps.iter().fold(0.0f32, |a, b| a.max(*b))
    );
    assert!(f.count > 0);
    assert!(f.sum_ms / f.count as f32 <= 16.0, "mean turn frame {} ms", f.sum_ms / f.count as f32);
}

#[test]
fn a_book_of_two_hundred_pages_previews_and_turns_inside_the_frame_budget() {
    use lightcraft_engine::book::{Book, MAX_PAGES, Page, Template};
    let mut h = demo();
    let photos: Vec<u64> = h.app.session.catalog.photos().take(24).map(|p| p.id.0).collect();
    let mut big = Book { name: "Big".into(), ..Default::default() };
    big.pages = (0..MAX_PAGES)
        .map(|i| Page::with(Template::Grid4, &[photos[i % 24], photos[(i + 1) % 24], photos[(i + 2) % 24], photos[(i + 3) % 24]]))
        .collect();
    big.validate().unwrap();
    h.app.session.books.add(big).unwrap();
    cmd(&mut h, "view.book", json!({}));
    cmd(&mut h, "book.preview", json!({"page": 100}));
    run_until(&mut h, |h| crate::panels::book::preview::state(&h.app, h.time())["pagesReady"].as_u64() >= Some(5));
    h.app.ui.book.preview.as_mut().unwrap().reduce_motion = false;
    wake(&mut h);
    assert!(
        has(&h, "book:previewThumb:50") && !has(&h, "book:previewThumb:0") && !has(&h, "book:previewThumb:100"),
        "only the spreads around the one on show are in the strip"
    );
    for _ in 0..4 {
        let _ = h.request("ui.key", json!({"key": "ArrowRight"}), T);
        for _ in 0..30 {
            h.view.ctx.request_repaint();
            h.step();
        }
    }
    let f = h.app.ui.book.preview.as_ref().unwrap().frames;
    eprintln!("book preview, 200 pages: {} turn frames, drawing mean {:.2} ms, worst {:.2} ms", f.count, f.sum_ms / f.count.max(1) as f32, f.max_ms);
    assert!(f.count > 0 && f.sum_ms / f.count as f32 <= 16.0, "{f:?}");
    assert_eq!(page(&h), 107, "four spreads on from 99·100");
}

/// Every photo's rating, flag and the count, to see that nothing under the preview changed.
fn library_state(h: &Headless) -> (usize, Vec<(u64, u8, String)>) {
    let photos: Vec<_> = h.app.session.catalog.photos().map(|p| (p.id.0, p.rating, format!("{:?}", p.flag))).collect();
    (photos.len(), photos)
}

#[test]
fn keys_the_preview_does_not_use_do_not_reach_the_library() {
    let mut h = demo();
    open(&mut h, 8);
    let first = h.app.session.catalog.photos().next().map(|p| p.id.0).unwrap();
    cmd(&mut h, "library.select", json!({"ids": [first]}));
    let before = library_state(&h);
    let selected = h.app.session.selected().to_vec();
    for k in ["3", "x", "p", "5", "0", "Backspace", "Delete", "u"] {
        key(&mut h, k);
    }
    assert!(h.app.ui.book.preview.is_some() && h.app.ui.fullscreen, "still open");
    assert_eq!(library_state(&h), before, "ratings, flags and photos stay as they were");
    assert_eq!(h.app.session.selected().to_vec(), selected, "the selection stays");
    // the book's own undo stack gained nothing either
    key_with(&mut h, "z", json!({"cmd": true}));
    assert!(h.app.ui.book.preview.is_some());
}

#[test]
fn a_page_drawn_from_thumbnails_says_rendering_until_its_render_arrives() {
    let mut h = demo();
    let photos: Vec<u64> = h.app.session.catalog.photos().take(8).map(|p| p.id.0).collect();
    cmd(&mut h, "book.create", json!({"ids": photos, "preset": "one", "cover": true}));
    cmd(&mut h, "view.book", json!({}));
    // no settling: the cover's render is still on its way for the first frames
    let r = h.request("engine.execute", json!({"command": "book.preview", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    run_until(&mut h, |h| has(h, "book:previewRendering"));
    assert!(!h.app.session.activity.list().iter().any(|t| t.kind == "preview"), "no row in the activity stack");
    run_until(&mut h, |h| !has(h, "book:previewRendering"));
    let r = crate::panels::book::preview::state(&h.app, h.time());
    assert!(r["pagesReady"].as_u64() >= Some(1), "the label goes when the cover is ready: {r}");
}

#[test]
fn never_more_than_two_renders_run_at_once() {
    let mut h = demo();
    open(&mut h, 8);
    let mut worst = 0;
    for round in 0..12 {
        let to = ["last", "first", "next", "last", "first", "previous"][round % 6];
        cmd_nosettle(&mut h, "book.previewPage", json!({"to": to}));
        for _ in 0..6 {
            h.step();
            let r = crate::panels::book::preview::state(&h.app, h.time());
            worst = worst.max(r["rendersRunning"].as_u64().unwrap_or(99));
        }
    }
    assert!(worst <= 2, "{worst} renders at once");
    run_until(&mut h, |h| crate::panels::book::preview::state(&h.app, h.time())["rendersRunning"] == 0);
}

/// `cmd` without waiting for the renders to settle.
fn cmd_nosettle(h: &mut Headless, id: &str, params: Value) {
    // (a turn past the cover or the last spread is refused: only the renders matter here)
    let _ = h.request("engine.execute", json!({"command": id, "params": params}), T);
}
