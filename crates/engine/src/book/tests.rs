//! The book model: layout, geometry, template changes, moving photos, hostile input, the file.
use super::edit::{Preset, auto_layout};
use super::geometry::{cells, check_cells, fitted};
use super::model::{Book, BookSettings, Page, Slot, Template};
use super::store::BooksFile;
use super::{Books, MAX_PAGES, MAX_PHOTOS};
use crate::print::layout::{MIN_CELL_IN, Orientation, Rect};

fn photos(n: u64) -> Vec<u64> {
    (1..=n).collect()
}

fn book(n: u64, cover: bool, preset: Preset) -> Book {
    let mut b = Book { name: "Trip".into(), ..Default::default() };
    b.relayout(&photos(n), cover, preset).unwrap();
    b.validate().unwrap();
    b
}

fn shape(b: &Book) -> Vec<(Template, Vec<Option<u64>>)> {
    b.pages.iter().map(|p| (p.template, p.slots.clone())).collect()
}

#[test]
fn auto_layout_fills_pages_in_order_with_a_cover() {
    let b = book(11, true, Preset::Mixed);
    // cover, then one, two, three, four photos per page; every photo exactly once, in order
    assert_eq!(b.pages.iter().map(|p| p.template).collect::<Vec<_>>(), [Template::FullBleed, Template::One, Template::Two, Template::Three, Template::Grid4]);
    assert_eq!(b.pages[0].slots, [Some(1)]);
    assert_eq!(b.photos(), photos(11));
    assert!(b.cover && b.unplaced.is_empty());
}

#[test]
fn the_last_page_takes_the_template_that_fits_what_is_left() {
    // cover + 4-up pages: 1 + 4 + 4 + 2 left → the last page is a two, never a grid with holes
    let b = book(11, true, Preset::Grid4);
    assert_eq!(b.pages.iter().map(|p| p.template).collect::<Vec<_>>(), [Template::FullBleed, Template::Grid4, Template::Grid4, Template::Two]);
    assert!(b.pages.iter().all(|p| p.slots.iter().all(Option::is_some)));
    assert_eq!(book(1, false, Preset::Three).pages, [Page::with(Template::One, &[1])]);
    assert_eq!(book(1, true, Preset::Mixed).pages, [Page::with(Template::FullBleed, &[1])]);
    // without a cover the first photo is on an inner page
    assert_eq!(book(3, false, Preset::One).pages.len(), 3);
    assert_eq!(book(3, false, Preset::FullBleed).pages.iter().map(|p| p.template).collect::<Vec<_>>(), [Template::FullBleed; 3]);
}

#[test]
fn hostile_photo_counts_are_errors() {
    assert!(auto_layout(&[], true, Preset::Mixed).unwrap_err().contains("at least one photo"));
    let many: Vec<u64> = (0..=MAX_PHOTOS as u64).collect();
    assert!(auto_layout(&many, false, Preset::Grid4).unwrap_err().contains("at most"));
    // one photo per page runs out of pages before it runs out of photos
    let e = auto_layout(&photos(MAX_PAGES as u64 + 1), false, Preset::One).unwrap_err();
    assert!(e.contains("more than 50 pages"), "{e}");
    // exactly the limit works
    assert_eq!(auto_layout(&photos(MAX_PAGES as u64), false, Preset::One).unwrap().len(), MAX_PAGES);
    assert_eq!(auto_layout(&photos(MAX_PHOTOS as u64), false, Preset::Grid4).unwrap().len(), MAX_PAGES);
    assert!(Preset::parse("nope").is_err());
}

