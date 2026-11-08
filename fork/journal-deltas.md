# Develop edits and History saved as what changed

List: a brush stroke, slider edit, undo or redo saves only what it changed to the catalog log, and History is saved in the snapshot as changes from the step before (fork catalog format 2); `perf/77-journal-deltas`.

## What it does

- **The problem ([#77](https://github.com/techneissa/lightcraft/issues/77)).** Each edit wrote the photo's whole develop settings to `catalog.log` twice (the change and its History step), and each undo or redo wrote every History step whole. Brush masks keep every point of every stroke in the settings, so the bytes grew with the square of the strokes.
- **Log records as patches.** `Journal::append` writes `SetDevelop`, `PushHistory`, `SetHistory` and batches holding them as `patchDevelop`, `patchPush`, `patchHistory`, `patchBatch`: a JSON patch (`crates/catalog/src/delta.rs`) against the photo as it was before the record. A pushed step that equals the settings the same record sets is just a reference; an undo's History keeps runs of the old steps by index. Every other op is written as before.
- **The shadow.** The journal keeps the photos as replaying the log would leave them (`journal_delta::Shadow`): the loaded catalog, then each appended record, moved with the catalog's own rules (`Photo::push_history` is shared). Patches are made against it, so replay finds the exact state each patch was made from. A failed append leaves it unchanged, so the retry writes the same records. A snapshot resets it to the catalog it saved.
- **History in the snapshot.** `Photo.history` is stored as the first step whole and each later step as a patch of the one before (`history_serde.rs`); an unchanged step is `"="` and shares its settings when loaded.
- **Never crash.** A patch names the array lengths and keys it was made from, so one applied to other settings fails. Such a patch is skipped with the rest of its photo's part of the record, and that photo's later patches are skipped too, instead of applied to the wrong state; the other photos in the record still apply. A History step that can't be rebuilt ends that photo's History there, with a warning. A torn final patch record is dropped like any torn record. Hostile numbers (`from`, `len`, head and tail counts) are checked; patch nesting is capped.
- **What didn't load is kept.** When a record didn't apply (`LoadReport::failed`) or a History was cut short, the log is kept as `catalog.log.damaged-<seq>` (the snapshot as `catalog.snap.damaged-<seq>` when the cut was in it), as for a damaged log, and a snapshot of what loaded is written. Later patches are then made against that state, so they apply on every later open, and the next save doesn't overwrite what was lost.
- **Torn end after an upgrade.** A torn last record is cut only once the upgrade's snapshot is written; when that write fails, the files stay exactly as they were and the first append cuts it.
- **Exact comparison.** The diff compares numbers by their bits: `==` on JSON values takes `-0.0` for `0.0`, which dropped a change of sign.
- **Formats.** This is fork catalog format 2 (`"format":"lightcraft-catalog-fork"`, `fork_format::VERSION = 2`, see `fork/catalog-format.md`). Fork 1 and the older official and fork formats (whole records, whole steps) still load and are rewritten as fork 2 on open, keeping the old snapshot as `catalog.snap.before-upgrade`. Builds that read fork 1 at most refuse a fork 2 library and leave it untouched.
- **Serialization is part of the format.** Patches and History steps are relative to the writer's JSON of the develop settings. Any change to how develop settings serialize (a renamed field, a changed default that is skipped when written, a reordered array) is a fork format change: raise `fork_format::VERSION`. The same rule is in `journal.rs` → *Format versions* → Rules.

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

- `crates/catalog/src/journal.rs`: the "fork 2" row and the serialization rule; `Journal.shadow`; `append` encodes through the shadow; `open` replays through `journal_delta::Replay`, keeps what didn't load (`keep_what_did_not_load`) and cuts a torn end after the upgrade (`end_log`); `snapshot_with_unlogged` resets the shadow; `Line::Record` holds a `LogOp`; `resync` reads patch records too. `encode_record` / `decode_record` stay for whole records (`decode_record` returns `None` for a patch).
- `crates/catalog/src/fork_format.rs`: `VERSION = 2`.
- `crates/catalog/src/tests_grids.rs`: the log test counts the grid once (in the patch that adds the mask).
- `crates/catalog/src/lib.rs`: three `mod` lines and the test module; `PushHistory` calls `Photo::push_history`.
- `crates/catalog/src/model.rs`: `Photo::push_history`; `history` is `#[serde(with = "crate::history_serde")]`.
- `crates/catalog/src/tests_format_version.rs`: `NEWEST_FORK_FIELDS` 2.
- `crates/engine/src/tests.rs`: `mod journal_growth;`.

## Tests

`cargo test -p lightcraft-catalog --lib -- delta history_serde tests_journal tests_format_version`
