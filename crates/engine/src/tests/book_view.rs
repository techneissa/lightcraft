//! The Book view's commands: undo, adding photos (and the duplicate question), pages, locks, crops,
//! and that a crop shows the same part of the photo in the preview and in the export.

use lightcraft_catalog::MemStore;
use serde_json::{Value, json};

use crate::Session;
use crate::book::geometry::crop_window;
use crate::book::model::MAX_ZOOM;
use crate::book::{Book, Crop, MAX_PAGES, Page, Template, render};
use crate::library::LibraryStores;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn fails(s: &mut Session, id: &str, p: Value) -> String {
    s.execute(id, &p).expect_err(&format!("{id} {p} should fail")).to_string()
}

fn ids(s: &Session, n: usize) -> Vec<u64> {
    s.catalog.photos().take(n).map(|p| p.id.0).collect()
}

fn open(files: &MemStore) -> Session {
    let mut s = Session::with_demo();
    let stores = LibraryStores { dir: "bookview".into(), catalog: Box::new(MemStore::new()), files: Box::new(files.clone()), on_disk: false };
    s.open_library_in(stores, false).unwrap();
    crate::demo::load(&mut s);
    s
}

/// A book of six photos on `one`, `two` and `three` pages, no cover.
fn six(s: &mut Session) -> Value {
    let ids = ids(s, 6);
    run(s, "book.create", json!({"name": "Six", "ids": ids, "cover": false, "preset": "mixed"}))
}

#[test]
fn every_change_is_one_undo_step_and_redo_brings_it_back() {
    let mut s = Session::with_demo();
    let photos = ids(&s, 8);
    six(&mut s);
    let steps: Vec<(&str, Value, &str)> = vec![
        ("book.rename", json!({"name": "Holiday"}), "Rename Book"),
        ("book.set", json!({"margin": 1.0}), "Change Book Settings"),
        ("book.setTemplate", json!({"page": 0, "template": "two"}), "Set Page Template"),
        ("book.movePhoto", json!({"from": {"page": 1, "cell": 0}, "to": {"page": 0, "cell": 0}}), "Move Book Photo"),
        ("book.addPhotos", json!({"ids": [photos[6], photos[7]]}), "Add Photos to Book"),
        ("book.addPage", json!({"template": "grid4"}), "Add Book Page"),
        ("book.duplicatePage", json!({"page": 0}), "Duplicate Book Page"),
        ("book.movePage", json!({"from": 0, "to": 2}), "Move Book Page"),
        ("book.lockPage", json!({"page": 1}), "Lock Book Page"),
        ("book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": 2.0, "x": 0.3, "y": 0.7}), "Crop Book Photo"),
        ("book.layout", json!({"preset": "grid4"}), "Auto Layout Book"),
        ("book.deletePage", json!({"page": 0}), "Delete Book Page"),
    ];
    for (id, params, label) in steps {
        let before = run(&mut s, "book.get", json!({}));
        let depth = s.undo.len();
        run(&mut s, id, params.clone());
        let after = run(&mut s, "book.get", json!({}));
        assert_ne!(before, after, "{id} {params} changed nothing");
        assert_eq!(s.undo.len(), depth + 1, "{id} is one undo step");
        assert_eq!(s.undo.last().map(|e| e.label.as_str()), Some(label), "{id}");
        assert_eq!(s.undo_step().unwrap(), label);
        assert_eq!(run(&mut s, "book.get", json!({})), before, "undo {id}");
        assert_eq!(s.redo_step().unwrap(), label);
        assert_eq!(run(&mut s, "book.get", json!({})), after, "redo {id}");
    }
}