#[test]
fn cells_follow_the_template_and_the_page() {
    let area = |r: &Rect| (r.x, r.y, r.w, r.h);
    // landscape 10 x 8, margin 1, no padding: area 8 x 6
    let c = cells(Template::Two, (10.0, 8.0), 1.0, 0.0);
    assert_eq!((area(&c[0]), area(&c[1])), ((1.0, 1.0, 4.0, 6.0), (5.0, 1.0, 4.0, 6.0)), "side by side");
    // portrait 8 x 10: stacked
    let c = cells(Template::Two, (8.0, 10.0), 1.0, 0.0);
    assert_eq!((area(&c[0]), area(&c[1])), ((1.0, 1.0, 6.0, 4.0), (1.0, 5.0, 6.0, 4.0)));
    // three: a big cell and two small ones sharing the other half
    let c = cells(Template::Three, (8.0, 10.0), 1.0, 0.0);
    assert_eq!((area(&c[0]), area(&c[1]), area(&c[2])), ((1.0, 1.0, 6.0, 4.0), (1.0, 5.0, 3.0, 4.0), (4.0, 5.0, 3.0, 4.0)));
    let c = cells(Template::Three, (10.0, 8.0), 1.0, 0.0);
    assert_eq!((area(&c[0]), area(&c[1]), area(&c[2])), ((1.0, 1.0, 4.0, 6.0), (5.0, 1.0, 4.0, 3.0), (5.0, 4.0, 4.0, 3.0)));
    // grid4 in reading order
    let c = cells(Template::Grid4, (10.0, 8.0), 1.0, 0.0);
    assert_eq!(c.iter().map(|r| (r.x, r.y)).collect::<Vec<_>>(), [(1.0, 1.0), (5.0, 1.0), (1.0, 4.0), (5.0, 4.0)]);
    // padding insets every cell on every side; full bleed ignores margin and padding
    let c = cells(Template::One, (10.0, 8.0), 1.0, 0.25);
    assert_eq!(area(&c[0]), (1.25, 1.25, 7.5, 5.5));
    assert_eq!(area(&cells(Template::FullBleed, (10.0, 8.0), 1.0, 0.25)[0]), (0.0, 0.0, 10.0, 8.0));
    // every template's cells stay inside the page
    for t in Template::ALL {
        for r in cells(t, (10.0, 8.0), 0.5, 0.1) {
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.right() <= 10.0 + 1e-9 && r.bottom() <= 8.0 + 1e-9, "{t:?} {r:?}");
        }
    }
}

