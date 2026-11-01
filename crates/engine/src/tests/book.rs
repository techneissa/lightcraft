//! The `book.*` commands: creating from photos, settings, templates, moving photos, saving with
//! the library, the PDF and JPEG output, and input that must be an error, never a panic.

use lightcraft_catalog::{MemStore, Op, Photo, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::book::render;
use crate::library::LibraryStores;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn fails(s: &mut Session, id: &str, p: Value) -> String {
    s.execute(id, &p).expect_err(&format!("{id} {p} should fail")).to_string()
}

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lightcraft-book-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn demo_ids(s: &Session, n: usize) -> Vec<u64> {
    s.catalog.photos().take(n).map(|p| p.id.0).collect()
}

/// A photo in the catalog whose file does not exist.
fn add_missing(s: &mut Session) -> u64 {
    let id = s.catalog.alloc_photo_id();
    let gone = std::env::temp_dir().join("lightcraft-book-no-such-folder").join("gone.jpg").to_string_lossy().to_string();
    s.catalog
        .apply(Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: gone }, "gone.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00")) })
        .unwrap();
    id.0
}

fn open(files: &MemStore) -> Session {
    let mut s = Session::with_demo();
    let stores = LibraryStores { dir: "book".into(), catalog: Box::new(MemStore::new()), files: Box::new(files.clone()), on_disk: false };
    s.open_library_in(stores, false).unwrap();
    crate::demo::load(&mut s);
    s
}

fn slots(book: &Value) -> Vec<Vec<Value>> {
    book["pages"].as_array().unwrap().iter().map(|p| p["slots"].as_array().unwrap().clone()).collect()
}

fn photo_ids(book: &Value) -> Vec<u64> {
    slots(book).concat().iter().filter_map(Value::as_u64).collect()
}

#[test]
fn create_lays_the_photos_out_and_reports_the_pages() {
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 7);
    let b = run(&mut s, "book.create", json!({"name": "Summer", "ids": ids}));
    assert_eq!((b["name"].as_str(), b["id"].as_u64(), b["current"].as_bool()), (Some("Summer"), Some(1), Some(true)));
    // cover, one, two, three: seven photos
    let templates: Vec<&str> = b["pages"].as_array().unwrap().iter().map(|p| p["template"].as_str().unwrap()).collect();
    assert_eq!(templates, ["fullBleed", "one", "two", "three"]);
    assert_eq!((b["pageWidth"].as_f64(), b["pageHeight"].as_f64()), (Some(10.0), Some(8.0)));
    assert_eq!(b["photos"], 7);
    assert_eq!(b["spreads"], json!([[0], [1, 2], [3]]));
    assert_eq!(photo_ids(&b), ids, "every photo once, in order");
    assert_eq!(b["pages"][2]["cells"].as_array().unwrap().len(), 2);
    assert!(b["sizes"].as_array().unwrap().iter().any(|z| z["name"] == "a4"));
    // a second book becomes current; the list names both
    let second = run(&mut s, "book.create", json!({"ids": &ids[..2], "cover": false, "preset": "one"}));
    assert_eq!(second["name"], "Untitled Book");
    assert_eq!(run(&mut s, "book.create", json!({"ids": &ids[..1]}))["name"], "Untitled Book 2");
    let l = run(&mut s, "book.list", json!({}));
    assert_eq!(l["books"].as_array().unwrap().len(), 3);
    assert_eq!(l["current"], 3);
    run(&mut s, "book.open", json!({"book": 1}));
    assert_eq!(run(&mut s, "book.get", json!({}))["name"], "Summer");
    run(&mut s, "book.delete", json!({"book": 1}));
    assert_eq!(run(&mut s, "book.list", json!({}))["current"], 2, "current moves on");
}