#[test]
fn creating_deleting_and_duplicating_a_book_undo_in_place() {
    let mut s = Session::with_demo();
    let a = ids(&s, 3);
    run(&mut s, "book.create", json!({"name": "A", "ids": a}));
    run(&mut s, "book.create", json!({"name": "B", "ids": a}));
    run(&mut s, "book.create", json!({"name": "C", "ids": a}));
    let names = |s: &mut Session| -> Vec<String> {
        run(s, "book.list", json!({}))["books"].as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap().to_string()).collect()
    };
    run(&mut s, "book.open", json!({"book": 2}));
    run(&mut s, "book.delete", json!({"book": 2}));
    assert_eq!(names(&mut s), ["A", "C"]);
    s.undo_step().unwrap();
    assert_eq!(names(&mut s), ["A", "B", "C"], "a deleted book comes back in its place");
    assert_eq!(run(&mut s, "book.list", json!({}))["current"], 2, "and is the one on show again");
    s.redo_step().unwrap();
    assert_eq!(names(&mut s), ["A", "C"]);
    s.undo_step().unwrap();
    let copy = run(&mut s, "book.duplicate", json!({"book": 1}));
    assert_eq!((copy["name"].as_str(), copy["current"].as_bool()), (Some("A Copy"), Some(true)));
    assert_eq!(names(&mut s), ["A", "B", "C", "A Copy"]);
    s.undo_step().unwrap();
    assert_eq!(names(&mut s), ["A", "B", "C"]);
    s.undo_step().unwrap();
    assert_eq!(names(&mut s), ["A", "B"], "the book made last is taken away");
    // a new change clears what could be redone
    run(&mut s, "book.rename", json!({"book": 1, "name": "Z"}));
    assert!(s.redo_step().is_err() || s.redo.is_empty());
}

#[test]
fn undo_and_redo_save_the_books_file() {
    let files = MemStore::new();
    let mut s = open(&files);
    six(&mut s);
    run(&mut s, "book.rename", json!({"name": "Renamed"}));
    s.undo_step().unwrap();
    drop(s);
    let mut again = open(&files);
    assert_eq!(run(&mut again, "book.get", json!({}))["name"], "Six", "the file holds the undone book");
    again.undo_step().unwrap_err(); // the history is the session's: nothing to undo after reopening
}

#[test]
fn adding_photos_asks_about_the_ones_the_book_has() {
    let mut s = Session::with_demo();
    six(&mut s);
    let all = ids(&s, 9);
    // new photos only: no question
    let b = run(&mut s, "book.addPhotos", json!({"ids": [all[6], all[7]]}));
    assert_eq!((b["added"].as_u64(), b["skipped"].as_u64(), b["unplaced"].as_array().map(Vec::len)), (Some(2), Some(0), Some(2)));
    // some the book has: the call must say what to do with them
    let e = fails(&mut s, "book.addPhotos", json!({"ids": [all[0], all[8]]}));
    assert!(e.contains("duplicates") && e.contains("already"), "{e}");
    let b = run(&mut s, "book.addPhotos", json!({"ids": [all[0], all[8]], "duplicates": "skip"}));
    assert_eq!((b["added"].as_u64(), b["skipped"].as_u64()), (Some(1), Some(1)));
    let b = run(&mut s, "book.addPhotos", json!({"ids": [all[0], all[8], all[8]], "duplicates": "add"}));
    assert_eq!((b["added"].as_u64(), b["skipped"].as_u64()), (Some(2), Some(0)), "a photo named twice goes in once");
    assert_eq!(b["unplaced"].as_array().unwrap().iter().filter(|p| **p == json!(all[0])).count(), 1);
    assert_eq!(b["photos"], 6 + 2 + 1 + 2);
    fails(&mut s, "book.addPhotos", json!({}));
    fails(&mut s, "book.addPhotos", json!({"ids": [all[0]], "duplicates": "maybe"}));
    fails(&mut s, "book.addPhotos", json!({"ids": [999_999], "duplicates": "add"}));
    // a photo in the tray counts as held too
    let e = fails(&mut s, "book.addPhotos", json!({"ids": [all[6]]}));
    assert!(e.contains("already"), "{e}");
}

