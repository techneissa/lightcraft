# Quick filters in the bottom bar

## What it does

Quick filters in the bottom bar, beside the thumbnail slider: a Stars dropdown (Unrated, 1★ … 5★, several at once, the menu stays open) and Picked / Rejected / Unflagged buttons; OR inside a group, AND between groups; ✕ or right-click Clear Filters; one button in a narrower bar and in Detail / Compare; the filter bar shows the same state (its flag buttons are multi-select now). `library.filter` takes `ratings: [0..5]` and `flags: [pick|reject|none]` next to `rating` / `ratingOp` / `flag` (all optional, skipped when empty, so old smart albums and saved filters load unchanged; a patch that sets the range clears the set and the other way round).

## Branch

`feat/bottombar-quick-filters`

## Own files

`crates/catalog/src/quick_filter.rs` and `tests_quick_filter.rs`, `crates/engine/src/tests/quick_filters.rs`, `crates/ui-egui/src/panels/quick_filters.rs`, `crates/ui-egui/src/tests/quick_filters.rs`

## Hooks in official files

catalog `query.rs` (`ratings`; `flags` is shared with official's #2588, which added the same field and `flags_shown`: the fork keeps its own `flags_shown` and `toggle_flag_patch` in `quick_filter.rs`, official's `null_as_empty` on the field and official's filter chip for flags) and `lib.rs` (modules); engine `cmd/library/specs.rs` (`settle_sets` call, doc line), `view.rs` (two chips), `tests.rs`; UI `panels.rs`, `panels/bottombar.rs` (two calls), `panels/filterbar.rs` (flags multi-select, ratings text), `tests.rs`; `docs/parity-checklist.md`, `docs/whats-new.md`

## Tests

`quick_filter`, `quick_filters`

The official `filter_bar_filters_and_saves_a_smart_album` (`headless/tests.rs`) holds too: the flag buttons always write a `flags` set, one flag included (`toggle_flag_patch`).