#[test]
fn the_photos_come_from_ids_the_selection_or_the_view() {
    let mut s = Session::with_demo();
    let all = s.visible_cloned();
    assert!(all.len() > 3);
    // no selection: the photos in view
    s.selection = crate::Selection::default();
    let b = run(&mut s, "book.create", json!({"cover": false, "preset": "one"}));
    assert_eq!(b["photos"].as_u64().unwrap() as usize, all.len());
    // a selection: just those, in selection order
    s.selection = crate::Selection { ids: vec![all[2], all[0]], active: Some(all[2]) };
    let b = run(&mut s, "book.create", json!({"cover": false, "preset": "one"}));
    assert_eq!(photo_ids(&b), [all[2].0, all[0].0]);
    // source=view ignores the selection
    let b = run(&mut s, "book.create", json!({"cover": false, "preset": "one", "source": "view"}));
    assert_eq!(b["photos"].as_u64().unwrap() as usize, all.len());
    fails(&mut s, "book.create", json!({"source": "album"}));
}

#[test]
fn settings_change_the_geometry_and_are_checked() {
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 3);
    run(&mut s, "book.create", json!({"ids": ids, "cover": false, "preset": "one"}));
    let b = run(
        &mut s,
        "book.set",
        json!({"size": "a4", "orientation": "portrait", "margin": 0.25, "padding": 0.0, "background": [10, 20, 30], "fill": true, "ppi": 100.0, "name": "Album"}),
    );
    assert_eq!(b["name"], "Album");
    assert_eq!(b["settings"]["background"], json!([10, 20, 30]));
    assert!((b["pageWidth"].as_f64().unwrap() - 210.0 / 25.4).abs() < 1e-9);
    let cell = &b["pages"][0]["cells"][0];
    assert_eq!(cell["x"], 0.25);
    assert!((cell["w"].as_f64().unwrap() - (210.0 / 25.4 - 0.5)).abs() < 1e-9);
    // nested form, and padding insets the cell
    let b = run(&mut s, "book.set", json!({"settings": {"padding": 0.5, "margin": 0.0}}));
    assert_eq!(b["pages"][0]["cells"][0]["x"], 0.5);
    for bad in [
        json!({"size": "poster"}),
        json!({"margin": -1}),
        json!({"margin": 9}),
        json!({"padding": "wide"}),
        json!({"ppi": 5}),
        json!({"background": [1, 2]}),
        json!({"orientation": "sideways"}),
        json!({"colour": 1}),
        json!({"name": ""}),
        json!({"book": 99, "fill": true}),
        json!({"book": "x"}),
        json!({"size": "5x7", "margin": 2.0, "padding": 1.0}),
    ] {
        fails(&mut s, "book.set", bad);
    }
    assert_eq!(run(&mut s, "book.get", json!({}))["settings"]["padding"], 0.5, "an error changed nothing");
}

#[test]
fn templates_change_and_photos_move() {
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 5);
    run(&mut s, "book.create", json!({"ids": ids, "cover": false, "preset": "mixed"})); // one, two, two
    let b = run(&mut s, "book.get", json!({}));
    assert_eq!(b["pages"].as_array().unwrap().len(), 3);
    let b = run(&mut s, "book.setTemplate", json!({"page": 1, "template": "one"}));
    assert_eq!(b["unplaced"], json!([ids[2]]));
    assert_eq!(b["pages"][1]["template"], "one");
    // the tray photo goes onto a page by drag: it takes the cell, the displaced photo is unplaced
    let b = run(&mut s, "book.movePhoto", json!({"from": {"unplaced": 0}, "to": {"page": 2, "cell": 0}}));
    assert_eq!(b["pages"][2]["slots"], json!([ids[2], ids[4]]));
    assert_eq!(b["unplaced"], json!([ids[3]]));
    // swap two cells across pages
    let b = run(&mut s, "book.movePhoto", json!({"from": {"page": 0, "cell": 0}, "to": {"page": 2, "cell": 1}}));
    assert_eq!((b["pages"][0]["slots"][0].as_u64(), b["pages"][2]["slots"][1].as_u64()), (Some(ids[4]), Some(ids[0])));
    for (id, p) in [
        ("book.setTemplate", json!({"page": 9, "template": "one"})),
        ("book.setTemplate", json!({"page": -1, "template": "one"})),
        ("book.setTemplate", json!({"page": 0, "template": "poster"})),
        ("book.setTemplate", json!({"page": 0})),
        ("book.movePhoto", json!({"from": {"page": 9, "cell": 0}, "to": {"page": 0, "cell": 0}})),
        ("book.movePhoto", json!({"from": {"page": 0}, "to": {"page": 0, "cell": 0}})),
        ("book.movePhoto", json!({"from": {"unplaced": 7}, "to": {"page": 0, "cell": 0}})),
        ("book.movePhoto", json!({"from": {"page": 0, "cell": 0}})),
        ("book.movePhoto", json!({"from": 1, "to": 2})),
        ("book.layout", json!({"preset": "spiral"})),
        ("book.layout", json!({"cover": "yes"})),
    ] {
        fails(&mut s, id, p);
    }
    // auto layout again keeps every photo (pages first, then the tray)
    let before = photo_ids(&run(&mut s, "book.get", json!({})));
    let b = run(&mut s, "book.layout", json!({"preset": "two", "cover": true}));
    let after = photo_ids(&b);
    assert_eq!(after.len(), 5);
    assert_eq!(after[..before.len()], before[..], "same order, same photos");
    assert_eq!(b["cover"], true);
    assert_eq!(b["unplaced"], json!([]));
    // new photos replace the old ones
    let b = run(&mut s, "book.layout", json!({"ids": &ids[..2], "cover": false, "preset": "grid4"}));
    assert_eq!(photo_ids(&b), &ids[..2]);
}