#[test]
fn pages_are_added_deleted_duplicated_moved_and_locked() {
    let mut s = Session::with_demo();
    let photos = ids(&s, 5);
    run(&mut s, "book.create", json!({"ids": photos, "cover": true, "preset": "one"}));
    // pages: cover [1], [2], [3], [4], [5]
    let b = run(&mut s, "book.addPage", json!({"at": 2, "template": "two"}));
    assert_eq!((b["pages"][2]["template"].as_str(), b["pages"].as_array().unwrap().len()), (Some("two"), 6));
    assert_eq!(b["pages"][2]["slots"], json!([null, null]));
    assert!(fails(&mut s, "book.addPage", json!({"at": 0})).contains("cover"));
    assert!(fails(&mut s, "book.addPage", json!({"at": 99})).contains("no place"));
    // delete: its photos go to the end of the tray; the cover going leaves a book without one
    let b = run(&mut s, "book.deletePage", json!({"page": 3}));
    assert_eq!(b["unplaced"], json!([photos[2]]));
    let b = run(&mut s, "book.deletePage", json!({"page": 0}));
    assert_eq!((b["cover"].as_bool(), b["unplaced"].as_array().map(Vec::len)), (Some(false), Some(2)));
    assert!(fails(&mut s, "book.deletePage", json!({"page": 40})).contains("no page 41"));
    // duplicate: a copy right after, photos twice, lock and crop come along
    run(&mut s, "book.lockPage", json!({"page": 2}));
    run(&mut s, "book.setCellCrop", json!({"page": 2, "cell": 0, "zoom": 2.0}));
    let b = run(&mut s, "book.duplicatePage", json!({"page": 2}));
    assert_eq!(b["pages"][3]["slots"], b["pages"][2]["slots"]);
    assert_eq!((b["pages"][3]["locked"].as_bool(), b["pages"][3]["crops"][0]["zoom"].as_f64()), (Some(true), Some(2.0)));
    // move
    let before: Vec<Value> = b["pages"].as_array().unwrap().iter().map(|p| p["slots"].clone()).collect();
    let b = run(&mut s, "book.movePage", json!({"from": 0, "to": 3}));
    let after: Vec<Value> = b["pages"].as_array().unwrap().iter().map(|p| p["slots"].clone()).collect();
    assert_eq!(after[3], before[0]);
    assert_eq!(after[0], before[1]);
    fails(&mut s, "book.movePage", json!({"from": 0, "to": 99}));
    // with a cover it stays first
    run(&mut s, "book.setCover", json!({"cover": true}));
    assert!(fails(&mut s, "book.movePage", json!({"from": 0, "to": 2})).contains("cover"));
    assert!(fails(&mut s, "book.movePage", json!({"from": 2, "to": 0})).contains("cover"));
}

#[test]
fn auto_layout_leaves_a_locked_page_as_it_is() {
    let mut s = Session::with_demo();
    let photos = ids(&s, 9);
    run(&mut s, "book.create", json!({"ids": photos, "cover": false, "preset": "one"}));
    // lock page 2 (photo 3) and page 5 (photo 6)
    run(&mut s, "book.lockPage", json!({"page": 2}));
    run(&mut s, "book.lockPage", json!({"page": 5}));
    let b = run(&mut s, "book.layout", json!({"preset": "grid4", "cover": false}));
    let pages = b["pages"].as_array().unwrap();
    // 9 photos, 2 stay: 7 are laid out in grid4 (4 + 3); a locked page keeps its number while the
    // book has that many pages, else it goes to the end
    assert_eq!(pages.len(), 4);
    assert_eq!(pages[2]["slots"], json!([photos[2]]));
    assert_eq!(pages[3]["slots"], json!([photos[5]]));
    assert!(pages[2]["locked"] == true && pages[3]["locked"] == true);
    let mut all: Vec<u64> = pages.iter().flat_map(|p| p["slots"].as_array().unwrap().iter().filter_map(Value::as_u64)).collect();
    all.sort_unstable();
    let mut want = photos.clone();
    want.sort_unstable();
    assert_eq!(all, want, "every photo is on a page exactly once");
    assert_eq!(pages[0]["template"], "grid4");
    // a locked first page stays the cover
    let mut s = Session::with_demo();
    run(&mut s, "book.create", json!({"ids": photos, "cover": true, "preset": "two"}));
    run(&mut s, "book.lockPage", json!({"page": 0}));
    let b = run(&mut s, "book.layout", json!({"preset": "one", "cover": true}));
    assert_eq!((b["cover"].as_bool(), b["pages"][0]["slots"].clone()), (Some(true), json!([photos[0]])));
    assert_eq!(b["pages"].as_array().unwrap().len(), 9, "the cover and eight pages of one photo");
}

