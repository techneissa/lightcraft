# Book

## What it does

A photo book view in the main window ([#23](https://github.com/techneissa/lightcraft/issues/23), slices 1 and 2). Mac only.

- **Books** are listed in the side bar (a Books block under the sections, + makes an empty one) like albums: click to open, double-click or right-click ▸ Rename… to rename in the row (Return or a click away saves, Esc cancels, an empty or taken name is refused), Duplicate, Delete… (asks). File ▸ Book… opens the view too. Leaving and coming back keeps the page on show.
- **Spread** in the middle (the cover alone, then two pages side by side). Drag a photo from the pool into a frame (the photo that was there waits in the pool), drag inside a frame to move the photo, scroll or pinch to zoom it (1 to 4 times, a run of scroll steps is one undo step), double-click to reset, drag it out of the frame onto another frame to swap or onto the pool to take it off its page.
- **Page strip** at the bottom: thumbnails (only the pages on screen are drawn, from the grid's thumbnails), click to jump, drag to reorder (the cover stays first), + adds a page after the one on show. ← → move between pages, Page Up and Down between spreads.
- **Pool** in the right panel, in place of the tools while the view is on (Photos tab): the book's photos, searchable by file name, the ones on no page first with an outline, placed ones marked with their page number. **Settings** tab: Auto Layout (layout, Cover), page size, orientation, margin, padding, background, Zoom to Fill Frame, resolution.
- The bar above the spread acts on the page on show: Template, Lock Page (Auto Layout leaves a locked page where it is), Duplicate Page, Delete Page (its photos go to the pool); Export PDF… and Export JPEGs….
- **Add to Book**: right-click ▸ Add to Book ▸ a book or New Book… (the photo menu item `addToBook`, which Settings ▸ Photo Menu can move), or drag photos onto a book in the side bar. A book that has some of the photos asks: Add Duplicates, Skip Duplicates or Cancel.
- Every change is one step of the app's undo (Edit ▸ Undo): `Session::push_book_step` puts a `UndoEntry` whose `book` holds what the book was; undo and redo swap it with the book now and save `books.json`.
- A frame's **crop** (zoom, centre; fractions of the photo) is stored per cell and follows the photo when it moves; the preview and the export both take the part of the photo from `geometry::crop_window`, so they show the same at any resolution. The export (`book/render.rs`) draws cropped cells itself and every other cell through the Print composer (`draw_cell`).
- Books are saved with the library in `books.json` (version 2; version 1 files read as they are): the `version` is read first; a newer file, one over 4 MB or one with a book this build can't use is left untouched and books can't be saved; only text that is not JSON is copied to `books.json.corrupt-<time>`. At most 50 books of 200 pages (a PDF of a very large book may need a lower resolution to fit memory; the export says so).
- Agents use `book.list|get|create|open|delete|rename|duplicate|set|layout|setCover|setTemplate|movePhoto|addPhotos|addPage|deletePage|duplicatePage|movePage|lockPage|setCellCrop|exportPdf|exportJpeg` and `view.book` (`dialog.book`). `book.addPhotos` needs `duplicates: add|skip` when the book has some of the photos.

Not yet: a full-screen flip-through, photo and page text, typography, guides, spreads as one PDF page, Blurb upload.

## Branch

`feat/23-book` (slice 1), `feat/23-book-view` (slice 2)

## Own files

`crates/engine/src/book.rs`, `crates/engine/src/book/` (model, geometry, edit, pages, undo, render, store, tests), `crates/engine/src/cmd/book.rs`, `crates/engine/src/tests/book.rs`, `crates/engine/src/tests/book_view.rs`, `crates/ui-egui/src/panels/book.rs`, `crates/ui-egui/src/panels/book/` (spread, strip, pool, settings, menu, thumbs), `crates/ui-egui/src/panels/left/books.rs`, `crates/ui-egui/src/export_task/book_task.rs`, `crates/ui-egui/src/tests/book.rs`

## Hooks in official files

- `crates/engine/src/lib.rs`: `pub mod book;`, the `books` field of `Session`, the `book` field of `UndoEntry` and its two early returns in `undo_step_with` / `redo_step_with` (`book_history_step`), `merge_undo` leaves book steps alone.
- `crates/engine/src/library.rs`: reading `books.json` when a library opens, and `write_library_file` (writes a settings file, refuses one that was left untouched).
- `crates/engine/src/cmd.rs`: `mod book;` and `book::specs()`. `crates/engine/src/print/compose.rs`: `render` is `pub(crate)`.
- `crates/ui-egui/src/state/view.rs` (`ViewMode::Book`), `state.rs` (`UiState::book`), `docking.rs` (the view in the centre panel), `panels.rs` (`pub mod book;`), `keybinds/when/keys.rs` (the `view` value `book`), `shortcuts.rs` (← → Page Up / Down), `app.rs` (`after_command`: a library source or folder leaves the view), `app/frame.rs` (`show_prompt`), `panels/left.rs` (`mod books;` and one call to `books_block`), `panels/right.rs` (the pool in place of the tools), `panels/photo_menu.rs` and `panels/photo_menu/items.rs` (Add to Book), `export_task.rs` (`mod book_task;`, the finished-export message and the row's final count), `menus.rs` (the exports are enabled with a photo), `menus/ui_commands.rs` (`dialog.book`, `view.book`, `app.bookPdf`, `app.bookJpeg`), `menus/run.rs` (one call to `book_task::command`).
- `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs`: `mod book;` (`book_view;`).
- `docs/whats-new.md`, `docs/parity-checklist.md` (LRC-BOOK rows), `docs/gaps.md` (gap 16).

## Tests

`cargo test -p lightcraft-engine --lib -- book::tests tests::book` and `cargo test -p lightcraft-ui-egui --lib -- tests::book`