#[test]
fn hostile_input_is_an_error_and_changes_nothing() {
    let mut s = Session::with_demo();
    // no book yet
    for id in ["book.get", "book.open", "book.delete", "book.set", "book.layout", "book.exportPdf", "book.exportJpeg"] {
        fails(&mut s, id, json!({"path": "/tmp/x.pdf", "dir": "/tmp/x", "book": null}));
    }
    assert_eq!(run(&mut s, "book.list", json!({}))["books"], json!([]));
    let one = demo_ids(&s, 1);
    // 0 photos, unknown photos, wrong shapes, too many
    for p in [
        json!({"ids": []}),
        json!({"ids": [999_999_999]}),
        json!({"ids": ["a"]}),
        json!({"ids": 5}),
        json!({"id": "x"}),
        json!({"ids": one, "name": "\u{0}"}),
        json!({"ids": one, "preset": 4}),
        json!({"ids": one, "cover": 1}),
        json!({"ids": one, "settings": {"ppi": 0}}),
        json!({"ids": one, "typo": 1}),
        json!({"ids": (0..100_000).collect::<Vec<u64>>()}),
        json!(7),
    ] {
        fails(&mut s, "book.create", p);
    }
    assert_eq!(run(&mut s, "book.list", json!({}))["books"], json!([]), "nothing was created");
    // more photos than 200 pages can hold at one photo per page
    let all: Vec<u64> = s.catalog.photos().map(|p| p.id.0).collect();
    let many: Vec<u64> = all.iter().copied().cycle().take(201).collect();
    let e = fails(&mut s, "book.create", json!({"ids": many, "preset": "one", "cover": false}));
    assert!(e.contains("200 pages"), "{e}");
}

#[test]
fn a_book_is_saved_with_the_library_and_reopens() {
    let files = MemStore::new();
    let mut s = open(&files);
    let ids = demo_ids(&s, 5);
    run(&mut s, "book.create", json!({"name": "Kept", "ids": ids}));
    run(&mut s, "book.set", json!({"background": [1, 2, 3], "padding": 0.3}));
    run(&mut s, "book.setTemplate", json!({"page": 1, "template": "grid4"}));
    run(&mut s, "book.create", json!({"name": "Other", "ids": &ids[..2]}));
    run(&mut s, "book.open", json!({"book": 1}));
    let saved = run(&mut s, "book.get", json!({}));
    assert!(files.get("books.json").is_some());

    let mut again = open(&files);
    assert!(again.take_library_warnings().is_empty());
    assert_eq!(run(&mut again, "book.get", json!({})), saved);
    assert_eq!(run(&mut again, "book.list", json!({}))["books"].as_array().unwrap().len(), 2);
    // delete is saved too
    run(&mut again, "book.delete", json!({"book": 2}));
    assert_eq!(run(&mut open(&files), "book.list", json!({}))["books"].as_array().unwrap().len(), 1);
    // another library has none of them
    let mut other = open(&MemStore::new());
    assert_eq!(run(&mut other, "book.list", json!({}))["books"], json!([]));
}

