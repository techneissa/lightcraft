# Performance: noise reduction bands take a share of the memory budget

List: A full-size render or export on a Mac no longer filters colour noise in one 1.5 GB piece; it works in bands sized from the app's memory budget. Issue #409, branch `perf/409-nr-band-budget`.

## What it does

The joint colour noise reduction sizes its bands from the GPU's free memory. No backend reports it on a Mac, so it took a fixed 2 GB, which held a whole 33 MP frame in one band. Where free memory is unknown, a band now takes a quarter of the memory budget (`crates/engine/src/memory.rs`, `set_band_budget`; 384 MB at the 1.5 GB budget), and keeps at least half its rows (four halos a band at least). The pixels are the same. `LIGHTCRAFT_PROFILE=1` prints the band size. Step 2 of `fork/memory-design.md` (#408).

Measured on a test Mac, `lightcraft-cli render` of the corpus A7 V raw (7008 × 4672) to JPEG with no masks, one run each: peak footprint 5.2 → 3.2 GB, peak GPU buffers 2622 → 1828 MB, the white balance and noise reduction stage 316 → 390 ms, the same JPEG byte for byte.

## Branch

`perf/409-nr-band-budget`

## Own files

`fork/nr-band-budget.md`, `fork/tests.d/nr-band-budget.txt`

## Hooks in official files

`crates/gpu/src/render.rs` (`band_rows`, `color_nr_joint`), `crates/gpu/src/lib.rs` (`set_band_budget`), `crates/gpu/src/ctx.rs` (`BAND_BUDGET`), `crates/engine/src/memory.rs` (`apply`), `crates/gpu/tests/it/equivalence.rs`

## Tests

`fork/tests.d/nr-band-budget.txt`: a small share plans a 33 MP frame in several bands; banded and whole renders give the same pixels.

## When upstream merges

Keep the share where upstream still has the fixed 2 GB (`2048 << 20`) in `color_nr_joint`.
