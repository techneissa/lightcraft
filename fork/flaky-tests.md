# Flaky Tests

List: Three tests that failed on a busy machine now wait on a condition with a hang guard, or no longer depend on how many thumbnails the grid cached first (issues #76, #78, #20). Branch `fix/76-flaky-tests`.

## What it does

No feature: three tests stop failing when the machine is busy, and one of them no longer stops `cargo xtask ci --quick`.

- **Find Similar stop test** (`tests::find_similar::stopping_leaves_the_view_alone_and_the_next_search_reads_only_the_rest`, #76). The test counted every file the second search read and expected 12. The search takes a photo from the media cache when the cache holds its thumbnail, and the grid's own thumbnail reads were waiting on the same closed drive; they land in the cache once the test opens it. How many had landed when the second search planned its reads varied, so it read 10 files, or 1 when all had landed. Not a bug in Find Similar (a cached photo needs no read). The test now checks what it is for: no file is read twice, the searched photo is read once, and every photo got a signature (the photos shown).
- **200-page book preview** (`tests::book_preview::a_book_of_two_hundred_pages_previews_and_turns`, #78). The wait for 5 pages took about 3 s on this Mac (load ~60) and 26 to 30 s with 8 copies of the test side by side. The 60 s limit was too tight for a Fork CI runner running the whole crate's tests together. The file's waits are now 10 minutes, and only guard a hang; each one still waits on a condition.
- **Cancelled denoise batch** (`tests::denoise::a_cancelled_batch_does_not_come_back_while_its_job_winds_down`, #20). Two counted-loop waits (10 s for the job to reach its model, 4 s for it to wind down) were timing assumptions. The stopped job also waited its turn at the process-wide memory gate, behind every other test's decode, before it noticed it was stopped: with the gate held for 8 s the test failed 3 of 3. The waits are now `wait_for` (a condition, a 5-minute hang guard), and `make_product` (`crates/engine/src/denoise/products.rs`) checks for a stop right after the model loads, so a stopped job ends at once instead of waiting at the gate. The one change to code under test.

## Branch

`fix/76-flaky-tests`

## Own files

`fork/flaky-tests.md`, `fork/tests.d/flaky-tests.txt`.

## Hooks in official files

`crates/engine/src/denoise/products.rs` (a small hook): a cancel check after the model loads. The test files `crates/ui-egui/src/tests/find_similar.rs`, `crates/ui-egui/src/tests/book_preview.rs` and `crates/engine/src/tests/denoise.rs`.

## Tests

`fork/tests.d/flaky-tests.txt`.