#[test]
fn a_damaged_or_newer_books_file_is_never_overwritten() {
    for (bytes, why) in [
        (br#"{"version": 1, "library": {"books": [{"id": 1"#.to_vec(), "damaged"),
        (serde_json::to_vec(&json!({"version": 9})).unwrap(), "newer"),
        (
            serde_json::to_vec(&json!({"version": 1, "library": {"books": [{"id": 1, "name": "x", "pages": [{"template": "grid4", "slots": []}]}]}}))
                .unwrap(),
            "broken",
        ),
    ] {
        let files = MemStore::new();
        files.set("books.json", bytes.clone());
        let mut s = open(&files);
        let warnings = s.take_library_warnings();
        assert_eq!(warnings.len(), 1, "{why}: {warnings:?}");
        assert!(warnings[0].contains("books.json"), "{why}: {warnings:?}");
        assert_eq!(run(&mut s, "book.list", json!({}))["books"], json!([]), "{why}");
        if why != "damaged" {
            // the file stays as it is: a new book is refused rather than saved over it
            let ids = demo_ids(&s, 1);
            let e = fails(&mut s, "book.create", json!({"ids": ids}));
            assert!(e.contains("books.json"), "{why}: {e}");
            assert_eq!(files.get("books.json").unwrap(), bytes, "{why}");
            assert_eq!(run(&mut s, "book.list", json!({}))["books"], json!([]), "{why}: the failed save was rolled back");
        }
    }
}

/// The first MediaBox: width and height in points.
fn media_box(pdf: &[u8]) -> (f64, f64) {
    let text = String::from_utf8_lossy(pdf);
    let at = text.find("/MediaBox [").expect("a MediaBox") + "/MediaBox [".len();
    let nums: Vec<f64> = text[at..].split(']').next().unwrap().split_whitespace().map(|v| v.parse().unwrap()).collect();
    (nums[2], nums[3])
}

#[test]
fn export_pdf_has_a_page_per_book_page_at_the_page_size() {
    let dir = Scratch::new("pdf");
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 6);
    run(&mut s, "book.create", json!({"name": "Out", "ids": ids, "settings": {"ppi": 72.0}}));
    let path = dir.path("out.pdf");
    let r = run(&mut s, "book.exportPdf", json!({"path": path}));
    assert_eq!((r["pages"].as_u64(), r["photos"].as_u64()), (Some(4), Some(6)));
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len() as u64, r["bytes"].as_u64().unwrap());
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("%PDF-") && text.contains("/Count 4"), "four pages");
    assert_eq!(media_box(&bytes), (720.0, 576.0), "10 x 8 in landscape");
    assert!(s.activity.list().is_empty(), "the activity row is gone when the export is done");

    // another size and orientation: A5 portrait, 148 x 210 mm
    run(&mut s, "book.set", json!({"size": "a5", "orientation": "portrait"}));
    run(&mut s, "book.exportPdf", json!({"path": path}));
    let (w, h) = media_box(&std::fs::read(&path).unwrap());
    assert!((w - 419.5).abs() < 2.0 && (h - 595.3).abs() < 2.0, "{w} x {h}");

    for p in [json!({}), json!({"path": "  "}), json!({"path": path, "typo": 1}), json!({"path": path, "book": 77})] {
        fails(&mut s, "book.exportPdf", p);
    }
}

/// Width and height from a JPEG's frame header.
fn jpeg_size(b: &[u8]) -> (u32, u32) {
    let at = b.windows(2).position(|w| w[0] == 0xFF && matches!(w[1], 0xC0..=0xC2)).expect("a frame header");
    let n = |i: usize| u32::from(b[at + i]) << 8 | u32::from(b[at + i + 1]);
    (n(7), n(5))
}

