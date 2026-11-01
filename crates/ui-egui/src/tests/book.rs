//! The Book dialog, driven through the control channel: creating a book, the template and setup
//! buttons, dragging a photo onto another cell, the views and the PDF.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);

fn demo(services: Services) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
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

fn ids(h: &Headless, n: usize) -> Vec<u64> {
    h.app.session.catalog.photos().take(n).map(|p| p.id.0).collect()
}

fn open_book(h: &mut Headless) {
    cmd(h, "dialog.book", json!({}));
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Book)));
}

fn book(h: &Headless) -> lightcraft_engine::book::Book {
    h.app.session.books.find(None).unwrap().clone()
}

fn media_box(pdf: &[u8]) -> (f64, f64) {
    let text = String::from_utf8_lossy(pdf);
    let at = text.find("/MediaBox [").expect("a MediaBox") + "/MediaBox [".len();
    let nums: Vec<f64> = text[at..].split(']').next().unwrap().split_whitespace().map(|v| v.parse().unwrap()).collect();
    (nums[2], nums[3])
}

#[test]
fn the_dialog_opens_without_a_book_and_makes_one_from_the_view() {
    let mut h = demo(Services::default());
    open_book(&mut h);
    assert!(has(&h, "button:bookNewView") && has(&h, "button:dialogOk"));
    assert!(!has(&h, "book:pageLabel"), "no book, no pages");
    click(&mut h, "button:bookNewView");
    let b = book(&h);
    assert_eq!(b.photos().len(), h.app.session.visible().len());
    assert!(has(&h, "book:pageLabel") && has(&h, "book:page:0") && has(&h, "book:cell:0:0"));
    click(&mut h, "button:dialogCancel");
    assert!(h.app.ui.dialog.is_none());
}

/// Opens a dropdown and picks its row.
fn pick(h: &mut Headless, combo: &str, row: &str) {
    click(h, combo);
    click(h, row);
}

#[test]
fn the_dropdowns_and_setup_run_book_commands() {
    let mut h = demo(Services::default());
    let picked = ids(&h, 8);
    cmd(&mut h, "book.create", json!({"ids": picked, "preset": "one", "cover": false}));
    open_book(&mut h);
    assert_eq!(book(&h).pages.len(), 8);
    // select page 3 by clicking its sheet, then give it the 4 grid from the Page Template dropdown
    click(&mut h, "book:page:2");
    assert_eq!(h.app.ui.book.page, 2);
    pick(&mut h, "combo:bookTemplate", "menu:bookTemplate-grid4");
    assert_eq!(book(&h).pages[2].template, lightcraft_engine::book::Template::Grid4);
    assert_eq!(book(&h).unplaced.len(), 0);
    pick(&mut h, "combo:bookTemplate", "menu:bookTemplate-one");
    assert_eq!(book(&h).unplaced.len(), 0, "one photo was already there");
    // setup
    click(&mut h, "button:bookOrientation-0");
    assert_eq!(book(&h).settings.orientation, lightcraft_engine::print::layout::Orientation::Portrait);
    click(&mut h, "check:bookFill");
    assert!(book(&h).settings.fill);
    // the cover is added in front: the other pages and the page on show keep their places
    let before: Vec<_> = book(&h).pages.iter().map(|p| p.slots.clone()).collect();
    click(&mut h, "check:bookCover");
    assert!(book(&h).cover);
    let after: Vec<_> = book(&h).pages.iter().map(|p| p.slots.clone()).collect();
    assert_eq!(after[1..], before[..], "no hand-made placement was lost");
    assert_eq!(h.app.ui.book.page, 3, "still on the same page");
    // the spread view shows the page and its neighbour
    click(&mut h, "button:bookView-1");
    assert_eq!(h.app.ui.book.view, crate::book_view::View::Spread);
    assert!(has(&h, "book:page:3") && has(&h, "book:page:4"));
    click(&mut h, "button:bookPageNext");
    assert_eq!(h.app.ui.book.page, 4);
    click(&mut h, "check:bookCover");
    assert!(!book(&h).cover);
    assert_eq!(book(&h).pages.iter().map(|p| p.slots.clone()).collect::<Vec<_>>(), before);
}

