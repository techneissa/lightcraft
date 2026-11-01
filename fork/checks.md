# Checks for a fork feature

Check in batches:
- after each change, and once more at the end of the round: only the tests of the code it touched
  (`cargo test -p <crate> <filter>`), `cargo clippy -p <crate> --all-targets -- -D warnings` for the crates it
  touched, and `cargo fmt --all --check`;
- the full suite runs on GitHub, not on this Mac: a push to `custom` starts Fork CI (`fork-ci.yml`), which runs the
  official CI (`cargo xtask ci`) on `custom`. Check it with `gh run list -R techneissa/lightcraft -w CI -b custom`
  and fix a failure in the next round.

This fork reads `CLAUDE.md`'s "before every commit" and "run the full one locally before merging" as the above:
no `cargo xtask ci` or `ci --quick` on this Mac (techneissa, 2026-10-29: the same tests ran three times per
feature and loaded the Mac). The official CI runs on Linux, so macOS-only tests (`macraw`, macOS Colour) run
only in a lane's own filtered tests. A failure Fork CI shows only on Linux is noted, not fixed (Mac only, in FORK.md).

## Faster builds on this Mac

- **Build folder per lane slot.** The orchestrator gives each lane a slot, 1 to 3 (never two running lanes on one
  slot). The lane builds with `CARGO_TARGET_DIR=$WORKSPACE/repos/lightcraft/target/lane-<slot>` (the main checkout's
  `target`, outside the lane's worktree) and leaves the folder in place when it finishes, so the next lane on that slot
  rebuilds only what changed. Cargo reuses a target folder across worktree paths. Measured on PhotoCraft (2026-11-02,
  `cargo build -p photocraft`): a new worktree on a used slot recompiled 6 crates in 100 s, against 537 crates and
  6 min in an empty folder (22 min at load ~100); a one-file change, 10 s. Read-only reviews that must build use
  `target/review`. Never delete a slot folder while a lane uses it.
- **No sccache.** Tried and removed on 2026-11-02: a build in another target folder never hits the cache (0 of 49
  hits), and a cold cache made a build slower (46 s against 36 s for `lightcraft-pipeline`).
- **Load is the biggest cost.** The same clean build takes about 6 min at load 10 and 22 min at load ~100: at most
  3 lanes build at once.
- **Batch the Mac's work.** After a single merge nothing builds or tests on the Mac (Fork CI checks `custom`). Once
  per batch of merges: `fork/sync-docs.py`, `fork/tests.sh` (the Mac-only tests), one push.
- **PR and issue body files** are named after the repo and branch (`$TMPDIR/lightcraft-<branch>-body.md`), never a
  shared `$TMPDIR/body.md`: every session on this Mac uses the same `$TMPDIR`.