#[test]
fn a_photo_takes_its_crop_with_it_and_loses_it_in_the_tray() {
    let mut s = Session::with_demo();
    let photos = ids(&s, 4);
    run(&mut s, "book.create", json!({"ids": photos, "cover": false, "preset": "two"}));
    run(&mut s, "book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": 3.0, "x": 0.2, "y": 0.8}));
    let crops = |b: &Value, page: usize| b["pages"][page]["crops"].clone();
    let b = run(&mut s, "book.movePhoto", json!({"from": {"page": 0, "cell": 0}, "to": {"page": 1, "cell": 1}}));
    assert_eq!(crops(&b, 0), json!([null, null]));
    assert_eq!(crops(&b, 1)[1], json!({"zoom": 3.0, "x": 0.2, "y": 0.8}), "swapped with the photo in that cell: the crop went along");
    let b = run(&mut s, "book.movePhoto", json!({"from": {"page": 1, "cell": 1}, "to": {"unplaced": 0}}));
    assert_eq!(crops(&b, 1), json!([null, null]));
    // a template with fewer cells drops the crops of the photos that leave
    run(&mut s, "book.setCellCrop", json!({"page": 1, "cell": 0, "zoom": 2.0}));
    let b = run(&mut s, "book.setTemplate", json!({"page": 1, "template": "one"}));
    assert_eq!(crops(&b, 1), json!([{"zoom": 2.0, "x": 0.5, "y": 0.5}]));
    // reset
    let b = run(&mut s, "book.setCellCrop", json!({"page": 1, "cell": 0, "reset": true}));
    assert_eq!(crops(&b, 1), json!([null]));
}

#[test]
fn hostile_crops_pages_and_names_are_errors_that_change_nothing() {
    let mut s = Session::with_demo();
    six(&mut s);
    let before = run(&mut s, "book.get", json!({}));
    for (id, p) in [
        ("book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": 0.5})),
        ("book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": MAX_ZOOM + 1.0})),
        ("book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": 2.0, "x": 1.5})),
        ("book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": 2.0, "y": -0.1})),
        ("book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": "big"})),
        ("book.setCellCrop", json!({"page": 0, "cell": 0})),
        ("book.setCellCrop", json!({"page": 0, "cell": 7, "zoom": 2.0})),
        ("book.setCellCrop", json!({"page": 70, "cell": 0, "zoom": 2.0})),
        ("book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": 2.0, "extra": 1})),
        ("book.lockPage", json!({"page": 70})),
        ("book.lockPage", json!({"page": 0, "locked": "yes"})),
        ("book.addPage", json!({"template": "round"})),
        ("book.duplicatePage", json!({"page": -1})),
        ("book.movePage", json!({"from": 0})),
        ("book.rename", json!({"name": ""})),
        ("book.rename", json!({"name": "   "})),
        ("book.rename", json!({})),
        ("book.rename", json!({"name": 5})),
        ("book.duplicate", json!({"book": 77})),
    ] {
        fails(&mut s, id, p);
    }
    assert_eq!(run(&mut s, "book.get", json!({})), before);
    assert!(s.undo.iter().all(|e| e.book.is_none()) || s.undo.len() == 1, "only the create is a step");
    // an empty cell has no crop
    run(&mut s, "book.movePhoto", json!({"from": {"page": 0, "cell": 0}, "to": {"unplaced": 0}}));
    assert!(fails(&mut s, "book.setCellCrop", json!({"page": 0, "cell": 0, "zoom": 2.0})).contains("no photo"));
}

