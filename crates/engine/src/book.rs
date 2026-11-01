//! The Book module: photo-book pages laid out from a set of photos, saved with the library and
//! exported as a PDF (one page per book page) or one JPEG per page.
//!
//! A [`Book`] is plain data: its [`BookSettings`] (page size, orientation, margin, cell padding,
//! background, fit or fill, resolution), an optional cover and its [`Page`]s. A page holds one of
//! a few original [`Template`]s (1 photo, 2, 3, a 2 x 2 grid, full bleed) and one photo slot per
//! cell. Photos that fit on no page (after a template change) wait in [`Book::unplaced`] so a
//! change never loses one.
//!
//! * [`model`]: the data, its checks and the page sizes.
//! * [`geometry`]: where a template's cells lie on a page, in inches.
//! * [`edit`]: auto layout, changing a page's template and moving a photo.
//! * [`render`]: a book as a print job ([`crate::print`]'s composer and PDF writer), PDF and JPEG output.
//! * [`store`]: `books.json` in the library folder.
//!
//! The commands are `book.*` (`crates/engine/src/cmd/book.rs`). Nothing here touches the catalog,
//! so none of it is journaled or undoable; a book is saved to `books.json` after each change.
pub mod edit;
pub mod geometry;
pub mod model;
pub mod render;
pub mod store;

pub use model::{Book, BookSettings, Page, Slot, Template};

use serde::{Deserialize, Serialize};

/// Most books a library keeps.
pub const MAX_BOOKS: usize = 50;
/// Most pages a book has (the Print module's page limit, which the composer applies).
pub const MAX_PAGES: usize = crate::print::layout::MAX_PAGES as usize;
/// Most cells a page has (the 2 x 2 grid).
pub const MAX_CELLS_PER_PAGE: usize = 4;
/// Most photos a book holds: every cell of every page.
pub const MAX_PHOTOS: usize = MAX_PAGES * MAX_CELLS_PER_PAGE;
/// Longest book name, characters.
pub const MAX_NAME: usize = 80;

/// The library's books and which one the Book view shows. Lives in [`crate::Session::books`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Books {
    pub books: Vec<Book>,
    /// Id of the book on show.
    pub current: Option<u32>,
    next_id: u32,
}

impl Books {
    /// The book with `id`, else the current one.
    pub fn find(&self, id: Option<u32>) -> Result<&Book, String> {
        let id = id.or(self.current).ok_or("there is no book yet; create one with book.create")?;
        self.books.iter().find(|b| b.id == id).ok_or_else(|| format!("no book {id} (see book.list)"))
    }

    pub fn find_mut(&mut self, id: Option<u32>) -> Result<&mut Book, String> {
        let id = id.or(self.current).ok_or("there is no book yet; create one with book.create")?;
        self.books.iter_mut().find(|b| b.id == id).ok_or_else(|| format!("no book {id} (see book.list)"))
    }

    /// Adds `book` under a fresh id (the id it carries is replaced) and makes it current.
    pub fn add(&mut self, mut book: Book) -> Result<u32, String> {
        if self.books.len() >= MAX_BOOKS {
            return Err(format!("at most {MAX_BOOKS} books; delete one first"));
        }
        self.next_id = self.next_id.max(self.books.iter().map(|b| b.id).max().unwrap_or(0)).saturating_add(1);
        book.id = self.next_id;
        self.current = Some(book.id);
        self.books.push(book);
        Ok(self.next_id)
    }

    /// Removes a book; the current one moves to the first left, if it was the one removed.
    pub fn remove(&mut self, id: u32) -> Result<(), String> {
        let at = self.books.iter().position(|b| b.id == id).ok_or_else(|| format!("no book {id} (see book.list)"))?;
        self.books.remove(at);
        if self.current == Some(id) {
            self.current = self.books.first().map(|b| b.id);
        }
        Ok(())
    }

    /// Checks everything read from a file: counts, unique ids, every book. A bad book is an error
    /// for the whole file (it is then set aside and the library starts without books).
    pub fn validate(&mut self) -> Result<(), String> {
        if self.books.len() > MAX_BOOKS {
            return Err(format!("at most {MAX_BOOKS} books"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for b in &mut self.books {
            if !seen.insert(b.id) {
                return Err(format!("duplicate book id {}", b.id));
            }
            b.validate().map_err(|e| format!("book {}: {e}", b.id))?;
        }
        self.next_id = self.next_id.max(self.books.iter().map(|b| b.id).max().unwrap_or(0));
        if self.current.is_some_and(|c| !seen.contains(&c)) {
            self.current = self.books.first().map(|b| b.id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
