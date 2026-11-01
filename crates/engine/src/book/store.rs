//! Books in the library folder: `books.json`, read when the library opens and written after each
//! change (hooks in `library.rs`: [`Session::read_books`] and the library's settings writer).
use super::Books;
use crate::{EngineError, Result, Session};
use serde::{Deserialize, Serialize};

/// Version of the file this build writes and the newest it reads.
const VERSION: u64 = 1;

/// The file: the books under a version, so a later build can change the shape.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BooksFile {
    pub version: u64,
    pub library: Books,
}

impl BooksFile {
    /// The books of a file as read, checked. A newer version or a bad book is an error (the
    /// caller keeps the file and starts without books).
    pub fn into_books(mut self) -> std::result::Result<Books, String> {
        if self.version > VERSION {
            return Err(format!("books.json is version {}, newer than the {VERSION} this LightCraft reads", self.version));
        }
        self.library.validate()?;
        Ok(self.library)
    }
}

impl Session {
    /// Writes the books to the library (a no-op for an in-memory session). On failure the books
    /// are what they were before `before`, so the list matches what is saved.
    pub fn save_books(&mut self, before: Books) -> Result<()> {
        let file = BooksFile { version: VERSION, library: self.books.clone() };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| EngineError::Other(format!("books: {e}")))?;
        if let Err(e) = self.write_library_file("books.json", &bytes) {
            self.books = before;
            return Err(e);
        }
        Ok(())
    }
}