#[test]
fn a_book_name_is_one_no_other_book_has() {
    let mut s = Session::with_demo();
    let a = ids(&s, 2);
    run(&mut s, "book.create", json!({"name": "Alps", "ids": a}));
    run(&mut s, "book.create", json!({"name": "Coast", "ids": a}));
    assert!(fails(&mut s, "book.rename", json!({"book": 2, "name": "alps"})).contains("already"));
    assert!(fails(&mut s, "book.create", json!({"name": "ALPS", "ids": a})).contains("already"));
    assert!(fails(&mut s, "book.set", json!({"book": 2, "name": "Alps"})).contains("already"));
    // its own name, in another case, is fine
    run(&mut s, "book.rename", json!({"book": 2, "name": "COAST"}));
    // an empty book can be made, and photos added to it
    let b = run(&mut s, "book.create", json!({"name": "Empty", "empty": true}));
    assert_eq!((b["pages"].as_array().map(Vec::len), b["photos"].as_u64()), (Some(0), Some(0)));
    let b = run(&mut s, "book.addPhotos", json!({"ids": a}));
    assert_eq!(b["unplaced"].as_array().map(Vec::len), Some(2));
    let b = run(&mut s, "book.layout", json!({"cover": false, "preset": "two"}));
    assert_eq!(b["pages"][0]["slots"].as_array().map(Vec::len), Some(2));
}

#[test]
fn a_book_of_two_hundred_pages_is_a_book() {
    let mut s = Session::with_demo();
    let photos = ids(&s, 4);
    let mut book = Book { name: "Big".into(), ..Default::default() };
    book.pages = (0..MAX_PAGES).map(|i| Page::with(Template::Two, &[photos[i % 4], photos[(i + 1) % 4]])).collect();
    book.validate().unwrap();
    s.books.add(book).unwrap();
    let b = run(&mut s, "book.get", json!({}));
    assert_eq!(b["pages"].as_array().map(Vec::len), Some(MAX_PAGES));
    assert!(fails(&mut s, "book.addPage", json!({})).contains("at most 200"));
    assert!(fails(&mut s, "book.duplicatePage", json!({"page": 0})).contains("at most 200"));
    run(&mut s, "book.deletePage", json!({"page": 199}));
    run(&mut s, "book.duplicatePage", json!({"page": 0}));
}