#[test]
fn export_jpeg_writes_one_file_per_page() {
    let dir = Scratch::new("jpeg");
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 6);
    run(&mut s, "book.create", json!({"name": "Per/Page", "ids": ids, "settings": {"ppi": 72.0}}));
    let r = run(&mut s, "book.exportJpeg", json!({"dir": dir.path("pages"), "quality": 80}));
    let files: Vec<String> = r["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
    assert_eq!(files.len(), 4);
    assert!(files[0].ends_with("Per_Page-01.jpg") && files[3].ends_with("Per_Page-04.jpg"), "{files:?}");
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        assert_eq!(bytes[..2], [0xFF, 0xD8], "a JPEG");
        assert_eq!(jpeg_size(&bytes), (720, 576));
    }
    fails(&mut s, "book.exportJpeg", json!({}));
    fails(&mut s, "book.exportJpeg", json!({"dir": dir.path("x"), "quality": 0}));
    fails(&mut s, "book.exportJpeg", json!({"dir": dir.path("x"), "quality": 101}));
    assert!(s.activity.list().is_empty());
}

#[test]
fn a_missing_photo_is_an_error_not_a_panic() {
    let dir = Scratch::new("missing");
    let mut s = Session::with_demo();
    let mut ids = demo_ids(&s, 2);
    ids.insert(1, add_missing(&mut s));
    run(&mut s, "book.create", json!({"ids": ids, "settings": {"ppi": 72.0}}));
    // its file is gone
    let e = fails(&mut s, "book.exportPdf", json!({"path": dir.path("m.pdf")}));
    assert!(e.contains("gone.jpg"), "{e}");
    assert!(!std::path::Path::new(&dir.path("m.pdf")).exists(), "nothing written");
    fails(&mut s, "book.exportJpeg", json!({"dir": dir.path("m")}));
    assert!(s.activity.list().is_empty(), "the row goes away on failure");
    // the photo is not in the catalog any more (here: the book names an id the catalog lacks)
    if let Some(b) = s.books.books.first_mut() {
        b.pages[0].slots[0] = Some(987_654_321);
    }
    let b = run(&mut s, "book.get", json!({}));
    assert_eq!(b["missing"], json!([987_654_321u64]));
    let e = fails(&mut s, "book.exportPdf", json!({"path": dir.path("m.pdf")}));
    assert!(e.contains("987654321"), "{e}");
}

#[test]
fn a_stopped_export_leaves_no_file_and_no_row() {
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 3);
    run(&mut s, "book.create", json!({"ids": ids, "settings": {"ppi": 72.0}}));
    let book = s.books.find(None).unwrap().clone();
    let job = render::prepare(&mut s, &book).unwrap();
    assert!(job.steps() >= 3);
    let mut seen = 0;
    let e = job.run(&mut |_, _| {
        seen += 1;
        false
    });
    assert_eq!(e.err().as_deref(), Some(render::CANCELLED));
    assert_eq!(seen, 1, "it stops at the first check");
    // running to the end gives one raster per page
    let rendered = render::prepare(&mut s, &book).unwrap().run(&mut |_, _| true).unwrap();
    assert_eq!(rendered.pages.len(), book.pages.len());
    assert!(s.activity.list().is_empty());
}