#[test]
fn the_side_panel_has_three_dropdowns_and_no_button_rows() {
    let mut h = demo(Services::default());
    let picked = ids(&h, 8);
    cmd(&mut h, "book.create", json!({"ids": picked}));
    open_book(&mut h);
    for combo in ["combo:bookLayout", "combo:bookSource", "combo:bookTemplate"] {
        assert!(has(&h, combo), "{combo}");
    }
    let rows: Vec<String> =
        h.app.widgets.iter().map(|(w, _)| w.clone()).filter(|w| w.starts_with("button:bookTemplate") || w.starts_with("button:bookPreset")).collect();
    assert!(rows.is_empty(), "template and preset button rows are gone: {rows:?}");
    for gone in ["button:bookUseSelected", "button:bookUseView"] {
        assert!(!has(&h, gone), "{gone}");
    }
    assert!(has(&h, "button:bookAutoLayout") && has(&h, "check:bookCover"));
    // Auto Layout takes the layout and the photos source from the dropdowns
    pick(&mut h, "combo:bookLayout", "menu:bookLayout-grid4");
    pick(&mut h, "combo:bookSource", "menu:bookSource-view");
    click(&mut h, "button:bookAutoLayout");
    let b = book(&h);
    assert!(b.cover);
    assert!(b.pages[1..].iter().filter(|p| p.slots.len() == 4).count() >= 1);
    assert!(b.pages[1..b.pages.len() - 1].iter().all(|p| p.template == lightcraft_engine::book::Template::Grid4));
    assert_eq!(b.photos().len(), h.app.session.visible().len(), "the photos in view");
    // Selected Photos: the selection, here one photo, on the cover
    let sel = h.app.session.selected();
    pick(&mut h, "combo:bookSource", "menu:bookSource-selected");
    click(&mut h, "button:bookAutoLayout");
    assert_eq!(book(&h).photos().len(), sel.len());
}

#[test]
fn the_top_bar_group_is_centred_and_the_counter_never_touches_the_arrows() {
    let mut h = demo(Services::default());
    let picked = ids(&h, 12);
    cmd(&mut h, "book.create", json!({"ids": picked, "preset": "one", "cover": false}));
    open_book(&mut h);
    let centre_x = |h: &Headless, id: &str| rect(h, id).center().x;
    for page in [0, 5, 11] {
        h.app.ui.book.page = page;
        h.settle(T);
        let (bar, group) = (rect(&h, "book:topBar"), rect(&h, "book:topGroup"));
        assert!((centre_x(&h, "book:topGroup") - bar.center().x).abs() < 4.0, "page {page}: group {group:?} in bar {bar:?}");
        assert!(bar.contains_rect(group));
        let (prev, counter, next) = (rect(&h, "button:bookPagePrev"), rect(&h, "book:pageLabel"), rect(&h, "button:bookPageNext"));
        assert!(prev.right() + 4.0 <= counter.left() && counter.right() + 4.0 <= next.left(), "page {page}: {prev:?} {counter:?} {next:?}");
    }
    // the arrows stay where they are as the page changes
    h.app.ui.book.page = 0;
    h.settle(T);
    let (prev0, next0) = (rect(&h, "button:bookPagePrev"), rect(&h, "button:bookPageNext"));
    h.app.ui.book.page = 11;
    h.settle(T);
    assert_eq!((rect(&h, "button:bookPagePrev"), rect(&h, "button:bookPageNext")), (prev0, next0));
}

