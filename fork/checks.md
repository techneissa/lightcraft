# Checks for a fork feature

Check in batches:
- after each change, and once more at the end of the round: only the tests of the code it touched
  (`cargo test -p <crate> <filter>`), `cargo clippy -p <crate> --all-targets -- -D warnings` for the crates it
  touched, and `cargo fmt --all --check`;
- a pull request waits for **`fork-gate`** (`fork-ci.yml`): it tests the changed crates and the crates that depend
  on them, on ten machines, and fails for any failing test or clippy warning in them
  ([fork/github-builds.md](github-builds.md)). Merge when `fork-gate` is green;
- the full suite runs on GitHub, not on this Mac: a push to `custom` starts the same workflow (Fork CI) on the whole
  workspace, as `cargo xtask ci` would. Check it with `gh run list -R techneissa/lightcraft -w "Fork CI" -b custom`
  and fix a failure in the next round.

This fork reads `CLAUDE.md`'s "before every commit" and "run the full one locally before merging" as the above:
no `cargo xtask ci` or `ci --quick` on this Mac (techneissa, 2026-10-29: the same tests ran three times per
feature and loaded the Mac). The official CI runs on Linux, so macOS-only tests (`macraw`, macOS Colour) run
only in a lane's own filtered tests. A failure Fork CI shows only on Linux is noted, not fixed (Mac only, in FORK.md).

## Small looks fixes skip the pull request (techneissa, 2026-11-02)

A fix that only changes how something looks (alignment, spacing, size, an icon's drawing, a label, a hover text, a
colour) and changes no behaviour, saved data or command goes straight onto `custom` as its own commit, with no pull
request and no wait for GitHub's `fork-gate`. Run the touched tests, clippy on the touched crate and `cargo fmt
--all --check` first. Fork CI checks `custom` after the push; a failure there is fixed in the next round. Several such
fixes in one round go in together. Anything that changes behaviour, data or a command still goes through a pull
request and `fork-gate`. A lane never pushes to `custom` itself: it commits the fix locally and the orchestrator puts it
on `custom`.

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
- **Build only when it buys something** (techneissa, 2026-11-02): logic tests, AI model runs, and the round's pictures (one build at the end of a round, not after each change). Never rebuild just to see that a button or label is there. Small UI-only changes (a button, a label, hover text, a spacing nudge) are pushed without a local build or test run; `fork-gate` checks them.
- **Batch the Mac's work.** After a single merge nothing builds or tests on the Mac (Fork CI checks `custom`). Once
  per batch of merges: `fork/sync-docs.py`, `fork/tests.sh` (the Mac-only tests), one push.
- **Driving the app** (control channel, screenshots): always with a scratch library (`--library <scratch folder>`) and its own `LIGHTCRAFT_CONFIG_DIR`; without `--library` the app opens techneissa's real library (2026-11-02: a lane imported a test photo into it and had to remove it).
- **PR and issue body files** are named after the repo and branch (`$TMPDIR/lightcraft-<branch>-body.md`), never a
  shared `$TMPDIR/body.md`: every session on this Mac uses the same `$TMPDIR`.