#[test]
fn fit_shows_the_whole_photo_and_fill_crops_to_the_slot() {
    let mut book = crate::book::Book { name: "A".into(), ..Default::default() };
    book.relayout(&[1, 2, 3], false, crate::book::edit::Preset::One).unwrap();
    book.pages[2] = crate::book::Page::with(crate::book::Template::FullBleed, &[3]);
    book.settings.background = [9, 8, 7];
    book.settings.ppi = 200.0;
    let (settings, ids) = render::print_settings(&book, &|_| Some(1.0)).unwrap();
    assert_eq!(ids.iter().map(|i| i.0).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!((settings.layout.page.custom_width, settings.layout.page.custom_height), (10.0, 8.0));
    assert_eq!((settings.page.background, settings.job.ppi, settings.layout.pages), ([9, 8, 7], 200.0, 3));
    assert!(settings.image.zoom_to_fill && !settings.image.rotate_to_fit);
    // a square photo in the 8.8 x 6.8 slot is 6.8 x 6.8, centred; the composer then fills a cell of its own aspect
    let c = &settings.layout.cells[0].rect;
    assert!((c.w - 6.8).abs() < 1e-9 && (c.h - c.w).abs() < 1e-9, "{c:?}");
    assert!((c.x + c.w / 2.0 - 5.0).abs() < 1e-9, "centred");
    // full bleed covers the page whatever the setting
    assert_eq!(settings.layout.cells[2].rect, crate::print::layout::Rect::new(0.0, 0.0, 10.0, 8.0));
    // fill: the slot itself
    book.settings.fill = true;
    let (settings, _) = render::print_settings(&book, &|_| Some(1.0)).unwrap();
    let slot = crate::book::geometry::cells_for(crate::book::Template::One, &book.settings).unwrap()[0];
    assert_eq!(settings.layout.cells[0].rect, slot);
    // unknown aspect: the slot; empty slots print nothing; a book without photos has nothing to print
    book.settings.fill = false;
    let (settings, _) = render::print_settings(&book, &|_| None).unwrap();
    assert_eq!(settings.layout.cells[0].rect, slot);
    book.pages[0].slots[0] = None;
    assert_eq!(render::print_settings(&book, &|_| None).unwrap().1.len(), 2);
    book.pages.truncate(1);
    assert!(render::print_settings(&book, &|_| None).unwrap_err().contains("no photos"));
    // settings that did not come from the setters are checked
    book.settings.ppi = f32::NAN;
    assert!(render::print_settings(&book, &|_| None).is_err());
}

#[test]
fn every_book_command_is_registered_with_a_description() {
    for id in [
        "book.list",
        "book.get",
        "book.create",
        "book.open",
        "book.delete",
        "book.set",
        "book.layout",
        "book.setTemplate",
        "book.movePhoto",
        "book.exportPdf",
        "book.exportJpeg",
    ] {
        let spec = crate::cmd::find_command(id).unwrap_or_else(|| panic!("{id} is not registered"));
        assert!(spec.params.len() > 10, "{id} has no description");
    }
}

fn corrupt_copies(files: &MemStore) -> Vec<String> {
    files.files.lock().unwrap().keys().filter(|k| k.starts_with("books.json.corrupt")).cloned().collect()
}

/// A books.json from a newer LightCraft may use keys this build does not know: it is a newer file,
/// not a damaged one, so it is left exactly as it is, no copy is made and no save replaces it.
#[test]
fn a_newer_books_file_with_new_keys_is_left_untouched() {
    let newer = serde_json::to_vec(&json!({
        "version": 7, "extra": true,
        "library": {"books": [{"id": 1, "name": "x", "flavour": "new", "pages": []}], "shelf": 1}
    }))
    .unwrap();
    let files = MemStore::new();
    files.set("books.json", newer.clone());
    let mut s = open(&files);
    let warnings = s.take_library_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("books.json") && warnings[0].contains("newer") && warnings[0].contains("untouched"), "{warnings:?}");
    assert!(corrupt_copies(&files).is_empty(), "a newer file is not damaged: no copy");
    let ids = demo_ids(&s, 2);
    let e = fails(&mut s, "book.create", json!({"ids": ids}));
    assert!(e.contains("books.json") && e.contains("untouched"), "{e}");
    assert_eq!(files.get("books.json").unwrap(), newer, "never overwritten");
    assert!(corrupt_copies(&files).is_empty());
    // opening again changes nothing either
    drop(open(&files));
    assert_eq!(files.get("books.json").unwrap(), newer);
    assert!(corrupt_copies(&files).is_empty());
}

#[test]
fn a_huge_books_file_is_not_read_and_blocks_saves() {
    let files = MemStore::new();
    let huge = vec![b' '; crate::book::store::MAX_FILE + 1];
    files.set("books.json", huge.clone());
    let mut s = open(&files);
    let warnings = s.take_library_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("books.json") && warnings[0].contains("untouched"), "{warnings:?}");
    let ids = demo_ids(&s, 1);
    fails(&mut s, "book.create", json!({"ids": ids}));
    assert_eq!(files.get("books.json").unwrap(), huge);
}

#[test]
fn book_set_rejects_mistyped_settings_and_names() {
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 2);
    run(&mut s, "book.create", json!({"ids": ids}));
    for p in [
        json!({"settings": null, "margin": 1}),
        json!({"settings": 5}),
        json!({"settings": [1]}),
        json!({"settings": "x"}),
        json!({"name": 5}),
        json!({"name": null}),
        json!({"name": ["a"]}),
    ] {
        fails(&mut s, "book.set", p);
    }
    assert_eq!(run(&mut s, "book.get", json!({}))["settings"]["margin"], 0.5);
    run(&mut s, "book.set", json!({"settings": {"margin": 1.0}}));
    assert_eq!(run(&mut s, "book.get", json!({}))["settings"]["margin"], 1.0);
}

