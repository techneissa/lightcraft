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

What was tried on this Mac (2026-11-02) to speed up builds in a new worktree:

- **No sccache.** It was tried on 2026-11-02 and removed: a build in another worktree never hits the cache (the
  target folder is on rustc's command line: 0 of 49 hits), and a cold cache made a build slower (46 s against 36 s for
  `lightcraft-pipeline`). Copying a built `target/` (below) is what helps.
- **Start a new worktree from a copy of a built `target/`.** Registry crates (about 340 of the 368 that
  `lightcraft-ui-egui` needs) are the same in every worktree, and cargo reuses them from a copied folder; only the
  workspace's own crates rebuild. On APFS `cp -Rc` clones the folder in about a second and costs no disk until files
  change. Before the first build in a new worktree, from another worktree of this repo that has a built `target/` and is not building right now:

  ```
  cp -Rc <another worktree>/target target
  ```

  (the same for the folder `CARGO_TARGET_DIR` names). Measured with `cargo build -p lightcraft-ui-egui` (load about 90):
  326 s from nothing (368 crates), 192 s from a clone in another worktree (25 crates). Delete your `target` when your
  task is done, as before; never another worktree's.
