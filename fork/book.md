# Book

## What it does

File ▸ Book… lays the selected photos (or the photos in view) out as a photo book and exports it. Mac only; first slice of [#23](https://github.com/techneissa/lightcraft/issues/23).

- A book has a cover (full bleed) and pages of five original templates: 1, 2, 3, a 2 × 2 grid and full bleed. Auto Layout cycles 1, 2, 3, 4 photos per page, or one template for every page; the last page takes the template that fits the photos left.
- Settings: page size (11 names, 5×7 … 12×12, A5, A4, Letter), orientation, margin, cell padding, background colour, Zoom to Fill Frame, resolution.
- Pages view (every page) and Spread view (cover alone, then two pages side by side). A page's template changes from the Page Template dropdown (Auto Layout has a layout dropdown, a photos dropdown and one button; Cover is a checkbox that adds or removes only the cover page); drag a photo onto another cell to swap, or onto the "Not on a page" strip to take it off (photos that no longer fit a smaller template wait there, none is lost).
- Export PDF… (the dialog's OK button) and Export JPEGs… run on a worker thread with a cancellable row in the activity stack. The PDF has one page per book page at the page size. They reuse the Print module's composer and PDF writer: the book becomes a Custom Package layout (`book/render.rs`).
- Books are saved with the library in `books.json` (next to `prefs.json`), after every change. The file's `version` is read first: a newer file, one over 4 MB or one with a book this build can't use is left untouched and books can't be saved (the warning says so); only text that is not JSON is copied to `books.json.corrupt-<time>`. Agents use `book.list|get|create|open|delete|set|layout|setCover|setTemplate|movePhoto|exportPdf|exportJpeg`.

Not yet: photo and page text, typography, guides, adding and removing pages, spreads as one PDF page, Blurb upload.

## Branch

`feat/23-book`

## Own files

`crates/engine/src/book.rs`, `crates/engine/src/book/` (model, geometry, edit, render, store, tests), `crates/engine/src/cmd/book.rs`, `crates/engine/src/tests/book.rs`, `crates/ui-egui/src/dialogs/book_view.rs`, `crates/ui-egui/src/export_task/book_task.rs`, `crates/ui-egui/src/tests/book.rs`

## Hooks in official files

- `crates/engine/src/lib.rs`: `pub mod book;`, the `books` field of `Session`.
- `crates/engine/src/library.rs`: reading `books.json` when a library opens, and `write_library_file` (writes a settings file, refuses one that was left untouched).
- `crates/engine/src/cmd.rs`: `mod book;` and `book::specs()`.
- `crates/ui-egui/src/state/dialog.rs` (`Dialog::Book`), `state.rs` (`UiState::book`), `dialogs.rs` and `dialogs/confirm.rs` (title, width, OK button, body, stay open on error), `lib.rs` (`pub use dialogs::book_view`), `export_task.rs` (`mod book_task;`, the finished-export message and the row's final count), `menus.rs` (enabled with a photo), `menus/ui_commands.rs` (`dialog.book`, `app.bookPdf`, `app.bookJpeg`), `menus/run.rs` (one call to `book_task::command`).
- `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs`: `mod book;`.
- `docs/whats-new.md`, `docs/parity-checklist.md` (LRC-BOOK rows), `docs/gaps.md` (gap 16).

## Tests

`cargo test -p lightcraft-engine --lib -- book::tests tests::book` and `cargo test -p lightcraft-ui-egui --lib -- tests::book`