#[test]
fn a_photo_taken_off_a_page_goes_to_the_end_of_the_tray_and_the_tray_index_is_checked() {
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 4);
    run(&mut s, "book.create", json!({"ids": ids, "cover": false, "preset": "two"}));
    run(&mut s, "book.movePhoto", json!({"from": {"page": 0, "cell": 0}, "to": {"unplaced": 0}}));
    // the tray holds one photo: the next one lands at the end, which is index 1 (or the end: `unplaced` = its length)
    let b = run(&mut s, "book.movePhoto", json!({"from": {"page": 1, "cell": 0}, "to": {"unplaced": 1}}));
    assert_eq!(b["unplaced"], json!([ids[0], ids[2]]));
    // an index past the end of the tray is an error, and nothing moves
    let before = run(&mut s, "book.get", json!({}));
    fails(&mut s, "book.movePhoto", json!({"from": {"page": 0, "cell": 1}, "to": {"unplaced": 9}}));
    assert_eq!(run(&mut s, "book.get", json!({})), before);
    // the index places the photo there
    let b = run(&mut s, "book.movePhoto", json!({"from": {"page": 0, "cell": 1}, "to": {"unplaced": 0}}));
    assert_eq!(b["unplaced"], json!([ids[1], ids[0], ids[2]]));
}

/// What the export draws on each page is what the model says: a full-bleed page reaches the paper's
/// corner, a grid page keeps its margin and has a photo in each of its four cells.
#[test]
fn an_exported_page_has_the_template_the_model_says() {
    use crate::book::{Book, Page, Template};
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 5);
    let mut book = Book { name: "T".into(), cover: true, ..Default::default() };
    book.settings.ppi = 72.0;
    book.settings.fill = true;
    book.pages = vec![Page::with(Template::FullBleed, &ids[..1]), Page::with(Template::Grid4, &ids[1..5])];
    book.validate().unwrap();
    let rendered = render::prepare(&mut s, &book).unwrap().run(&mut |_, _| true).unwrap();
    assert_eq!(rendered.pages.len(), 2);
    let white = crate::print::compose::rec2020_from_srgb8([255; 3]);
    let px = |page: usize, x: u32, y: u32| {
        let p = &rendered.pages[page];
        assert_eq!((p.width, p.height), (720, 576));
        let at = ((y * p.width + x) * 3) as usize;
        [p.rgb[at], p.rgb[at + 1], p.rgb[at + 2]]
    };
    assert_ne!(px(0, 1, 1), white, "the cover reaches the corner of the paper");
    assert_ne!(px(0, 718, 574), white);
    assert_eq!(px(1, 1, 1), white, "a grid page keeps its margin");
    assert_eq!(px(1, 718, 574), white);
    // the centre of each cell of the 2 x 2 grid holds a photo; the gutter between them does not
    for (x, y) in [(190, 150), (530, 150), (190, 430), (530, 430)] {
        assert_ne!(px(1, x, y), white, "cell at {x},{y}");
    }
    assert_eq!(px(1, 360, 288), white, "the middle of the page is the gutter");
}

#[test]
fn the_cover_command_keeps_hand_made_placements() {
    let mut s = Session::with_demo();
    let ids = demo_ids(&s, 5);
    run(&mut s, "book.create", json!({"ids": ids, "cover": false, "preset": "two"}));
    run(&mut s, "book.movePhoto", json!({"from": {"page": 0, "cell": 0}, "to": {"page": 1, "cell": 1}}));
    let before = slots(&run(&mut s, "book.get", json!({})));
    let b = run(&mut s, "book.setCover", json!({"cover": true}));
    assert_eq!(b["cover"], true);
    assert_eq!(slots(&b)[1..], before[..]);
    assert_eq!(b["pages"][0]["template"], "fullBleed");
    let b = run(&mut s, "book.setCover", json!({"cover": false}));
    assert_eq!(slots(&b), before);
    fails(&mut s, "book.setCover", json!({}));
    fails(&mut s, "book.setCover", json!({"cover": "yes"}));
}
