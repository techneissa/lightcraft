//! The full-screen preview's pages: composed by the export's own code, so what the preview shows is
//! what prints.
use serde_json::{Value, json};

use crate::Session;
use crate::book::geometry::crop_window;
use crate::book::{Book, Crop, Page, Template, preview, render};
use crate::print::output::page_to_srgb8;

fn demo_book(s: &mut Session) -> Book {
    let ids: Vec<u64> = s.catalog.photos().take(4).map(|p| p.id.0).collect();
    let v: Value = s.execute("book.create", &json!({"name": "P", "ids": ids, "cover": false, "preset": "two"})).unwrap();
    let id = v["id"].as_u64().unwrap() as u32;
    s.books.find(Some(id)).unwrap().clone()
}

#[test]
fn a_preview_page_is_the_export_page() {
    // whole photos (the book's setting), then filled frames and a full-bleed page
    for fill in [false, true] {
        a_preview_page_is_the_export_page_with(fill);
    }
}

fn a_preview_page_is_the_export_page_with(fill: bool) {
    let mut s = Session::with_demo();
    let mut book = demo_book(&mut s);
    book.settings.ppi = 72.0;
    book.settings.fill = fill;
    if fill {
        book.pages[1].template = Template::FullBleed;
        book.pages[1].slots.truncate(1);
        book.pages[1].crops.truncate(1);
    }
    // a cropped cell too: the export draws it itself, and so must the preview
    book.pages[0].crops = vec![Some(Crop { zoom: 2.0, x: 0.3, y: 0.6 }), None];
    book.validate().unwrap();
    let export = render::prepare(&mut s, &book).unwrap().run_rasters(&mut |_, _| true).unwrap();
    let (pw, ph) = book.settings.page_size().unwrap();
    let edge = (pw.max(ph) * 72.0).round() as usize;
    for (index, raster) in export.iter().enumerate() {
        let shown = preview::prepare_page(&mut s, &book, index, edge).unwrap().run(&|| false).unwrap();
        let want = page_to_srgb8(raster).unwrap();
        assert_eq!((shown.width, shown.height), (want.width, want.height), "page {index}");
        assert_eq!(shown.rgba, want.data.into_flattened(), "page {index} differs from the export");
    }
    // (the crop is a real one: the window is a part of the photo)
    assert!(crop_window(1.5, 1.0, &Crop { zoom: 2.0, x: 0.3, y: 0.6 }).2 < 1.0);
}

#[test]
fn the_page_size_follows_the_screen() {
    let mut s = Session::with_demo();
    let book = demo_book(&mut s);
    let (pw, ph) = book.settings.page_size().unwrap();
    let small = preview::prepare_page(&mut s, &book, 0, 1000).unwrap().run(&|| false).unwrap();
    let big = preview::prepare_page(&mut s, &book, 0, 1600).unwrap().run(&|| false).unwrap();
    assert!(big.width > small.width && big.height > small.height);
    assert_eq!(small.width * 4, small.rgba.len() / small.height);
    // the long side is what was asked for, within a pixel of rounding
    assert!((small.width.max(small.height) as i64 - 1000).abs() <= 2, "{}x{}", small.width, small.height);
    assert!((small.width as f64 / small.height as f64 - pw / ph).abs() < 0.01);
}

#[test]
fn an_empty_page_is_paper_and_a_missing_page_is_an_error() {
    let mut s = Session::with_demo();
    let mut book = demo_book(&mut s);
    book.settings.background = [10, 20, 30];
    book.pages.push(Page::with(Template::One, &[]));
    let empty = book.pages.len() - 1;
    let px = preview::prepare_page(&mut s, &book, empty, 300).unwrap().run(&|| false).unwrap();
    assert!(px.rgba.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
    let e = preview::prepare_page(&mut s, &book, 99, 300).err().unwrap();
    assert!(e.contains("no page 100"), "{e}");
}

#[test]
fn a_cancelled_preview_page_stops() {
    let mut s = Session::with_demo();
    let book = demo_book(&mut s);
    let e = preview::prepare_page(&mut s, &book, 0, 300).unwrap().run(&|| true).unwrap_err();
    assert_eq!(e, render::CANCELLED);
}

#[test]
fn hostile_sizes_are_kept_in_range() {
    let mut s = Session::with_demo();
    let book = demo_book(&mut s);
    for edge in [0, 1, usize::MAX] {
        let px = preview::prepare_page(&mut s, &book, 0, edge).unwrap().run(&|| false).unwrap();
        assert!(px.width.max(px.height) <= preview::MAX_EDGE.max(8 * 72 * 10), "{edge}: {}x{}", px.width, px.height);
    }
}
