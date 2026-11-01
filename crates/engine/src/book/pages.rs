//! Changing a book's pages and its photo list: adding photos, adding, deleting, duplicating, moving
//! and locking pages, and a cell's crop. Page and cell numbers are zero-based.
use super::model::{Crop, Page, Template};
use super::{Book, MAX_PAGES, MAX_PHOTOS};

/// What to do with photos the book already holds when they are added again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Duplicates {
    /// Add them again: the book then shows the photo twice.
    Add,
    /// Leave them out.
    Skip,
}

impl Duplicates {
    pub fn parse(s: &str) -> Result<Duplicates, String> {
        match s.trim().to_lowercase().as_str() {
            "add" => Ok(Duplicates::Add),
            "skip" => Ok(Duplicates::Skip),
            _ => Err(format!("`duplicates` must be add or skip, not `{s}`")),
        }
    }
}

/// What [`Book::add_photos`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Added {
    pub added: usize,
    /// Photos left out because the book already held them.
    pub skipped: usize,
}

impl Book {
    /// The photos of `photos` the book already holds, each once, in the order given.
    pub fn duplicates_of(&self, photos: &[u64]) -> Vec<u64> {
        let mine: std::collections::BTreeSet<u64> = self.photos().into_iter().collect();
        let mut seen = std::collections::BTreeSet::new();
        photos.iter().copied().filter(|p| mine.contains(p) && seen.insert(*p)).collect()
    }

    /// Adds photos to the end of the tray ([`Book::unplaced`]), from where they go onto pages. A
    /// photo repeated in `photos` is added once. Photos the book holds already are added again or
    /// left out, as `duplicates` says.
    pub fn add_photos(&mut self, photos: &[u64], duplicates: Duplicates) -> Result<Added, String> {
        let held: std::collections::BTreeSet<u64> = self.photos().into_iter().collect();
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Added::default();
        let mut fresh = Vec::new();
        for &p in photos {
            if !seen.insert(p) {
                continue;
            }
            if held.contains(&p) && duplicates == Duplicates::Skip {
                out.skipped += 1;
            } else {
                fresh.push(p);
            }
        }
        if self.photos().len().saturating_add(fresh.len()) > MAX_PHOTOS {
            return Err(format!("a book holds at most {MAX_PHOTOS} photos"));
        }
        out.added = fresh.len();
        self.unplaced.extend(fresh);
        Ok(out)
    }

    fn page_index(&self, index: usize) -> Result<usize, String> {
        if index < self.pages.len() { Ok(index) } else { Err(format!("no page {} (the book has {})", index.saturating_add(1), self.pages.len())) }
    }

    /// The first page that is not the cover.
    fn first_inner(&self) -> usize {
        usize::from(self.cover && !self.pages.is_empty())
    }

    /// Adds an empty page of `template` at `at` (zero-based: it becomes that page; `None`: the end).
    /// Nothing goes before the cover.
    pub fn add_page(&mut self, at: Option<usize>, template: Template) -> Result<usize, String> {
        if self.pages.len() >= MAX_PAGES {
            return Err(format!("a book has at most {MAX_PAGES} pages"));
        }
        let at = at.unwrap_or(self.pages.len());
        if at > self.pages.len() {
            return Err(format!("no place {} for a page (the book has {} pages)", at.saturating_add(1), self.pages.len()));
        }
        if at < self.first_inner() {
            return Err("the cover stays the first page".into());
        }
        self.pages.insert(at, Page::with(template, &[]));
        Ok(at)
    }

    /// Removes a page; its photos go to the end of the tray. Removing the cover leaves a book without one.
    pub fn delete_page(&mut self, index: usize) -> Result<(), String> {
        let index = self.page_index(index)?;
        let page = self.pages.remove(index);
        self.unplaced.extend(page.slots.into_iter().flatten());
        if index == 0 && self.cover {
            self.cover = false;
        }
        Ok(())
    }

    /// Copies a page right after itself (its photos then show twice, its lock and crops come along).
    /// The copy of the cover is an inner page.
    pub fn duplicate_page(&mut self, index: usize) -> Result<usize, String> {
        let index = self.page_index(index)?;
        if self.pages.len() >= MAX_PAGES {
            return Err(format!("a book has at most {MAX_PAGES} pages"));
        }
        let copy = self.pages.get(index).cloned().ok_or("no such page")?;
        if self.photos().len().saturating_add(copy.slots.iter().flatten().count()) > MAX_PHOTOS {
            return Err(format!("a book holds at most {MAX_PHOTOS} photos"));
        }
        self.pages.insert(index + 1, copy);
        Ok(index + 1)
    }

    /// Moves page `from` so it becomes page `to`. The cover can't move and nothing goes before it.
    pub fn move_page(&mut self, from: usize, to: usize) -> Result<(), String> {
        let from = self.page_index(from)?;
        let to = self.page_index(to)?;
        if from < self.first_inner() || to < self.first_inner() {
            return Err("the cover stays the first page".into());
        }
        let page = self.pages.remove(from);
        self.pages.insert(to, page);
        Ok(())
    }

    /// Locks or unlocks a page: Auto Layout leaves a locked page as it is.
    pub fn lock_page(&mut self, index: usize, locked: bool) -> Result<(), String> {
        let index = self.page_index(index)?;
        self.pages.get_mut(index).ok_or("no such page")?.locked = locked;
        Ok(())
    }

    /// Sets how the photo in a cell shows (`None`: as the book's settings say).
    pub fn set_cell_crop(&mut self, page: usize, cell: usize, crop: Option<Crop>) -> Result<(), String> {
        let at = self.page_index(page)?;
        if let Some(c) = &crop {
            c.validate()?;
        }
        let p = self.pages.get_mut(at).ok_or("no such page")?;
        if p.slots.get(cell).copied().ok_or_else(|| format!("page {} has no cell {}", page + 1, cell.saturating_add(1)))?.is_none() {
            return Err(format!("cell {} of page {} holds no photo", cell + 1, page + 1));
        }
        p.crops.resize(p.slots.len(), None);
        if let Some(c) = p.crops.get_mut(cell) {
            *c = crop;
        }
        Ok(())
    }
}