/// The part of the photo a crop shows is computed once, from fractions of the photo and the
/// frame's aspect, so it is the same at any size and in the preview and the export.
#[test]
fn a_crop_window_is_in_fractions_of_the_photo() {
    let c = Crop { zoom: 2.0, x: 0.5, y: 0.5 };
    // a landscape 3:2 photo in a square frame: the full-height square, half of it at zoom 2
    let (x, y, w, h) = crop_window(1.5, 1.0, &Crop::FILL);
    assert!((w - 1.0 / 1.5).abs() < 1e-12 && h == 1.0 && (x - (1.0 - w) / 2.0).abs() < 1e-12 && y == 0.0);
    let (x, y, w, h) = crop_window(1.5, 1.0, &c);
    assert!((w - 1.0 / 3.0).abs() < 1e-12 && (h - 0.5).abs() < 1e-12 && (x - (0.5 - w / 2.0)).abs() < 1e-12 && (y - 0.25).abs() < 1e-12);
    // a window never leaves the photo
    let (x, y, w, h) = crop_window(1.5, 1.0, &Crop { zoom: 4.0, x: 0.0, y: 1.0 });
    assert_eq!((x, y), (0.0, 1.0 - h));
    assert!(w > 0.0 && x + w <= 1.0 + 1e-12);
    let (x, _, w, _) = crop_window(1.5, 1.0, &Crop { zoom: 4.0, x: 1.0, y: 0.5 });
    assert!((x + w - 1.0).abs() < 1e-12);
    // hostile numbers show the photo whole, never panic
    assert_eq!(crop_window(0.0, 1.0, &c), (0.0, 0.0, 1.0, 1.0));
    assert_eq!(crop_window(1.0, f64::NAN, &c), (0.0, 0.0, 1.0, 1.0));
    let (x, y, w, h) = crop_window(1.0, 1.0, &Crop { zoom: f64::NAN, x: f64::NAN, y: f64::INFINITY });
    assert!([x, y, w, h].iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
}

fn crop_page(s: &mut Session, photo: u64, ppi: f32, crop: Option<Crop>) -> crate::print::PageRaster {
    let mut book = Book { name: "C".into(), ..Default::default() };
    book.settings.ppi = ppi;
    book.settings.fill = true;
    book.pages = vec![Page::with(Template::One, &[photo])];
    book.pages[0].crops = vec![crop];
    book.validate().unwrap();
    render::prepare(s, &book).unwrap().run_rasters(&mut |_, _| true).unwrap().remove(0)
}

/// Mean absolute difference of two pages' samples, `a` being twice the size of `b`, as a fraction of full scale.
fn half_size_difference(a: &crate::print::PageRaster, b: &crate::print::PageRaster) -> f64 {
    assert_eq!((a.width, a.height), (b.width * 2, b.height * 2));
    let (w, h) = (b.width as usize, b.height as usize);
    let at = |p: &crate::print::PageRaster, x: usize, y: usize, c: usize| f64::from(p.rgb[(y * p.width as usize + x) * 3 + c]);
    let mut sum = 0.0;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let big = (at(a, 2 * x, 2 * y, c) + at(a, 2 * x + 1, 2 * y, c) + at(a, 2 * x, 2 * y + 1, c) + at(a, 2 * x + 1, 2 * y + 1, c)) / 4.0;
                sum += (big - at(b, x, y, c)).abs();
            }
        }
    }
    sum / (w * h * 3) as f64 / 65535.0
}

#[test]
fn a_crop_shows_the_same_part_of_the_photo_at_any_resolution() {
    let mut s = Session::with_demo();
    let photo = ids(&s, 1)[0];
    let crop = Crop { zoom: 2.5, x: 0.3, y: 0.6 };
    let (low, high) = (crop_page(&mut s, photo, 72.0, Some(crop)), crop_page(&mut s, photo, 144.0, Some(crop)));
    let same = half_size_difference(&high, &low);
    // what the crop changes is far more than what the resolution changes
    let plain = crop_page(&mut s, photo, 72.0, None);
    let plain_high = crop_page(&mut s, photo, 144.0, None);
    let resolution_only = half_size_difference(&plain_high, &plain);
    let cropped_vs_plain = half_size_difference(&high, &plain);
    assert!(same < 0.03, "the crop differs between 72 and 144 ppi: {same}");
    assert!(cropped_vs_plain > 3.0 * resolution_only.max(0.002), "the crop must show: {cropped_vs_plain} vs {resolution_only}");
    assert!(cropped_vs_plain > same * 2.0, "{cropped_vs_plain} vs {same}");
}

/// With Zoom to Fill Frame off (the default) a cropped photo still fills its whole slot, in the
/// export as on screen: the crop's window is cut from the photo for the slot, not for a box the
/// whole photo was fitted into first.
#[test]
fn a_crop_fills_its_slot_when_zoom_to_fill_is_off() {
    let mut book = Book { name: "F".into(), ..Default::default() };
    assert!(!book.settings.fill, "the default");
    book.pages = vec![Page::with(Template::Two, &[1, 2])];
    book.pages[0].crops = vec![Some(Crop { zoom: 2.0, x: 0.3, y: 0.6 }), None];
    book.validate().unwrap();
    let slots = crate::book::geometry::cells_for(Template::Two, &book.settings).unwrap();
    assert!(slots[0].h > slots[0].w, "a tall, non-square frame");
    let (settings, _) = render::print_settings(&book, &|_| Some(1.5)).unwrap();
    assert_eq!(settings.layout.cells[0].rect, slots[0], "the cropped cell is the whole slot");
    assert_ne!(settings.layout.cells[1].rect, slots[1], "an uncropped cell of the same page is still fitted");
}

