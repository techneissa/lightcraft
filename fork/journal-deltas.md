# Develop edits and History saved as what changed

List: a brush stroke, slider edit, undo or redo saves only what it changed to the catalog log, and History is saved in the snapshot as changes from the step before (catalog format 14); `perf/77-journal-deltas`.

## What it does

- **The problem ([#77](https://github.com/techneissa/lightcraft/issues/77)).** Each edit wrote the photo's whole develop settings to `catalog.log` twice (the change and its History step), and each undo or redo wrote every History step whole. Brush masks keep every point of every stroke in the settings, so the bytes grew with the square of the strokes.
- **Log records as patches.** `Journal::append` writes `SetDevelop`, `PushHistory`, `SetHistory` and batches holding them as `patchDevelop`, `patchPush`, `patchHistory`, `patchBatch`: a JSON patch (`crates/catalog/src/delta.rs`) against the photo as it was before the record. A pushed step that equals the settings the same record sets is just a reference; an undo's History keeps runs of the old steps by index. Every other op is written as before.
- **The shadow.** The journal keeps the photos as replaying the log would leave them (`journal_delta::Shadow`): the loaded catalog, then each appended record, moved with the catalog's own rules (`Photo::push_history` is shared). Patches are made against it, so replay finds the exact state each patch was made from. A failed append leaves it unchanged, so the retry writes the same records. A snapshot resets it to the catalog it saved.
- **History in the snapshot.** `Photo.history` is stored as the first step whole and each later step as a patch of the one before (`history_serde.rs`); an unchanged step is `"="` and shares its settings when loaded.
- **Never crash.** A patch names the array lengths and keys it was made from, so one applied to other settings fails. A record that fails is reported (`LoadReport::failed`) and the photo's later patches are refused instead of applied to the wrong state; other photos replay. A History step that can't be rebuilt ends that photo's History there, with a warning. A torn final patch record is dropped like any torn record. Hostile numbers (`from`, `len`, head and tail counts) are checked; patch nesting is capped.
- **Older catalogs.** Format 13 and older logs and snapshots (whole records, whole steps) still load and are rewritten in format 14 on open. Older builds refuse a format 14 library ("written by a newer version") and leave it untouched.

Measured with `crates/engine/src/tests/journal_growth.rs` (300 strokes of 60 points on one photo, in memory, release build):

| | Before | After |
|---|---|---|
| Log written, 30 strokes | 2.7 MB | 92 KB |
| Log written, 300 strokes | 240 MB | 0.9 MB |
| Snapshots written during the 300 strokes | 758 MB | none |
| Save time per stroke at 300 strokes | 23 ms | 2.8 ms |
| One undo / redo at 300 strokes | 104 MB, 390 ms | 0.6 KB / 5.9 KB, 6–7 ms |
| `catalog.snap` after closing | 107 MB | 1.7 MB |
| Reopening the library | 1.5 s | 0.15 s |

History in memory is unchanged: each of the 200 steps still holds its own copy of the settings (106 MB as JSON at 300 strokes). Sharing strokes between steps needs shared types in the develop crate.

## Branch

`perf/77-journal-deltas`

## Own files

`crates/catalog/src/delta.rs`, `crates/catalog/src/journal_delta.rs`, `crates/catalog/src/history_serde.rs`, `crates/catalog/src/tests_journal_delta.rs`, `crates/engine/src/tests/journal_growth.rs` (the measurement, `#[ignore]`d: `cargo test --release -p lightcraft-engine --lib journal_growth -- --ignored --nocapture`)

## Hooks in official files

- `crates/catalog/src/journal.rs`: `VERSION` 14 and its table row; `Journal.shadow`; `append` encodes through the shadow; `open` replays through `journal_delta::Replay`; `snapshot_with_unlogged` resets the shadow; `Line::Record` holds a `LogOp`; `resync` reads patch records too. `encode_record` / `decode_record` stay for whole records (`decode_record` returns `None` for a patch).
- `crates/catalog/src/lib.rs`: three `mod` lines and the test module; `PushHistory` calls `Photo::push_history`.
- `crates/catalog/src/model.rs`: `Photo::push_history`; `history` is `#[serde(with = "crate::history_serde")]`.
- `crates/catalog/src/tests_format_version.rs`: `NEWEST_FIELDS` 14.
- `crates/engine/src/tests.rs`: `mod journal_growth;`.

## Tests

`cargo test -p lightcraft-catalog --lib -- delta history_serde tests_journal tests_format_version`
