//! A book change as a step of the app's undo (Edit ▸ Undo, `edit.undo`). Books live outside the
//! catalog, so the step is an [`crate::UndoEntry`] whose `op` does nothing and whose `book` holds
//! what one book was before the change ([`BookStep`]); undoing swaps it with what the book is now,
//! so redo is the same step the other way, and both save `books.json`.
use super::{Book, Books};
use crate::{Result, Session, UndoEntry};
use lightcraft_catalog::Op;

/// How many undo steps the session keeps (as for catalog changes).
const MAX_STEPS: usize = 1000;

/// One book as it was: put back by [`Books::restore`].
#[derive(Clone, Debug, PartialEq)]
pub struct BookStep {
    pub id: u32,
    /// The book, or `None` when there was none (a new book is taken away again).
    pub book: Option<Book>,
    /// Where in the list it goes when it is not there (a deleted book comes back in its place).
    pub at: usize,
    /// The book on show.
    pub current: Option<u32>,
}

impl Books {
    /// What book `id` is now, as a step that would put it back.
    pub fn snapshot(&self, id: u32) -> BookStep {
        let at = self.books.iter().position(|b| b.id == id);
        BookStep { id, book: at.and_then(|i| self.books.get(i)).cloned(), at: at.unwrap_or(self.books.len()), current: self.current }
    }

    /// Puts a step's book back (replacing it, adding it or removing it); returns the step that
    /// brings the books back to what they are now.
    pub fn restore(&mut self, step: BookStep) -> BookStep {
        let inverse = self.snapshot(step.id);
        let at = self.books.iter().position(|b| b.id == step.id);
        match (step.book, at) {
            (Some(book), Some(i)) => {
                if let Some(slot) = self.books.get_mut(i) {
                    *slot = book;
                }
            }
            (Some(book), None) => self.books.insert(step.at.min(self.books.len()), book),
            (None, Some(i)) => {
                self.books.remove(i);
            }
            (None, None) => {}
        }
        self.current = step.current.filter(|c| self.books.iter().any(|b| b.id == *c)).or_else(|| self.books.first().map(|b| b.id));
        inverse
    }
}

fn entry(label: &str, step: BookStep) -> UndoEntry {
    UndoEntry {
        label: label.to_string(),
        op: Op::Batch { ops: Vec::new() },
        folder: None,
        retry_inverse: Vec::new(),
        retry_sidecars: Vec::new(),
        answered_offer: None,
        book: Some(Box::new(step)),
    }
}

impl Session {
    /// Records a change to a book as one undo step: `step` is what the book was before it
    /// ([`Books::snapshot`]). Like any new change, it clears what could be redone.
    pub fn push_book_step(&mut self, label: &str, step: BookStep) {
        self.undo.push(entry(label, step));
        self.commits = self.commits.wrapping_add(1);
        if self.undo.len() > MAX_STEPS {
            let mut dropped = self.undo.remove(0);
            self.pending_sidecar_repairs.append(&mut dropped.retry_sidecars);
        }
        for mut dropped in self.redo.drain(..) {
            self.pending_sidecar_repairs.append(&mut dropped.retry_sidecars);
        }
    }

    /// Undoes (`undo`) or redoes a book step already taken off its stack: the books go back and are
    /// saved, and the opposite step goes on the other stack. On failure the step goes back.
    pub(crate) fn book_history_step(&mut self, e: UndoEntry, undo: bool) -> Result<String> {
        let Some(step) = e.book.clone() else { return Ok(e.label) };
        let before = self.books.clone();
        let inverse = self.books.restore(*step);
        if let Err(err) = self.save_books(before) {
            if undo {
                self.undo.push(e)
            } else {
                self.redo.push(e)
            }
            return Err(err);
        }
        let label = e.label.clone();
        let other = entry(&label, inverse);
        if undo {
            self.redo.push(other)
        } else {
            self.undo.push(other)
        }
        Ok(label)
    }
}