#[test]
fn a_crop_with_zoom_to_fill_off_shows_the_same_part_at_any_resolution_and_reaches_the_slot_edge() {
    let mut s = Session::with_demo();
    let photo = ids(&s, 1)[0];
    let crop = Crop { zoom: 2.0, x: 0.3, y: 0.6 };
    let page = |s: &mut Session, ppi: f32, crop: Option<Crop>| {
        let mut book = Book { name: "C".into(), ..Default::default() };
        book.settings.ppi = ppi;
        book.pages = vec![Page::with(Template::Two, &[photo])];
        book.pages[0].crops = vec![crop, None];
        book.validate().unwrap();
        let slot = crate::book::geometry::cells_for(Template::Two, &book.settings).unwrap()[0];
        (render::prepare(s, &book).unwrap().run_rasters(&mut |_, _| true).unwrap().remove(0), slot)
    };
    let ((low, slot), (high, _)) = (page(&mut s, 72.0, Some(crop)), page(&mut s, 144.0, Some(crop)));
    let (plain, _) = page(&mut s, 72.0, None);
    let same = half_size_difference(&high, &low);
    assert!(same < 0.03, "the crop differs between 72 and 144 ppi: {same}");
    assert!(half_size_difference(&high, &plain) > same * 2.0, "the crop must show");
    // the photo reaches the corner of the slot (it fills it), where a fitted photo leaves paper
    let white = crate::print::compose::rec2020_from_srgb8([255; 3]);
    let px = |x: f64, y: f64| {
        let (px, py) = ((x * 72.0) as usize, (y * 72.0) as usize);
        let at = (py * low.width as usize + px) * 3;
        [low.rgb[at], low.rgb[at + 1], low.rgb[at + 2]]
    };
    assert_ne!(px(slot.x + 0.04, slot.y + 0.04), white);
    assert_ne!(px(slot.x + slot.w - 0.04, slot.y + slot.h - 0.04), white);
}

#[test]
fn an_empty_book_made_with_photos_is_an_error() {
    let mut s = Session::with_demo();
    let a = ids(&s, 2);
    let e = fails(&mut s, "book.create", json!({"empty": true, "ids": a}));
    assert!(e.contains("empty"), "{e}");
    fails(&mut s, "book.create", json!({"empty": true, "source": "view"}));
    run(&mut s, "book.create", json!({"empty": true}));
}

#[test]
fn a_copy_of_a_book_with_a_long_name_still_has_a_name_the_file_accepts() {
    let files = MemStore::new();
    let mut s = open(&files);
    let a = ids(&s, 2);
    let long = "x".repeat(crate::book::MAX_NAME);
    run(&mut s, "book.create", json!({"name": long, "ids": a}));
    let copy = run(&mut s, "book.duplicate", json!({"book": 1}));
    let copy2 = run(&mut s, "book.duplicate", json!({"book": 1}));
    for c in [&copy, &copy2] {
        let name = c["name"].as_str().unwrap();
        assert!(name.chars().count() <= crate::book::MAX_NAME, "{name}");
        assert!(name.contains("Copy"));
    }
    assert_ne!(copy["name"], copy2["name"]);
    drop(s);
    let mut again = open(&files);
    assert_eq!(run(&mut again, "book.list", json!({}))["books"].as_array().unwrap().len(), 3, "the next launch reads all three");
    assert!(again.take_library_warnings().is_empty());
    // `Books::add` itself refuses a name the file would not read back
    let mut books = crate::book::Books::default();
    let bad = Book { name: "y".repeat(crate::book::MAX_NAME + 1), ..Default::default() };
    assert!(books.add(bad).is_err());
}

