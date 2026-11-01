//! The Book view, driven through the control channel: opening it from File ▸ Book… and the side bar,
//! the page strip, dragging photos between the pool and the frames, zoom and pan, Add to Book and its
//! questions, the side bar's rename, and undo.

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
    let r = h.request("ui.set", json!({"leftPanel": true, "sidebar": [{"id": "albums", "hidden": true}, {"id": "local", "hidden": true}, {"id": "byDate", "hidden": true}, {"id": "folders", "hidden": true}, {"id": "keywords", "hidden": true}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    h
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(T);
}

fn cmd(h: &mut Headless, id: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": id, "params": params}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(T);
    r["result"].clone()
}

fn key(h: &mut Headless, key: &str) {
    let r = h.request("ui.key", json!({"key": key}), T);
    assert_eq!(r["ok"], true, "{key}: {r}");
    h.settle(T);
}

/// Drags the middle of widget `from` onto the middle of widget `to`.
fn drag_onto(h: &mut Headless, from: &str, to: &str) {
    let (a, b) = (rect(h, from).center(), rect(h, to).center());
    let r = h.request("ui.drag", json!({"x": a.x, "y": a.y, "toX": b.x, "toY": b.y, "steps": 10}), T);
    assert_eq!(r["ok"], true, "{from} → {to}: {r}");
    h.settle(T);
}

fn ids(h: &Headless, n: usize) -> Vec<u64> {
    h.app.session.catalog.photos().take(n).map(|p| p.id.0).collect()
}

fn book(h: &Headless) -> lightcraft_engine::book::Book {
    h.app.session.books.find(None).unwrap().clone()
}

/// A book of `n` photos, one per page, no cover, open in the view.
fn open_book(h: &mut Headless, n: usize, preset: &str) {
    let photos = ids(h, n);
    cmd(h, "book.create", json!({"ids": photos, "preset": preset, "cover": false}));
    cmd(h, "view.book", json!({}));
    assert_eq!(h.app.ui.view, ViewMode::Book);
}

#[test]
fn file_book_opens_the_view_not_a_dialog_and_the_side_bar_lists_the_books() {
    let mut h = demo();
    cmd(&mut h, "dialog.book", json!({}));
    assert_eq!(h.app.ui.view, ViewMode::Book);
    assert!(h.app.ui.dialog.is_none(), "a view in the main window, not a dialog");
    assert!(has(&h, "button:bookNewEmpty"), "no book yet");
    let photos = ids(&h, 4);
    cmd(&mut h, "book.create", json!({"name": "Alps", "ids": photos}));
    cmd(&mut h, "book.create", json!({"name": "Coast", "ids": photos}));
    h.settle(T);
    assert!(has(&h, "source:book:1") && has(&h, "source:book:2"), "both books are in the side bar");
    assert!(has(&h, "book:spread") && has(&h, "book:strip") && has(&h, "book:pool"), "spread, strip and pool");
    // a book of the side bar opens it; a library row leaves; coming back keeps the page
    click(&mut h, "source:book:1");
    assert_eq!(h.app.session.books.current, Some(1));
    h.app.ui.book.page = 1;
    click(&mut h, "source:all");
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
    click(&mut h, "source:book:1");
    assert_eq!((h.app.ui.view, h.app.ui.book.page), (ViewMode::Book, 1));
}

#[test]
fn the_strip_jumps_reorders_adds_and_the_keys_move() {
    let mut h = demo();
    open_book(&mut h, 8, "one");
    assert!(has(&h, "book:thumb:0") && has(&h, "button:bookAddPage"));
    click(&mut h, "book:thumb:3");
    assert_eq!(h.app.ui.book.page, 3);
    key(&mut h, "ArrowRight");
    assert_eq!(h.app.ui.book.page, 4);
    key(&mut h, "ArrowLeft");
    assert_eq!(h.app.ui.book.page, 3);
    // pages lie open 0 | 1 2 | 3 4 | 5 6 | 7: Page Down goes to the next spread
    key(&mut h, "PageDown");
    assert_eq!(h.app.ui.book.page, 5);
    key(&mut h, "PageUp");
    assert_eq!(h.app.ui.book.page, 3);
    // drag page 6's thumbnail onto page 2's: it becomes page 2
    let before = book(&h).pages.iter().map(|p| p.slots.clone()).collect::<Vec<_>>();
    drag_onto(&mut h, "book:thumb:5", "book:thumb:1");
    let after = book(&h).pages.iter().map(|p| p.slots.clone()).collect::<Vec<_>>();
    assert_eq!(after[1], before[5], "dragged to page 2");
    assert_eq!(after.len(), 8);
    cmd(&mut h, "edit.undo", json!({}));
    assert_eq!(book(&h).pages.iter().map(|p| p.slots.clone()).collect::<Vec<_>>(), before, "one undo step");
    // + adds an empty page after the one on show
    click(&mut h, "book:thumb:2");
    click(&mut h, "button:bookAddPage");
    assert_eq!(book(&h).pages.len(), 9);
    assert_eq!(book(&h).pages[3].slots, [None]);
    assert_eq!(h.app.ui.book.page, 3, "the new page is on show");
}

#[test]
fn photos_go_from_the_pool_into_a_frame_and_swap_between_frames() {
    let mut h = demo();
    open_book(&mut h, 4, "two");
    let extra: Vec<u64> = h.app.session.catalog.photos().skip(4).take(2).map(|p| p.id.0).collect();
    cmd(&mut h, "book.addPhotos", json!({"ids": extra}));
    // pages: [a b] [c d]; tray: [e f]; the spread of page 1 and 2 is on show
    h.app.ui.book.page = 0;
    click(&mut h, "book:thumb:1");
    let b = book(&h);
    assert_eq!((b.pages.len(), b.unplaced.len()), (2, 2));
    let (first, second) = (b.pages[1].slots[0], b.unplaced[0]);
    // unplaced photos come first in the pool, marked by an outline; placed ones carry their page
    assert!(has(&h, "book:pool:unplaced:0") && has(&h, "book:pool:0:0"));
    drag_onto(&mut h, "book:pool:unplaced:0", "book:cell:1:0");
    let b = book(&h);
    assert_eq!(b.pages[1].slots[0], Some(second));
    assert_eq!(b.unplaced.last().copied(), first, "the photo that was there waits in the tray");
    // drag a frame's photo onto a frame of the other page: they swap
    let (x, y) = (b.pages[1].slots[0], b.pages[1].slots[1]);
    drag_onto(&mut h, "book:cell:1:0", "book:cell:1:1");
    assert_eq!((book(&h).pages[1].slots[0], book(&h).pages[1].slots[1]), (y, x));
    // a frame's photo dropped on the pool comes off its page
    let n = book(&h).unplaced.len();
    drag_onto(&mut h, "book:cell:1:0", "book:pool");
    assert_eq!(book(&h).unplaced.len(), n + 1);
    assert_eq!(book(&h).pages[1].slots[0], None);
    // each was one undo step
    for _ in 0..3 {
        cmd(&mut h, "edit.undo", json!({}));
    }
    assert_eq!(book(&h).pages[1].slots, [first, b.pages[1].slots[1]]);
}

#[test]
fn zoom_and_pan_save_a_crop_per_cell_as_one_undo_step() {
    let mut h = demo();
    open_book(&mut h, 4, "two");
    click(&mut h, "book:thumb:1");
    let depth = h.app.session.undo.len();
    // scroll over the frame zooms (a pinch too)
    let r = h.request("ui.hoverWidget", json!({"id": "book:cell:1:0"}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..3 {
        let r = h.request("ui.zoom", json!({"factor": 1.3}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h.settle(T);
    // (the zoom settles, then saves once)
    for _ in 0..40 {
        if book(&h).pages[1].crop(0).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        h.step();
    }
    let crop = book(&h).pages[1].crop(0).expect("the zoom was saved");
    assert!((crop.zoom - 1.3f64.powi(3)).abs() < 0.05, "{crop:?}");
    assert_eq!(h.app.session.undo.len(), depth + 1, "a run of scroll steps is one undo step");
    // dragging inside the frame moves the photo: the crop's centre goes the other way
    let r = h.request("ui.dragWidget", json!({"id": "book:cell:1:0", "dx": -25.0, "dy": 0.0, "steps": 8}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    let moved = book(&h).pages[1].crop(0).unwrap();
    assert!(moved.x > crop.x, "{moved:?} vs {crop:?}");
    assert_eq!(moved.zoom, crop.zoom);
    assert_eq!(h.app.session.undo.len(), depth + 2);
    // the other cell has none
    assert_eq!(book(&h).pages[1].crop(1), None);
    // double click puts the photo back to the book's setting
    click_twice(&mut h, "book:cell:1:0");
    assert_eq!(book(&h).pages[1].crop(0), None);
    cmd(&mut h, "edit.undo", json!({}));
    assert_eq!(book(&h).pages[1].crop(0), Some(moved));
}

fn click_twice(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id, "count": 2}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(T);
}

#[test]
fn add_to_book_asks_about_photos_the_book_has() {
    let mut h = demo();
    open_book(&mut h, 3, "one");
    let all = ids(&h, 6);
    // the library's right-click menu: Add to Book ▸ the books and New Book…
    cmd(&mut h, "view.photoGrid", json!({}));
    let photo = all[4];
    cmd(&mut h, "library.select", json!({"ids": [photo]}));
    let thumb = rect(&h, &format!("thumb:{photo}"));
    let r = h.request("ui.click", json!({"x": thumb.center().x, "y": thumb.center().y, "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let r = h.request("ui.hoverWidget", json!({"id": "button:contextAddToBook"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(has(&h, "button:contextBook:1") && has(&h, "button:contextNewBook"));
    click(&mut h, "button:contextBook:1");
    assert_eq!(book(&h).unplaced, [photo], "a new photo goes in at once, without a question");
    assert!(h.app.ui.book.prompt.is_none());
    // a photo the book has: a question, with its three answers
    let held = all[0];
    cmd(&mut h, "library.select", json!({"ids": [held, all[5]]}));
    let ctx = h.view.ctx.clone();
    crate::panels::book::add_photos_to_book(&mut h.app, &ctx, 1, vec![held, all[5]]);
    h.settle(T);
    assert!(has(&h, "book:prompt") && has(&h, "button:bookPromptYes") && has(&h, "button:bookPromptSkip") && has(&h, "button:bookPromptCancel"));
    click(&mut h, "button:bookPromptCancel");
    assert!(!has(&h, "book:prompt"));
    assert_eq!(book(&h).photos().len(), 4, "cancel adds nothing");
    crate::panels::book::add_photos_to_book(&mut h.app, &ctx, 1, vec![held, all[5]]);
    h.settle(T);
    click(&mut h, "button:bookPromptSkip");
    let b = book(&h);
    assert_eq!(b.photos().len(), 5, "skip adds only the new photo");
    assert!(b.unplaced.contains(&all[5]));
    crate::panels::book::add_photos_to_book(&mut h.app, &ctx, 1, vec![held]);
    h.settle(T);
    click(&mut h, "button:bookPromptYes");
    assert_eq!(book(&h).photos().iter().filter(|p| **p == held).count(), 2, "add duplicates puts it in again");
}

#[test]
fn photos_dragged_onto_a_book_in_the_side_bar_are_added() {
    let mut h = demo();
    open_book(&mut h, 2, "one");
    cmd(&mut h, "view.photoGrid", json!({}));
    let target = ids(&h, 8)[6];
    let thumb = rect(&h, &format!("thumb:{target}"));
    let row = rect(&h, "source:book:1");
    let r = h.request("ui.drag", json!({"x": thumb.center().x, "y": thumb.center().y, "toX": row.center().x, "toY": row.center().y, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(book(&h).unplaced.contains(&target));
}

#[test]
fn the_side_bar_renames_in_place_and_refuses_empty_and_taken_names() {
    let mut h = demo();
    let photos = ids(&h, 3);
    cmd(&mut h, "book.create", json!({"name": "Alps", "ids": photos}));
    cmd(&mut h, "book.create", json!({"name": "Coast", "ids": photos}));
    h.settle(T);
    let name = |h: &Headless, id: u32| h.app.session.books.find(Some(id)).unwrap().name.clone();
    // double click: a field in the row; Return saves
    let r = h.request("ui.clickWidget", json!({"id": "source:book:2", "count": 2}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(has(&h, "field:bookRename"));
    h.app.ui.book.rename = Some((2, "Shore".into()));
    key(&mut h, "Enter");
    assert_eq!(name(&h, 2), "Shore");
    // Esc cancels
    h.app.ui.book.rename = Some((2, "Reef".into()));
    h.settle(T);
    key(&mut h, "Escape");
    assert_eq!(name(&h, 2), "Shore");
    assert!(h.app.ui.book.rename.is_none());
    // a taken name and an empty one are refused
    for bad in ["alps", "   "] {
        h.app.ui.book.rename = Some((2, bad.into()));
        h.settle(T);
        key(&mut h, "Enter");
        assert_eq!(name(&h, 2), "Shore", "`{bad}` is refused");
    }
    // the rename is one undo step
    cmd(&mut h, "edit.undo", json!({}));
    assert_eq!(name(&h, 2), "Coast");
}

#[test]
fn locked_pages_show_and_the_toolbar_acts_on_the_page_on_show() {
    let mut h = demo();
    open_book(&mut h, 4, "one");
    click(&mut h, "book:thumb:2");
    click(&mut h, "button:bookLockPage");
    assert!(book(&h).pages[2].locked);
    click(&mut h, "button:bookDuplicatePage");
    assert_eq!(book(&h).pages.len(), 5);
    assert_eq!(h.app.ui.book.page, 3, "the copy is on show");
    click(&mut h, "button:bookDeletePage");
    assert_eq!(book(&h).pages.len(), 4);
    assert_eq!(book(&h).unplaced.len(), 1, "its photo waits in the pool");
}

/// The view draws a book of 200 pages without lag: it paints only what is on screen. The numbers go
/// to the test output (`--nocapture`); the check is the 16 ms budget with room for a loaded machine.
#[test]
fn a_book_of_two_hundred_pages_draws_inside_the_frame_budget() {
    use lightcraft_engine::book::{Book, MAX_PAGES, Page, Template};
    let mut h = demo();
    let photos: Vec<u64> = ids(&h, 24);
    let mut big = Book { name: "Big".into(), ..Default::default() };
    big.pages = (0..MAX_PAGES)
        .map(|i| Page::with(Template::Grid4, &[photos[i % 24], photos[(i + 1) % 24], photos[(i + 2) % 24], photos[(i + 3) % 24]]))
        .collect();
    big.validate().unwrap();
    h.app.session.books.add(big).unwrap();
    cmd(&mut h, "view.book", json!({}));
    h.app.ui.book.page = 100;
    h.app.ui.book.reveal_page = true;
    h.settle(T);
    let (mut times, mut steps) = (Vec::new(), Vec::new());
    for _ in 0..60 {
        h.view.ctx.request_repaint();
        let started = std::time::Instant::now();
        h.step();
        steps.push(started.elapsed().as_secs_f32() * 1000.0);
        times.push(h.app.ui.book.frame_ms);
    }
    let stats = |v: &[f32]| (v.iter().sum::<f32>() / v.len() as f32, v.iter().fold(0.0f32, |w, t| w.max(*t)));
    let (mean, worst) = stats(&times);
    let (step_mean, step_worst) = stats(&steps);
    eprintln!(
        "book view, 200 pages: building the view mean {mean:.2} ms, worst {worst:.2} ms; the whole headless frame (UI, tessellation, CPU raster) mean {step_mean:.2} ms, worst {step_worst:.2} ms; {} frames",
        times.len()
    );
    assert!(has(&h, "book:thumb:100"), "the page on show is in the strip");
    assert!(!has(&h, "book:thumb:0") && !has(&h, "book:thumb:199"), "only the pages on screen are drawn");
    assert!(mean < 16.0, "mean frame {mean} ms");
}

/// With Zoom to Fill Frame off a frame shows the whole photo; scrolling out at that size stores no
/// crop (it would turn the frame into a filled one), and zooming in and out again leaves none.
#[test]
fn zooming_out_at_the_frames_size_stores_no_crop() {
    let mut h = demo();
    open_book(&mut h, 4, "two");
    assert!(!book(&h).settings.fill);
    click(&mut h, "book:thumb:1");
    let depth = h.app.session.undo.len();
    let r = h.request("ui.hoverWidget", json!({"id": "book:cell:1:0"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.zoom", json!({"factor": 0.7}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(50));
        h.step();
    }
    assert_eq!(book(&h).pages[1].crop(0), None, "scrolling out stored a crop");
    assert_eq!(h.app.session.undo.len(), depth);
    // in, then back out to the same size, before it is saved: nothing is saved
    for factor in [1.5, 1.0 / 1.5] {
        let r = h.request("ui.zoom", json!({"factor": factor}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h.settle(T);
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(50));
        h.step();
    }
    assert_eq!(book(&h).pages[1].crop(0), None);
    assert_eq!(h.app.session.undo.len(), depth);
}