#[test]
fn settings_are_checked() {
    let ok = BookSettings::default();
    assert!(ok.validate().is_ok());
    assert_eq!(ok.page_size().unwrap(), (10.0, 8.0), "landscape 8x10 is 10 wide");
    assert_eq!(BookSettings { orientation: Orientation::Portrait, ..ok.clone() }.page_size().unwrap(), (8.0, 10.0));
    assert_eq!(BookSettings { size: "10x10".into(), ..ok.clone() }.page_size().unwrap(), (10.0, 10.0), "square ignores orientation");
    assert!(BookSettings { size: "A4".into(), ..ok.clone() }.validate().is_ok(), "names ignore case");
    for bad in [
        BookSettings { size: "huge".into(), ..ok.clone() },
        BookSettings { margin: -1.0, ..ok.clone() },
        BookSettings { margin: f64::NAN, ..ok.clone() },
        BookSettings { padding: f64::INFINITY, ..ok.clone() },
        BookSettings { padding: 5.0, ..ok.clone() },
        BookSettings { ppi: 10.0, ..ok.clone() },
        BookSettings { ppi: f32::NAN, ..ok.clone() },
        // margins that leave no room for a cell
        BookSettings { size: "5x7".into(), margin: 2.0, padding: 1.0, ..ok.clone() },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
    assert!(check_cells(&ok).is_ok());
    assert!(serde_json::from_str::<BookSettings>(r#"{"colour": 1}"#).is_err());
}

#[test]
fn a_photo_fits_inside_its_slot_with_its_own_aspect() {
    let slot = Rect::new(1.0, 1.0, 4.0, 2.0);
    // wide photo in a 2:1 slot fills it; a 4:3 photo is pillarboxed; a portrait one is narrower still
    assert_eq!(fitted(slot, 2.0), slot);
    let f = fitted(slot, 4.0 / 3.0);
    assert!((f.h - 2.0).abs() < 1e-9 && (f.w - 8.0 / 3.0).abs() < 1e-9 && (f.x - (1.0 + (4.0 - f.w) / 2.0)).abs() < 1e-9);
    let f = fitted(slot, 0.5);
    assert!((f.w - 1.0).abs() < 1e-9 && (f.h - 2.0).abs() < 1e-9);
    // a very wide panorama keeps the composer's smallest cell instead of vanishing
    assert!(fitted(slot, 1000.0).h >= MIN_CELL_IN);
    // hostile aspects leave the slot alone
    for a in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(fitted(slot, a), slot);
    }
}

#[test]
fn changing_a_template_never_loses_a_photo() {
    let mut b = book(11, true, Preset::Mixed);
    let before = b.photos();
    let grid = b.pages.iter().position(|p| p.template == Template::Grid4).unwrap();
    let held = b.pages[grid].slots.clone();
    b.set_template(grid, Template::One).unwrap();
    assert_eq!(b.pages[grid].slots, [held[0]]);
    assert_eq!(b.unplaced, held[1..].iter().flatten().copied().collect::<Vec<_>>(), "the three that no longer fit wait in the tray");
    // a bigger template takes them back, in order
    b.set_template(grid, Template::Three).unwrap();
    assert_eq!(b.pages[grid].slots, [held[0], held[1], held[2]]);
    assert_eq!(b.unplaced, [held[3].unwrap()]);
    b.set_template(grid, Template::Grid4).unwrap();
    assert_eq!(b.pages[grid].slots, held);
    assert!(b.unplaced.is_empty());
    let mut after = b.photos();
    let mut want = before;
    after.sort_unstable();
    want.sort_unstable();
    assert_eq!(after, want);
    b.validate().unwrap();
    // a template with more cells than photos left leaves cells empty
    let mut small = book(2, false, Preset::One);
    small.set_template(0, Template::Grid4).unwrap();
    assert_eq!(small.pages[0].slots, [Some(1), None, None, None]);
    assert!(small.set_template(7, Template::One).is_err());
}

#[test]
fn moving_a_photo_swaps_or_unplaces_it() {
    let cell = |page, cell| Slot::Cell { page, cell };
    let tray = |unplaced| Slot::Unplaced { unplaced };
    let mut b = book(6, false, Preset::Two); // pages: [1,2] [3,4] [5,6]
    b.move_photo(cell(0, 0), cell(2, 1)).unwrap();
    assert_eq!(shape(&b)[0].1, [Some(6), Some(2)]);
    assert_eq!(shape(&b)[2].1, [Some(5), Some(1)], "swapped");
    // off the page and back on, into an empty cell
    b.move_photo(cell(1, 0), tray(0)).unwrap();
    assert_eq!(b.unplaced, [3]);
    assert_eq!(shape(&b)[1].1, [None, Some(4)]);
    b.move_photo(tray(0), cell(1, 0)).unwrap();
    assert!(b.unplaced.is_empty());
    // a tray photo dropped on a full cell swaps with it
    b.move_photo(cell(1, 1), tray(0)).unwrap();
    b.move_photo(tray(0), cell(0, 0)).unwrap();
    assert_eq!((b.pages[0].slots[0], b.unplaced.clone()), (Some(4), vec![6]));
    // the same place, and the tray to the tray: nothing changes
    let same = b.clone();
    b.move_photo(cell(0, 0), cell(0, 0)).unwrap();
    b.move_photo(tray(0), tray(0)).unwrap();
    assert_eq!(b, same);
    // nothing to move, or nowhere to go
    b.move_photo(cell(1, 1), cell(0, 0)).unwrap_err();
    for (from, to) in [(cell(9, 0), cell(0, 0)), (cell(0, 0), cell(0, 9)), (tray(5), cell(0, 0)), (cell(0, 0), cell(9, 9))] {
        assert!(b.move_photo(from, to).is_err(), "{from:?} -> {to:?}");
    }
    assert_eq!(b.photos().len(), 6, "an error moved nothing");
    let _ = usize::MAX; // (indices come from commands; a huge one is just another error)
    assert!(b.move_photo(cell(usize::MAX, usize::MAX), cell(0, 0)).is_err());
}

#[test]
fn spreads_open_from_the_cover() {
    let b = book(12, true, Preset::One);
    assert_eq!(b.pages.len(), 12);
    let s = b.spreads();
    assert_eq!(s[0], [0]);
    assert_eq!(s[1], [1, 2]);
    assert_eq!(s.last().unwrap(), &[11], "an even count leaves the last page alone");
    assert_eq!(book(6, true, Preset::One).spreads(), vec![vec![0], vec![1, 2], vec![3, 4], vec![5]]);
    assert!(Book::default().spreads().is_empty());
}

#[test]
fn books_are_checked_when_they_come_from_outside() {
    let mut b = book(4, true, Preset::Two);
    b.pages[1].slots.push(Some(9)); // two cells, three slots
    assert!(b.validate().unwrap_err().contains("slots"));
    let mut b = book(4, true, Preset::Two);
    b.name = "  ".into();
    assert!(b.validate().is_err());
    b.name = "a\nb".into();
    assert!(b.validate().is_err());
    let mut b = Book { name: "x".into(), cover: true, ..Default::default() };
    assert!(b.validate().is_err(), "a cover needs a page");
    let mut b = book(4, false, Preset::One);
    b.pages.resize(MAX_PAGES + 1, Page::with(Template::One, &[1]));
    assert!(b.validate().is_err());
}

#[test]
fn the_books_file_round_trips_and_refuses_bad_data() {
    let mut books = Books::default();
    let a = books.add(book(7, true, Preset::Mixed)).unwrap();
    let c = books.add(Book { name: "Second".into(), ..book(3, false, Preset::One) }).unwrap();
    assert_eq!((a, c, books.current), (1, 2, Some(2)));
    books.books[0].unplaced.push(99);
    let json = serde_json::to_vec_pretty(&BooksFile { version: 1, library: books.clone() }).unwrap();
    let back = serde_json::from_slice::<BooksFile>(&json).unwrap().into_books().unwrap();
    assert_eq!(back, books);
    // removing the current book moves "current" on; ids are not reused while the list lives
    books.remove(2).unwrap();
    assert_eq!(books.current, Some(1));
    assert_eq!(books.add(book(1, false, Preset::One)).unwrap(), 3);
    assert!(books.remove(42).is_err() && books.find(Some(42)).is_err());
    // a newer file, a duplicate id, a broken book and an unknown key are all refused
    let newer = serde_json::from_str::<BooksFile>(r#"{"version": 9}"#).unwrap();
    assert!(newer.into_books().unwrap_err().contains("newer"));
    let mut dup = back.clone();
    dup.books[1].id = dup.books[0].id;
    assert!(BooksFile { version: 1, library: dup }.into_books().unwrap_err().contains("duplicate"));
    let mut broken = back;
    broken.books[0].settings.ppi = 1.0;
    assert!(BooksFile { version: 1, library: broken }.into_books().is_err());
    assert!(serde_json::from_str::<BooksFile>(r#"{"library": {"stuff": 1}}"#).is_err());
    assert!(serde_json::from_slice::<BooksFile>(b"{not json").is_err());
}

#[test]
fn too_many_books_are_refused() {
    let mut books = Books::default();
    for _ in 0..super::MAX_BOOKS {
        books.add(book(1, false, Preset::One)).unwrap();
    }
    assert!(books.add(book(1, false, Preset::One)).is_err());
}