#[test]
fn a_save_never_makes_a_file_the_reader_refuses() {
    use crate::book::store::MAX_FILE;
    // the largest library the limits allow, every cell cropped, still fits what the reader takes
    let mut books = crate::book::Books::default();
    for b in 0..crate::book::MAX_BOOKS {
        let mut book = Book { name: format!("Book {b}"), ..Default::default() };
        book.pages = (0..MAX_PAGES)
            .map(|i| {
                let mut p = Page::with(Template::Grid4, &[1, 2, 3, 4].map(|k| (i * 4 + k) as u64));
                p.crops = vec![Some(Crop { zoom: 1.2345678901234567, x: 0.123456789012345, y: 0.987654321098765 }); 4];
                p
            })
            .collect();
        book.validate().unwrap();
        books.add(book).unwrap();
    }
    let file = crate::book::store::BooksFile { version: 2, library: books };
    let bytes = serde_json::to_vec_pretty(&file).unwrap();
    assert!(bytes.len() < MAX_FILE, "{} bytes", bytes.len());
    // and a save over the limit is refused with a message, the books staying as they were
    let mut s = Session::with_demo();
    let a = ids(&s, 2);
    run(&mut s, "book.create", json!({"name": "A", "ids": a}));
    let before = s.books.clone();
    s.books.books[0].name = "Renamed".into();
    let e = s.save_books_within(before.clone(), 64).unwrap_err().to_string();
    assert!(e.contains("too large"), "{e}");
    assert_eq!(s.books, before);
}

#[test]
fn closing_a_library_forgets_the_book_steps() {
    let files = MemStore::new();
    let mut s = open(&files);
    six(&mut s);
    run(&mut s, "book.rename", json!({"name": "Renamed"}));
    assert!(s.undo.iter().any(|e| e.book.is_some()));
    s.close_library().unwrap();
    assert!(s.undo.iter().chain(&s.redo).all(|e| e.book.is_none()));
}

#[test]
fn switching_books_needs_no_save() {
    let files = MemStore::new();
    files.set("books.json", serde_json::to_vec(&json!({"version": 9})).unwrap());
    let mut s = open(&files);
    // the file is blocked: nothing can be saved, but a book made in memory can still be switched to
    s.books.add(Book { name: "A".into(), ..Default::default() }).unwrap();
    s.books.add(Book { name: "B".into(), ..Default::default() }).unwrap();
    let r = run(&mut s, "book.open", json!({"book": 1}));
    assert_eq!(r["id"], 1);
    assert_eq!(s.books.current, Some(1));
}

#[test]
fn a_pdf_over_the_limit_says_how_many_pages_fit() {
    let e = render::too_big(10 << 20, 50);
    assert!(e.contains("at most 25 pages fit"), "{e}");
    assert!(e.contains("500 MiB") && e.contains("256 MiB"), "{e}");
}

/// The PDF is made a page at a time and still has every page, at the page size.
#[test]
fn the_pdf_streams_its_pages_and_keeps_them_all() {
    let mut s = Session::with_demo();
    let photos = ids(&s, 6);
    let mut book = Book { name: "P".into(), ..Default::default() };
    book.settings.ppi = 72.0;
    book.pages = (0..6).map(|i| Page::with(Template::One, &photos[i..i + 1])).collect();
    book.validate().unwrap();
    let mut seen = Vec::new();
    let pdf = render::prepare(&mut s, &book)
        .unwrap()
        .run_pdf(&mut |done, _| {
            seen.push(done);
            true
        })
        .unwrap();
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches("/Type /Page ").count(), 6);
    assert!(seen.len() >= 6);
    let jpegs = render::prepare(&mut s, &book).unwrap().run_jpegs(80, &mut |_, _| true).unwrap();
    assert_eq!(jpegs.len(), 6);
    assert!(jpegs.iter().all(|j| j.starts_with(&[0xFF, 0xD8])));
}