#[test]
fn the_counter_has_room_for_fifty_of_fifty() {
    let mut h = demo(Services::default());
    let picked = ids(&h, 8);
    cmd(&mut h, "book.create", json!({"ids": picked}));
    // fifty pages (the most a book has): the counter is as wide as it is for two digits at both ends
    {
        let b = h.app.session.books.find_mut(None).unwrap();
        b.pages = (0..50).map(|i| lightcraft_engine::book::Page::with(lightcraft_engine::book::Template::One, &[1 + i % 8])).collect();
        b.cover = false;
    }
    open_book(&mut h);
    h.app.ui.book.page = 49;
    h.settle(T);
    let (prev, counter, next) = (rect(&h, "button:bookPagePrev"), rect(&h, "book:pageLabel"), rect(&h, "button:bookPageNext"));
    assert!(prev.right() + 4.0 <= counter.left() && counter.right() + 4.0 <= next.left(), "{prev:?} {counter:?} {next:?}");
    let galley = h.app.widgets.iter().any(|(w, _)| w == "book:pageLabel");
    assert!(galley);
}

#[test]
fn a_photo_dragged_onto_another_cell_swaps_with_it() {
    let mut h = demo(Services::default());
    let picked = ids(&h, 4);
    cmd(&mut h, "book.create", json!({"ids": picked, "preset": "two", "cover": false}));
    open_book(&mut h);
    click(&mut h, "button:bookView-1");
    let before = book(&h);
    let (a, b) = (rect(&h, "book:cell:0:0"), rect(&h, "book:cell:0:1"));
    let r = h.request(
        "ui.dragWidget",
        json!({"id": "book:cell:0:0", "dx": b.center().x - a.center().x, "dy": b.center().y - a.center().y, "steps": 8}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    let after = book(&h);
    assert_eq!(after.pages[0].slots, [before.pages[0].slots[1], before.pages[0].slots[0]], "the two photos swapped");
    assert_eq!(after.photos().len(), 4);
    // taken off its page, a photo waits in the tray, which then shows
    cmd(&mut h, "book.movePhoto", json!({"from": {"page": 0, "cell": 0}, "to": {"unplaced": 0}}));
    assert!(has(&h, "book:tray") && has(&h, "book:unplaced:0"));
}

#[test]
fn a_long_tray_scrolls_so_every_photo_can_be_reached() {
    let mut h = demo(Services::default());
    let picked = ids(&h, 8);
    cmd(&mut h, "book.create", json!({"ids": picked}));
    {
        let b = h.app.session.books.find_mut(None).unwrap();
        b.unplaced = (0..40).map(|i| 1 + i % 8).collect();
    }
    open_book(&mut h);
    let tray = rect(&h, "book:tray");
    let first = rect(&h, "book:unplaced:0");
    assert!(tray.contains_rect(first));
    // forty photos are laid out in one row, far wider than the strip; none is cut off
    assert!(has(&h, "book:unplaced:39"), "the last photo is in the strip");
    assert!(rect(&h, "book:unplaced:39").left() > tray.right(), "past the edge, so the strip has to scroll");
    // scrolling the strip moves the photos
    let r = h.request("ui.hoverWidget", json!({"id": "book:unplaced:0"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.scroll", json!({"dx": -400.0, "dy": 0.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(rect(&h, "book:unplaced:0").left() < first.left() - 100.0, "the strip scrolled");
    assert!(rect(&h, "book:tray") == tray, "the strip itself stays put");
}

#[test]
fn export_pdf_runs_in_the_background_and_the_book_is_the_page_size() {
    let asked = Arc::new(Mutex::new(Vec::<crate::pick::PickRequest>::new()));
    let requests = asked.clone();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let written = bytes.clone();
    let services = Services {
        picker: Some(Box::new(move |req| {
            requests.lock().unwrap().push(req);
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(vec!["Trip.pdf".into()]).unwrap();
            Ok(rx)
        })),
        write_shared: Some(Arc::new(move |path, data| {
            assert_eq!(path, "Trip.pdf");
            *written.lock().unwrap() = data.to_vec();
            Ok(())
        })),
        ..Default::default()
    };
    let mut h = demo(services);
    let ids: Vec<u64> = h.app.session.catalog.photos().take(5).map(|p| p.id.0).collect();
    cmd(&mut h, "book.create", json!({"name": "Trip", "ids": ids, "settings": {"ppi": 72.0, "size": "5x7"}}));
    open_book(&mut h);
    click(&mut h, "button:dialogOk");
    assert!(h.app.ui.dialog.is_none(), "the dialog closes once the export has started");
    assert!(h.step_until(Duration::from_secs(60), |h| h.app.last_export_result.is_some()), "the export did not finish");
    assert_eq!(asked.lock().unwrap()[0].file_name.as_deref(), Some("Trip.pdf"));
    let b = bytes.lock().unwrap();
    assert!(b.starts_with(b"%PDF-"));
    // 5 x 7 in landscape: 504 x 360 pt
    let (w, hgt) = media_box(&b);
    assert!((w - 504.0).abs() < 1.0 && (hgt - 360.0).abs() < 1.0, "got {w} x {hgt} pt");
    let first = &h.app.last_export_result.as_ref().unwrap()["files"][0];
    assert_eq!((first["book"].clone(), first["photos"].clone()), (json!(true), json!(5)));
    assert!(h.app.session.activity.list().is_empty());
    // the row ends with every render done, not one
    let files = h.app.last_export_result.as_ref().unwrap()["files"].as_array().unwrap().clone();
    assert_eq!(files.len(), 1);
    assert_eq!(crate::export_task::book_task::finished_done(&files, 24), 24);
    assert_eq!(crate::export_task::book_task::finished_done(&[json!({"path": "a"}), json!({"path": "b"})], 5), 2, "other exports count their files");
}

#[test]
fn a_book_export_shows_a_row_in_the_activity_stack_and_stops() {
    let mut app = LightcraftApp::new(
        lightcraft_engine::Session::with_demo(),
        Services { write_shared: Some(Arc::new(|_, _| panic!("a stopped export wrote a file"))), ..Default::default() },
    );
    let ids: Vec<u64> = app.session.catalog.photos().take(12).map(|p| p.id.0).collect();
    app.session.execute("book.create", &json!({"ids": ids, "settings": {"ppi": 300.0}})).unwrap();
    crate::export_task::book_task::start(&mut app, &json!({"path": "Stop.pdf"}), false).unwrap();
    let rows = app.session.activity.list();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].kind, rows[0].label.as_str()), ("export", "Exporting book to PDF"));
    assert!(rows[0].cancellable);
    app.session.activity.cancel(rows[0].id).unwrap();
    let task = app.export.take().unwrap();
    assert!(task.cancel.load(std::sync::atomic::Ordering::Relaxed));
    drop(task);
    assert!(app.session.activity.list().is_empty(), "the row goes with the export");
    // no book, no export; an export already running is refused
    let mut none = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    assert!(crate::export_task::book_task::start(&mut none, &json!({"path": "x.pdf"}), false).is_err());
    assert!(crate::export_task::book_task::start(&mut none, &json!({}), false).is_err());
}

#[test]
fn the_menu_commands_are_listed_and_need_a_photo() {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    assert!(crate::menus::ui_enabled(&app, "dialog.book"));
    let empty = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
    assert!(!crate::menus::ui_enabled(&empty, "dialog.book"));
    assert!(crate::menus::ui_commands().any(|c| c.0 == "dialog.book" && c.3 == "File"));
}

#[test]
fn the_book_dialog_fits_the_window_with_its_buttons_on_screen() {
    for size in [[1400.0, 900.0], [1100.0, 760.0]] {
        let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
        let mut h = Headless::new(app, size, 1.0);
        h.settle(T);
        let picked = ids(&h, 6);
        cmd(&mut h, "book.create", json!({"ids": picked}));
        cmd(&mut h, "book.movePhoto", json!({"from": {"page": 1, "cell": 0}, "to": {"unplaced": 0}}));
        open_book(&mut h);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, h.size);
        let dialog = rect(&h, "dialog:window");
        assert!(screen.contains_rect(dialog), "at {size:?}: dialog {dialog:?} runs off the window");
        for b in ["button:dialogOk", "button:dialogCancel"] {
            assert!(dialog.contains_rect(rect(&h, b)), "at {size:?}: {b} is outside the dialog");
        }
    }
}
