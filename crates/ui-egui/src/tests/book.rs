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

#[test]
fn template_buttons_setup_and_views_run_book_commands() {
    let mut h = demo(Services::default());
    let picked = ids(&h, 8);
    cmd(&mut h, "book.create", json!({"ids": picked, "preset": "one", "cover": false}));
    open_book(&mut h);
    assert_eq!(book(&h).pages.len(), 8);
    // select page 3 by clicking its sheet, then give it the 4 grid
    click(&mut h, "book:page:2");
    assert_eq!(h.app.ui.book.page, 2);
    click(&mut h, "button:bookTemplate-3");
    assert_eq!(book(&h).pages[2].template, lightcraft_engine::book::Template::Grid4);
    assert_eq!(book(&h).unplaced.len(), 0);
    click(&mut h, "button:bookTemplate-0");
    assert_eq!(book(&h).unplaced.len(), 0, "one photo was already there");
    // setup
    click(&mut h, "button:bookOrientation-0");
    assert_eq!(book(&h).settings.orientation, lightcraft_engine::print::layout::Orientation::Portrait);
    click(&mut h, "check:bookFill");
    assert!(book(&h).settings.fill);
    click(&mut h, "check:bookCover");
    assert!(book(&h).cover);
    // the spread view shows the page and its neighbour
    click(&mut h, "button:bookView-1");
    assert_eq!(h.app.ui.book.view, crate::book_view::View::Spread);
    assert!(has(&h, "book:page:1") && has(&h, "book:page:2"));
    click(&mut h, "button:bookPageNext");
    assert_eq!(h.app.ui.book.page, 3);
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
