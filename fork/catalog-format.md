# The fork's own catalog format name

List: the fork writes its library as `"format":"lightcraft-catalog-fork"` with its own version counter, so an official build refuses a fork library untouched and one number never means two formats; the first upgrade keeps the old snapshot as `catalog.snap.before-upgrade`. Branch `batch/2026-11-08`.

## What it does

Official builds and the fork used to share `"format":"lightcraft-catalog"` and one counter, and the same number came to mean two formats (the fork's 12 was local camera profiles; the official 12 is shared segmentation grids). From 2026-11-09 the fork writes `lightcraft-catalog-fork`, version 1 (official 12 plus the fork's camera profiles and edited camera values). An official build refuses it before reading anything else (it reads only `format` and `version` first); so do fork builds from before the change ("not a LightCraft catalog"), leaving it untouched.

This build reads:

| Format | Version | Read as |
|---|---|---|
| `lightcraft-catalog-fork` | 1 to `fork_format::VERSION` | the fork format of that number |
| `lightcraft-catalog-fork` | higher | refused, untouched (newer) |
| `lightcraft-catalog` | 0 to 11 | shared history, upgraded |
| `lightcraft-catalog` | 12 | official (shared grids) or fork (camera profiles): read whole, upgraded |
| `lightcraft-catalog` | 13, 14 | fork builds before the rename, upgraded |
| `lightcraft-catalog` | 15 and up | refused, untouched (newer) |

12, 13 and 14 need no guess: a snapshot is read field by field by name, and the two meanings of 12 use different fields, so a file of either (or one holding both) reads exactly. No official build has written 13 or 14.

Every upgrade first keeps the snapshot as it was in `catalog.snap.before-upgrade`; when one exists from an earlier upgrade, it stays. If the copy can't be written, the library is not opened and nothing changes (the error says so). A failed upgrade write leaves the old file as it was (temp file, then rename).

**A new fork format** (an `Op` variant or serialized field): raise `fork_format::VERSION` and add a row to the format table in `journal.rs`. PR #101 (journal deltas) takes fork version 2; Match Look's residual Look (#90, develop settings `look`) takes 3; a mask's Decontaminate amount (#115, `Mask.decontaminate`) takes 4; Select People's saved selection (#123, `MaskShape::People` `face`, `seg`, `detail`, `edge`) takes 5; Face Reshape (#143) takes 6 and Background 7; the Portrait tab's Refine strokes (#99, `fork_retouch.refine`, `fork_retouch.faces[].refine`) take 8.

**An official format past 12** (an official update changes `VERSION` in `journal.rs`, which clashes with the fork's line there): raise `fork_format::OFFICIAL_VERSION`, read the new official fields, and decide `lightcraft-catalog` 13 and 14 by content. Refuse what is still unclear rather than guess: an official 13 read as the fork's would drop the official fields.

## Branch

`batch/2026-11-08`

## Own files

`crates/catalog/src/fork_format.rs`, `crates/catalog/src/tests_fork_format.rs`

## Hooks in official files

catalog `lib.rs` (`pub mod fork_format`), `journal.rs` (`VERSION` re-exported from `fork_format`, the header check in `Journal::open`, `keep_before_upgrade`, the format name in both snapshot writers, the format table), tests `tests_format_version.rs`, `tests_grids.rs`, `tests_journal.rs` (current-format fixtures use the fork name)

## Tests

`fork_format`, `tests_format_version`, `tests_grids`, `tests_journal`
