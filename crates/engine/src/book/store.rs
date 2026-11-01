//! Books in the library folder: `books.json`, read when the library opens ([`load`], called from
//! `library.rs`) and written after each change ([`Session::save_books`]).
//!
//! A file this build cannot use is never replaced. A file from a newer LightCraft (its `version` is
//! read first, so keys this build does not know are no reason to call it damaged), too large to
//! read, or with a book that fails the checks is left untouched and saving books is blocked; text
//! that is not JSON at all is copied to `books.json.corrupt-<time>` and the library starts empty.
use super::Books;
use crate::{EngineError, Result, Session};
use lightcraft_catalog::Store;
use serde::{Deserialize, Serialize};

/// The file's name in the library folder.
pub const NAME: &str = "books.json";
/// Version of the file this build writes and the newest it reads.
const VERSION: u64 = 1;
/// Largest file read, bytes (50 books of 50 pages come to well under 1 MB).
pub const MAX_FILE: usize = 4 << 20;

/// The file: the books under a version, so a later build can change the shape.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BooksFile {
    pub version: u64,
    pub library: Books,
}

/// What reading `books.json` gave.
#[derive(Default)]
pub struct Loaded {
    pub books: Books,
    /// The file must not be overwritten this session.
    pub block: bool,
    /// What to tell the user.
    pub warnings: Vec<String>,
}

impl Loaded {
    fn blocked(why: String) -> Loaded {
        let note = format!(
            "{NAME} {why}. It was left untouched, and books can't be saved until it is moved away or the LightCraft that wrote it opens this library."
        );
        Loaded { block: true, warnings: vec![note], ..Default::default() }
    }
}

/// Reads the books from a library's files. Never writes to `books.json`.
pub fn load(files: &mut dyn Store) -> Loaded {
    let bytes = match files.read_capped(NAME, MAX_FILE) {
        Ok(Some(b)) => b,
        Ok(None) => return Loaded::default(),
        Err(e) => return Loaded::blocked(format!("couldn't be read ({e})")),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return set_aside(files, &bytes, &e.to_string()),
    };
    // the version first: a newer file may hold anything
    let version = value.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0);
    if version > VERSION {
        return Loaded::blocked(format!("is version {version}, from a newer LightCraft than this one (it reads up to {VERSION})"));
    }
    match serde_json::from_value::<BooksFile>(value).map_err(|e| e.to_string()).and_then(|f| f.library.into_checked()) {
        Ok(books) => Loaded { books, ..Default::default() },
        Err(e) => Loaded::blocked(format!("has books this LightCraft can't use ({e})")),
    }
}

/// Text that is not JSON: kept as `books.json.corrupt-<time>`, and the library starts with no books.
fn set_aside(files: &mut dyn Store, bytes: &[u8], err: &str) -> Loaded {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let keep = format!("{NAME}.corrupt-{secs}");
    match files.write_atomic(&keep, bytes) {
        Ok(()) => Loaded {
            warnings: vec![format!("{NAME} is damaged ({err}). It was kept as {keep}, and the library starts with no books.")],
            ..Default::default()
        },
        Err(w) => Loaded::blocked(format!("is damaged ({err}) and couldn't be copied aside ({w})")),
    }
}

impl Session {
    /// Writes the books to the library (a no-op for an in-memory session). On failure the books
    /// are what they were before `before`, so the list matches what is saved.
    pub fn save_books(&mut self, before: Books) -> Result<()> {
        let file = BooksFile { version: VERSION, library: self.books.clone() };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| EngineError::Other(format!("books: {e}")))?;
        if let Err(e) = self.write_library_file(NAME, &bytes) {
            self.books = before;
            return Err(e);
        }
        Ok(())
    }
}
